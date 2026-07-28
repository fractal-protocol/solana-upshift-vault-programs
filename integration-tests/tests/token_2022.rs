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
fn close_vault_lifecycle_under_token_2022() {
    // Full lifecycle: init → deposit → full redeem → close. The DD review
    // flagged Token-2022 closure as untested; this proves the CloseAccount
    // and SetAuthority CPIs work against the Token-2022 program and that the
    // rent from both closed accounts lands with the admin.
    use solana_sdk::program_pack::Pack;
    use solana_sdk::signature::Signer;

    let mut ctx = VaultCtx::fresh_token_2022();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("seed deposit");

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    ctx.redeem(shares).expect("full redeem empties the vault");
    assert_eq!(ctx.share_mint_supply(), 0);
    assert_eq!(ctx.token_account_amount(&ctx.vault_token_pda), 0);

    let rent_to_reclaim = ctx.svm.get_account(&ctx.vault_state).unwrap().lamports
        + ctx.svm.get_account(&ctx.vault_token_pda).unwrap().lamports;
    let admin_before = ctx.svm.get_account(&ctx.admin.pubkey()).unwrap().lamports;

    ctx.close_vault()
        .expect("close_vault must succeed under Token-2022");

    assert!(
        ctx.svm
            .get_account(&ctx.vault_state)
            .is_none_or(|a| a.lamports == 0),
        "vault_state must be closed"
    );
    assert!(
        ctx.svm
            .get_account(&ctx.vault_token_pda)
            .is_none_or(|a| a.lamports == 0),
        "vault token account must be closed"
    );

    // The share mint survives (SPL mints cannot be closed) but its mint
    // authority must be revoked so no shares can ever be minted again.
    let mint_acct = ctx.svm.get_account(&ctx.share_mint).expect("mint remains");
    let mint =
        spl_token_2022::state::Mint::unpack(&mint_acct.data[..spl_token_2022::state::Mint::LEN])
            .expect("valid mint");
    assert!(
        mint.mint_authority.is_none(),
        "share mint authority must be revoked on close"
    );

    let admin_after = ctx.svm.get_account(&ctx.admin.pubkey()).unwrap().lamports;
    // The admin paid one transaction fee and received both accounts' rent.
    assert!(
        admin_after > admin_before && admin_after - admin_before <= rent_to_reclaim,
        "admin must net-receive the reclaimed rent (got {} → {}, rent {})",
        admin_before,
        admin_after,
        rent_to_reclaim,
    );
}

#[test]
fn close_vault_rejects_non_admin_under_token_2022() {
    let mut ctx = VaultCtx::fresh_token_2022();
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .close_vault_as(&impostor)
        .expect_err("non-admin must not close the vault");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
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
