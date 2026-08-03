//! CEI (Check-Effects-Interactions) tests for `redeem`.
//!
//! Asserts that whenever `redeem` fails — for *any* of its named failure modes
//! — every observable state slot is unchanged. Covers the on-chain invariant
//! that backs security audit finding C-01 (no shares burned, no fees moved, no
//! AUM mutation when a require! short-circuits).
//!
//! These tests run in-process via LiteSVM and use typed `ErrorCode` assertions
//! (not log-string matching) so a regression that returns a *different* failure
//! variant for the same redeem call will fail loudly rather than masquerade
//! as a CEI pass.

use august_vault::{errors::ErrorCode, state::vault::VaultState};
use integration_tests::harness::{
    assert_anchor_err, VaultCtx, DEPOSIT_DECIMALS, HARNESS_SHARE_OFFSET,
};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32); // 10 tokens
const OPERATOR_WITHDRAW: u64 = DEPOSIT_AMOUNT * 9 / 10; // 90% drained off-vault

/// Drives a deposit + operator-withdraw so `local_aum` is too small to redeem
/// the user's full share balance.
fn vault_with_low_liquidity() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");
    ctx.operator_withdraw(OPERATOR_WITHDRAW)
        .expect("operator withdraw");
    ctx
}

// ---- liquidity-failure CEI: the C-01 scenario ----

#[test]
fn redeem_with_insufficient_liquidity_fails_with_typed_error() {
    let mut ctx = vault_with_low_liquidity();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert!(shares > 0, "user should hold shares after deposit");

    let err = ctx
        .redeem(shares)
        .expect_err("redeem should fail when liquidity is insufficient");
    assert_anchor_err(&err, ErrorCode::NotEnoughLiquidity);
}

#[test]
fn redeem_with_insufficient_liquidity_leaves_no_side_effects() {
    let mut ctx = vault_with_low_liquidity();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let before = ctx.snapshot();

    let err = ctx.redeem(shares).expect_err("should fail");
    assert_anchor_err(&err, ErrorCode::NotEnoughLiquidity);

    assert_eq!(ctx.snapshot(), before, "CEI violated on NotEnoughLiquidity");
}

// ---- pause-failure CEI: the same invariant must hold for every revert path ----

#[test]
fn redeem_when_paused_leaves_no_side_effects() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");

    ctx.pause().expect("admin pauses vault");

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let before = ctx.snapshot();

    let err = ctx.redeem(shares).expect_err("redeem must fail on pause");
    assert_anchor_err(&err, ErrorCode::VaultPaused);

    assert_eq!(ctx.snapshot(), before, "CEI violated on VaultPaused");
}

// ---- zero-amount CEI: pre-CPI require! short-circuit must leave state intact ----

#[test]
fn redeem_zero_shares_leaves_no_side_effects() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");

    let before = ctx.snapshot();

    let err = ctx.redeem(0).expect_err("redeem must reject 0 shares");
    assert_anchor_err(&err, ErrorCode::ZeroAmount);

    assert_eq!(ctx.snapshot(), before, "CEI violated on ZeroAmount");
}

// ---- happy-path control: a successful partial redeem mutates state by the exact
// amount the on-chain math predicts. Distinguishes a real CEI hold from a
// fixture that can't actually redeem at all.

#[test]
fn partial_redeem_within_liquidity_succeeds() {
    let mut ctx = vault_with_low_liquidity();
    let local_aum = ctx.vault_state_data().local_aum;
    let total_assets = ctx.vault_state_data().total_assets().unwrap();
    let supply = ctx.share_mint_supply();
    let shares_held = ctx.token_account_amount(&ctx.user_share_ata);

    // Choose `safe_shares` so the implied payout is bounded by `target_assets`.
    // Property (proved by `round_trip_never_extracts_value` in `state/vault.rs`):
    //   assets_for_redeem_with_offset(shares_for_deposit_with_offset(target_assets)) <= target_assets
    // because both helpers floor-round in the protocol's favour. With
    // `target_assets = local_aum / 2`, the predicted payout is guaranteed
    // `< local_aum`, so liquidity will never be the failure mode here.
    let target_assets = local_aum / 2;
    let safe_shares = VaultState::shares_for_deposit_with_offset(
        supply,
        total_assets,
        target_assets,
        HARNESS_SHARE_OFFSET,
    )
    .expect("share math fits u64")
    .min(shares_held);
    assert!(
        safe_shares > 0,
        "fixture must leave room for a partial redeem"
    );

    // Predict the exact asset payout using the on-chain helper. Fee=0 by
    // default in this fixture, so the user receives `expected_assets` and the
    // fee recipient receives 0.
    let expected_assets = VaultState::assets_for_redeem_with_offset(
        supply,
        total_assets,
        safe_shares,
        HARNESS_SHARE_OFFSET,
    )
    .expect("asset math fits u64");
    assert!(expected_assets > 0, "predicted payout must be non-trivial");
    assert_eq!(
        ctx.vault_state_data().withdrawal_fee,
        0,
        "fixture assumes the default zero withdrawal fee",
    );

    let vault_before = ctx.token_account_amount(&ctx.vault_token_pda);
    let user_before = ctx.token_account_amount(&ctx.user_deposit_ata);
    let fee_recipient_before = ctx.token_account_amount(&ctx.fee_recipient_deposit_ata);
    let supply_before = ctx.share_mint_supply();

    ctx.redeem(safe_shares)
        .expect("partial redeem under liquidity should succeed");

    let vault_after = ctx.token_account_amount(&ctx.vault_token_pda);
    let user_after = ctx.token_account_amount(&ctx.user_deposit_ata);
    let fee_recipient_after = ctx.token_account_amount(&ctx.fee_recipient_deposit_ata);
    let supply_after = ctx.share_mint_supply();

    assert_eq!(
        vault_before - vault_after,
        expected_assets,
        "vault must drop by exactly the predicted asset payout"
    );
    assert_eq!(
        user_after - user_before,
        expected_assets,
        "user must receive exactly the predicted asset payout (no fee in this fixture)"
    );
    assert_eq!(
        fee_recipient_after, fee_recipient_before,
        "fee branch must not move tokens when withdrawal_fee = 0",
    );
    assert_eq!(
        supply_before - supply_after,
        safe_shares,
        "share supply must drop by exactly the shares burned"
    );
}
