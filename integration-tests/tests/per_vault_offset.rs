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
use integration_tests::harness::{assert_anchor_err, harness_min_first_deposit, VaultCtx};

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
// These two tests write a vault state whose stored offset differs from the
// global default and then transact through the real instructions, so they fail
// if a handler reads the constant instead of the vault's own value.

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
    let with_default = VaultState::shares_for_deposit(supply, total, amount, EXTRA_SHARES).unwrap();
    let with_raw_zero = VaultState::shares_for_deposit(supply, total, amount, 0).unwrap();
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
