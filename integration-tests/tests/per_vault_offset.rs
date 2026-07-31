//! Per-vault share offsets.
//!
//! The virtual-share offset must dominate a 1-unit retained sliver, so it is an
//! absolute count; but `MIN_SUPPLY_MULTIPLE * offset` is the minimum first
//! deposit, whose *cost* is that count times what a base unit of the mint is
//! worth. One global constant therefore cannot serve both a 6-decimal dollar
//! stablecoin and an 8-decimal asset worth ~$100k — at an offset sized for the
//! stablecoin, opening the latter would cost six figures.
//!
//! So the offset is chosen per vault, at `initialize`, by the (permissioned)
//! protocol authority, and fixed for the vault's life. These tests pin that it
//! is validated, persisted, actually used by the pricing, and that a vault
//! created before the field existed still behaves.

use august_vault::errors::ErrorCode;
use august_vault::state::vault::{
    VaultState, EXTRA_SHARES, MAX_SHARE_OFFSET, MIN_SHARE_OFFSET, MIN_SUPPLY_MULTIPLE,
};
use integration_tests::harness::{
    assert_anchor_err, harness_min_first_deposit, VaultCtx, DEPOSIT_DECIMALS,
};

/// Offsets outside the permitted band, or not a power of ten, must be refused —
/// the field cannot be changed after creation, so a bad value would weaken the
/// defences for the life of the vault.
#[test]
fn initialize_rejects_an_invalid_offset() {
    let mut ctx = VaultCtx::fresh();
    let authority = ctx.protocol_authority.insecure_clone();

    for bad in [
        0u128,
        1,
        100,
        MIN_SHARE_OFFSET - 1,
        5_000,
        MAX_SHARE_OFFSET * 10,
    ] {
        let mint = ctx.create_extra_deposit_mint();
        let err = ctx
            .try_initialize_vault_with_offset(&authority, mint, 1, bad as u64)
            .expect_err(&format!("offset {bad} must be refused"));
        assert_anchor_err(&err, ErrorCode::InvalidShareOffset);
    }
}

/// A valid offset is persisted verbatim, and the vault prices against it rather
/// than against the global default.
#[test]
fn a_chosen_offset_is_stored_and_used() {
    let mut ctx = VaultCtx::fresh();
    let authority = ctx.protocol_authority.insecure_clone();
    let mint = ctx.create_extra_deposit_mint();
    let offset = MIN_SHARE_OFFSET;

    ctx.try_initialize_vault_with_offset(&authority, mint, 3, offset as u64)
        .expect("a power of ten inside the band is accepted");
    assert_eq!(
        ctx.stored_share_offset_raw(mint, 3) as u128,
        offset,
        "the requested offset must be what is stored"
    );

    // The minimum first deposit follows that offset. Note BOTH floors apply:
    // at 9 decimals the decimals floor (10^6) is the larger and binds instead,
    // which is why the offset floor is a `max` and not the only rule.
    assert!(VaultState::min_first_deposit_for(9, offset) as u128 >= MIN_SUPPLY_MULTIPLE * offset);
    // At 6 decimals the offset floor is the binding one, so lowering the offset
    // genuinely lowers the opening deposit — the point of making it per-vault.
    assert_eq!(
        VaultState::min_first_deposit_for(6, offset) as u128,
        MIN_SUPPLY_MULTIPLE * offset
    );
    assert!(
        VaultState::min_first_deposit_for(6, offset)
            < VaultState::min_first_deposit_for(6, EXTRA_SHARES),
        "a smaller offset must permit a smaller opening deposit"
    );
}

/// Vaults created before `share_offset` existed store zero, which must resolve
/// to the default rather than disabling the defences. The harness vault stands
/// in for that shape via the accessor.
#[test]
fn the_default_offset_still_prices_the_existing_vault() {
    let mut ctx = VaultCtx::fresh();
    let state = ctx.vault_state_data();
    assert_eq!(
        state.share_offset(),
        EXTRA_SHARES,
        "harness vaults use the default offset"
    );

    // And it actually prices with it: a first deposit mints 1:1.
    let amount = VaultState::min_first_deposit_for(9, state.share_offset());
    let d = ctx.new_depositor(amount);
    ctx.deposit_as(&d, amount).expect("first deposit");
    assert_eq!(ctx.token_account_amount(&d.share_ata), amount);
}

// ---- the offset must reach the HANDLERS, not just the account ----
//
// The tests above prove the offset is validated and persisted. Neither proves
// `deposit` / `redeem` actually consult it: every harness vault stores
// `HARNESS_SHARE_OFFSET`, which equals the global `EXTRA_SHARES` default, so an
// expectation computed either way is identical. Verified by mutation — replacing
// `vault_state.share_offset()` with `EXTRA_SHARES` in both handlers left all 66
// unit tests and all 117 LiteSVM tests green.
//
// These tests write a vault state whose stored offset differs from the global
// default and then transact through the real instructions, so they fail if a
// handler reads the constant instead of the vault's own value.
//
// Both legs must be covered. An earlier version of this section claimed "both
// handlers" while only ever calling `deposit`, so the redemption side of that
// same mutation still survived: `assets_for_redeem`'s offset argument was
// pinned by nothing in the tree.

/// A vault storing a NON-DEFAULT offset must be priced with it.
///
/// Discriminator: the harness mint has 9 decimals, so the first-deposit floor is
/// `max(10^6, MIN_SUPPLY_MULTIPLE * offset)` — `10^6` at `MIN_SHARE_OFFSET` but
/// `10^8` at the default. A deposit of exactly `10^6` therefore succeeds only if
/// the handler used the vault's stored offset; it fails `InsufficientAmount` if
/// it fell back to `EXTRA_SHARES`.
#[test]
fn a_non_default_offset_reaches_the_deposit_handler() {
    let mut ctx = VaultCtx::fresh();

    // Same vault, but stored offset lowered to the minimum.
    let mut state = ctx.vault_state_data();
    state.share_offset = MIN_SHARE_OFFSET as u64;
    ctx.force_overwrite_vault_state(state);
    assert_eq!(
        ctx.vault_state_data().share_offset(),
        MIN_SHARE_OFFSET,
        "precondition: the vault must now resolve to the minimum offset"
    );

    let amount = VaultState::min_first_deposit_for(9, MIN_SHARE_OFFSET);
    assert_eq!(amount, 1_000_000, "discriminator relies on this floor");
    assert!(
        amount < VaultState::min_first_deposit_for(9, EXTRA_SHARES),
        "the two offsets must disagree, or this test proves nothing"
    );

    let d = ctx.new_depositor(amount);
    ctx.deposit_as(&d, amount).expect(
        "a deposit at this vault's own floor must be accepted — if this fails with \
         InsufficientAmount the handler used the global EXTRA_SHARES default \
         instead of the vault's stored share_offset",
    );
    assert_eq!(
        ctx.token_account_amount(&d.share_ata),
        amount,
        "1:1 first mint"
    );
}

/// A vault storing ZERO — the shape of the two live mainnet vaults, whose
/// padding predates this field — must be priced with the default, not with 0.
///
/// A raw-field read would make the offset 0, which removes the ghost co-holder
/// entirely and reinstates the share-inflation and burn exposure the offsets
/// exist to prevent. Priced at a non-par rate so 0 and the default diverge; at
/// par they are identical and nothing would be proven.
#[test]
fn a_stored_zero_offset_is_priced_with_the_default() {
    let mut ctx = VaultCtx::fresh();

    // Seed the vault so a supply exists.
    let seed = harness_min_first_deposit();
    let s = ctx.new_depositor(seed);
    ctx.deposit_as(&s, seed).expect("seed deposit");

    // Force the legacy shape — a stored zero — and move the price off par in the
    // same write. `deployed_aum` is externally held capital so it needs no
    // matching reserve balance, which avoids the operator's relative AUM window
    // (bounded against a `deployed_aum` that starts at zero).
    let mut state = ctx.vault_state_data();
    state.share_offset = 0;
    state.deployed_aum = seed; // total_assets = 2 * supply -> share price of 2
    ctx.force_overwrite_vault_state(state);
    let state = ctx.vault_state_data();
    assert_eq!(state.share_offset, 0, "precondition: raw field is zero");
    assert_eq!(
        state.share_offset(),
        EXTRA_SHARES,
        "the accessor must resolve zero to the default"
    );

    let supply = ctx.share_mint_supply();
    let total = state.total_assets().unwrap();
    let amount = seed / 10;

    // What the handler must mint, and what it would mint on a raw-zero read.
    let with_default =
        VaultState::shares_for_deposit_with_offset(supply, total, amount, EXTRA_SHARES).unwrap();
    let with_raw_zero =
        VaultState::shares_for_deposit_with_offset(supply, total, amount, 0).unwrap();
    assert_ne!(
        with_default, with_raw_zero,
        "test setup: the two must diverge here or this proves nothing"
    );

    let d = ctx.new_depositor(amount);
    ctx.deposit_as(&d, amount)
        .expect("deposit into a legacy-shaped vault");
    assert_eq!(
        ctx.token_account_amount(&d.share_ata),
        with_default,
        "a vault storing 0 must price with EXTRA_SHARES; minting {with_raw_zero} \
         would mean the handler read the raw field and disabled the offsets"
    );
}

/// The redemption twin of the two tests above: `redeem` must price with the
/// vault's stored offset too.
///
/// Priced off par, because at par every offset gives the same answer. A vault
/// storing `MIN_SHARE_OFFSET` pays a different amount from one priced at the
/// `EXTRA_SHARES` default, so substituting the constant in the handler changes
/// the payout and fails here.
#[test]
fn a_non_default_offset_reaches_the_redeem_handler() {
    let mut ctx = VaultCtx::fresh();

    // Seed at the default offset so the deposit leg is unaffected, then take the
    // price off par through the real operator flow — at par every offset pays
    // the same and the test would prove nothing. The operator holds the
    // withdrawn tokens, so returning them restores redeemable liquidity.
    let seed = harness_min_first_deposit();
    let s = ctx.new_depositor(seed);
    ctx.deposit_as(&s, seed).expect("seed deposit");
    ctx.operator_withdraw(seed).expect("deploy capital");
    ctx.set_aum_limits(10_000, 10_000)
        .expect("widen the AUM window");
    ctx.operator_update_aum(seed * 2)
        .expect("report a gain, taking the share price to 2.0");
    ctx.operator_deposit(seed)
        .expect("recapitalise the reserve");

    // Only now lower the stored offset, leaving the AUM figures alone.
    let mut state = ctx.vault_state_data();
    state.share_offset = MIN_SHARE_OFFSET as u64;
    ctx.force_overwrite_vault_state(state);
    let state = ctx.vault_state_data();
    assert_eq!(state.share_offset(), MIN_SHARE_OFFSET, "precondition");

    let supply = ctx.share_mint_supply();
    let total = state.total_assets().unwrap();
    let shares = seed / 10;

    let with_stored =
        VaultState::assets_for_redeem_with_offset(supply, total, shares, MIN_SHARE_OFFSET).unwrap();
    let with_default =
        VaultState::assets_for_redeem_with_offset(supply, total, shares, EXTRA_SHARES).unwrap();
    assert_ne!(
        with_stored, with_default,
        "test setup: the two offsets must pay differently here or this proves nothing"
    );

    let before = ctx.token_account_amount(&s.deposit_ata);
    ctx.redeem_as(&s, shares).expect("redeem must succeed");
    let paid = ctx.token_account_amount(&s.deposit_ata) - before;
    assert_eq!(
        paid, with_stored,
        "redeem must price with the vault's stored offset; paying {with_default} \
         would mean the handler used the global EXTRA_SHARES default"
    );
}

/// The first-deposit floor is an invariant about **opening supply**, and it is
/// enforced on the minted shares rather than on the deposited amount.
///
/// With assets present at zero supply — reachable via the pro-rata cap's
/// residual, an external burn of the whole supply, or an AUM report at zero
/// supply — a deposit of exactly `min_first_deposit_for` mints
/// `amount * offset / (total_assets + offset)`, strictly fewer shares than the
/// amount, and can floor to zero. Before the shares-side check, that opened the
/// vault with the offset co-holder owning most of it, or rejected a perfectly
/// valid deposit with `ZeroAmount`.
#[test]
fn residual_assets_at_zero_supply_do_not_break_the_opening_floor() {
    let mut ctx = VaultCtx::fresh();

    // Assets with no shares outstanding, written directly: the states that
    // produce it naturally (a full external burn, the redeem residual) all end
    // in the same place.
    let residual = harness_min_first_deposit();
    let mut state = ctx.vault_state_data();
    state.deployed_aum = residual;
    ctx.force_overwrite_vault_state(state);
    assert_eq!(ctx.share_mint_supply(), 0, "precondition: no shares");

    let state = ctx.vault_state_data();
    let advertised = state.min_first_deposit(DEPOSIT_DECIMALS);
    let total = state.total_assets().unwrap();
    let would_mint =
        VaultState::shares_for_deposit_with_offset(0, total, advertised, state.share_offset())
            .unwrap();
    assert!(
        would_mint < state.min_opening_supply(),
        "test setup: the advertised amount must be insufficient here ({would_mint} \
         shares against a floor of {})",
        state.min_opening_supply()
    );

    // Paying the advertised minimum is refused, and refused for the right
    // reason: too small, not "amount must be > 0".
    let d = ctx.new_depositor(advertised);
    let err = ctx
        .deposit_as(&d, advertised)
        .expect_err("the advertised amount cannot open this vault");
    assert_anchor_err(&err, ErrorCode::InsufficientAmount);
    assert_eq!(
        ctx.share_mint_supply(),
        0,
        "a rejected opening deposit must mint nothing"
    );

    // A deposit large enough to clear the floor is accepted, and the vault
    // opens at or above the required supply. Solved rather than guessed:
    // shares = amount * offset / (total + offset), so clearing a floor of
    // `min_opening_supply` needs `amount >= floor * (total + offset) / offset`.
    // At the harness offsets that is ~101x the advertised amount, which is the
    // point — the advertised figure is nowhere near sufficient here.
    let floor = state.min_opening_supply() as u128;
    let offset = state.share_offset();
    let enough = u64::try_from(floor * (total as u128 + offset) / offset + 1)
        .expect("the required deposit must fit u64");
    let d2 = ctx.new_depositor(enough);
    ctx.deposit_as(&d2, enough)
        .expect("a deposit that clears the opening floor must be accepted");
    assert!(
        ctx.share_mint_supply() >= ctx.vault_state_data().min_opening_supply(),
        "the vault must open at or above min_opening_supply"
    );
}
