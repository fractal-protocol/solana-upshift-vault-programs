//! Token-2022 smoke test: exercises the `VaultCtx::fresh_token_2022()`
//! constructor end-to-end. The vault math doesn't care about token program;
//! this test exists to prove the `InterfaceAccount` deposit/redeem/operator
//! CPI paths work against an `spl_token_2022::ID`-owned mint.
//!
//! Limited to vanilla Token-2022 mints (no extensions). The harness's
//! `token_account_amount` reads only the base 165-byte layout shared between
//! SPL Token and Token-2022, so extension-aware accounting (transfer fees,
//! transfer hooks) is intentionally out of scope here.

use august_vault::{errors::ErrorCode, state::vault::VaultState};
use integration_tests::harness::{assert_anchor_err, VaultCtx, DEPOSIT_DECIMALS};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);

#[test]
fn deposit_and_redeem_round_trip_under_token_2022() {
    let mut ctx = VaultCtx::fresh_token_2022();

    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT)
        .expect("Token-2022 deposit should succeed");

    let shares_minted = ctx.token_account_amount(&ctx.user_share_ata);
    assert!(shares_minted > 0, "deposit must mint shares");
    assert_eq!(
        ctx.token_account_amount(&ctx.vault_token_pda),
        DEPOSIT_AMOUNT,
        "vault reserve should hold the full deposit"
    );

    let supply = ctx.share_mint_supply();
    let total_assets = ctx.vault_state_data().total_assets().unwrap();
    let expected_payout = VaultState::assets_for_redeem(supply, total_assets, shares_minted)
        .expect("redeem math fits u64");

    let user_before = ctx.token_account_amount(&ctx.user_deposit_ata);
    ctx.redeem(shares_minted)
        .expect("Token-2022 redeem should succeed");
    let user_after = ctx.token_account_amount(&ctx.user_deposit_ata);

    assert_eq!(
        user_after - user_before,
        expected_payout,
        "Token-2022 redeem payout matches on-chain math"
    );
    assert_eq!(
        ctx.share_mint_supply(),
        0,
        "every share burned on full redeem"
    );
}

#[test]
fn cei_holds_under_token_2022_zero_amount() {
    // `ZeroAmount` CEI under Token-2022: a 0-shares redeem must reject and
    // leave every state slot unchanged, identical to the SPL-Token path.
    let mut ctx = VaultCtx::fresh_token_2022();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("seed deposit");

    let before = ctx.snapshot();
    let err = ctx
        .redeem(0)
        .expect_err("redeem must reject 0 shares under Token-2022");
    assert_anchor_err(&err, ErrorCode::ZeroAmount);

    assert_eq!(
        ctx.snapshot(),
        before,
        "Token-2022 CEI violated on ZeroAmount"
    );
}

#[test]
fn cei_holds_under_token_2022_not_enough_liquidity() {
    // Same `NotEnoughLiquidity` scenario as the SPL-Token CEI suite, now
    // possible under Token-2022 because `operator_withdraw` was updated to
    // pass `associated_token::token_program = token_program` (see
    // `operator_withdraw.rs`/`operator_deposit.rs`).
    let mut ctx = VaultCtx::fresh_token_2022();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("seed deposit");
    ctx.operator_withdraw(DEPOSIT_AMOUNT * 9 / 10)
        .expect("Token-2022 operator_withdraw should succeed");

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let before = ctx.snapshot();

    let err = ctx
        .redeem(shares)
        .expect_err("redeem must fail when local_aum is starved");
    assert_anchor_err(&err, ErrorCode::NotEnoughLiquidity);

    assert_eq!(
        ctx.snapshot(),
        before,
        "Token-2022 CEI violated on NotEnoughLiquidity"
    );
}

#[test]
fn operator_round_trip_under_token_2022() {
    // operator_withdraw → operator_deposit round-trip should be balance-
    // preserving on the deployed_aum/local_aum accounting under Token-2022.
    let mut ctx = VaultCtx::fresh_token_2022();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("seed deposit");

    let before_local = ctx.vault_state_data().local_aum;
    let withdraw_amount = DEPOSIT_AMOUNT / 4;

    ctx.operator_withdraw(withdraw_amount)
        .expect("operator_withdraw under Token-2022");
    let after_withdraw = ctx.vault_state_data();
    assert_eq!(after_withdraw.local_aum, before_local - withdraw_amount);
    assert_eq!(after_withdraw.deployed_aum, withdraw_amount);

    ctx.operator_deposit(withdraw_amount)
        .expect("operator_deposit under Token-2022");
    let after_deposit = ctx.vault_state_data();
    assert_eq!(after_deposit.local_aum, before_local);
    assert_eq!(after_deposit.deployed_aum, 0);
}
