//! Fee-bearing redeem: asserts that with `withdrawal_fee > 0`, fees transfer
//! exactly once to the fee recipient, the user receives `assets - fee`, and
//! the share supply drops by exactly the burned amount.
//!
//! The existing CEI suite runs with `withdrawal_fee = 0` (initializer default).
//! This file exercises the fee branch end-to-end.

use august_vault::{errors::ErrorCode, state::vault::VaultState};
use integration_tests::harness::{
    assert_anchor_err, expected_withdrawal_fee, VaultCtx, DEPOSIT_DECIMALS,
};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
// Denominator is FEE_RATE_DENOMINATOR_VALUE = 1_000_000. Program caps the fee
// at strictly less than 10% (= 100_000); 5% is well within range.
const FEE_RATE_E6: u32 = 50_000;

#[test]
fn successful_redeem_transfers_exact_fee_and_assets() {
    let mut ctx = VaultCtx::fresh();
    ctx.set_withdrawal_fee(FEE_RATE_E6)
        .expect("admin sets withdrawal fee");

    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");
    let shares_held = ctx.token_account_amount(&ctx.user_share_ata);

    // Redeem half so we still have a vault left to inspect.
    let shares_to_burn = shares_held / 2;

    // Compute the protocol's expected fee/assets using the exact on-chain math.
    let state = ctx.vault_state_data();
    let supply = ctx.share_mint_supply();
    let total_assets = state.total_assets().unwrap();
    let assets = VaultState::assets_for_redeem(supply, total_assets, shares_to_burn).unwrap();
    let expected_fee = expected_withdrawal_fee(assets, FEE_RATE_E6);
    let expected_user_out = assets - expected_fee;
    assert!(expected_fee > 0, "test must exercise a non-zero fee path");

    let user_before = ctx.token_account_amount(&ctx.user_deposit_ata);
    let fee_before = ctx.token_account_amount(&ctx.fee_recipient_deposit_ata);
    let supply_before = ctx.share_mint_supply();

    ctx.redeem(shares_to_burn).expect("redeem under liquidity");

    let user_after = ctx.token_account_amount(&ctx.user_deposit_ata);
    let fee_after = ctx.token_account_amount(&ctx.fee_recipient_deposit_ata);
    let supply_after = ctx.share_mint_supply();

    assert_eq!(
        fee_after - fee_before,
        expected_fee,
        "fee recipient received the wrong amount"
    );
    assert_eq!(
        user_after - user_before,
        expected_user_out,
        "user received the wrong net amount"
    );
    assert_eq!(
        supply_before - supply_after,
        shares_to_burn,
        "share supply did not drop by the burned amount"
    );
}

#[test]
fn failed_redeem_with_fee_set_does_not_transfer_fee() {
    // Same setup as above, but liquidity-starve the vault first so redeem fails.
    let mut ctx = VaultCtx::fresh();
    ctx.set_withdrawal_fee(FEE_RATE_E6)
        .expect("admin sets withdrawal fee");

    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");
    ctx.operator_withdraw(DEPOSIT_AMOUNT * 9 / 10)
        .expect("operator withdraws 90%");

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let before = ctx.snapshot();

    let err = ctx
        .redeem(shares)
        .expect_err("redeem must fail with low liquidity");
    assert_anchor_err(&err, ErrorCode::NotEnoughLiquidity);

    let after = ctx.snapshot();
    assert_eq!(
        after.fee_recipient_tokens, before.fee_recipient_tokens,
        "fee transferred despite redeem failure"
    );
    assert_eq!(
        after, before,
        "any state slot changed despite redeem failure"
    );
}
