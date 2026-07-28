//! Negative-path and boundary tests for the admin-gated configuration
//! instructions: `set_withdrawal_fee`, `set_aum_limits`, `unpause`,
//! `set_operator`, `set_fee_recipient`, and the two-step admin nomination.
//!
//! These paths were flagged in the July 2026 due-diligence review as having no
//! passing runtime evidence: `WithdrawalFeeTooHigh`, `AumLimitTooHigh`,
//! `InvalidNominatedAdmin`, and `NominationExpired` had no matching passing
//! assertion anywhere, and the non-admin variants of `unpause`,
//! `set_operator`, `set_fee_recipient`, and `nominate_admin` were only
//! exercised in test files omitted from the default suite.

use august_vault::errors::ErrorCode;
use august_vault::state::vault::FEE_RATE_DENOMINATOR_VALUE;
use integration_tests::harness::{assert_anchor_err, VaultCtx, DEPOSIT_DECIMALS};
use solana_sdk::signature::{Keypair, Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32); // 10 tokens

/// Fee cap from `set_withdrawal_fee::handler`: `new_fee` must be strictly below
/// 10% of the fee denominator.
const FEE_CAP: u32 = FEE_RATE_DENOMINATOR_VALUE / 10;
/// The largest fee the cap admits, one unit below it.
const MAX_VALID_FEE: u32 = FEE_CAP - 1;

/// Nomination validity window from `NominatedAdmin::refresh` (whose own
/// constant is private to the program).
const ONE_DAY_IN_SECONDS: i64 = 24 * 60 * 60;

// ---- set_withdrawal_fee: boundary + access control ----

#[test]
fn set_withdrawal_fee_accepts_maximum_valid_fee() {
    let mut ctx = VaultCtx::fresh();
    ctx.set_withdrawal_fee(MAX_VALID_FEE)
        .expect("fee just below the 10% cap must be accepted");
    assert_eq!(ctx.vault_state_data().withdrawal_fee, MAX_VALID_FEE);
}

#[test]
fn set_withdrawal_fee_rejects_fee_at_ten_percent() {
    let mut ctx = VaultCtx::fresh();
    let err = ctx
        .set_withdrawal_fee(FEE_CAP)
        .expect_err("cap is strict: exactly 10% must be rejected");
    assert_anchor_err(&err, ErrorCode::WithdrawalFeeTooHigh);
    assert_eq!(
        ctx.vault_state_data().withdrawal_fee,
        0,
        "rejected update must not change stored fee"
    );
}

#[test]
fn set_withdrawal_fee_rejects_non_admin() {
    let mut ctx = VaultCtx::fresh();
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .set_withdrawal_fee_as(&impostor, 1)
        .expect_err("non-admin must not set fees");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
}

// ---- set_aum_limits: boundary + access control ----

#[test]
fn set_aum_limits_accepts_maximum_valid_limits() {
    let mut ctx = VaultCtx::fresh();
    ctx.set_aum_limits(10_000, 10_000)
        .expect("100% in basis points is the documented maximum");
    let state = ctx.vault_state_data();
    assert_eq!(state.aum_increase_limit, 10_000);
    assert_eq!(state.aum_decrease_limit, 10_000);
}

#[test]
fn set_aum_limits_rejects_increase_limit_above_maximum() {
    let mut ctx = VaultCtx::fresh();
    let err = ctx
        .set_aum_limits(10_001, 20)
        .expect_err("increase limit above 100% must be rejected");
    assert_anchor_err(&err, ErrorCode::AumLimitTooHigh);
}

#[test]
fn set_aum_limits_rejects_decrease_limit_above_maximum() {
    let mut ctx = VaultCtx::fresh();
    let err = ctx
        .set_aum_limits(20, 10_001)
        .expect_err("decrease limit above 100% must be rejected");
    assert_anchor_err(&err, ErrorCode::AumLimitTooHigh);
    let state = ctx.vault_state_data();
    assert_eq!(
        (state.aum_increase_limit, state.aum_decrease_limit),
        (20, 20),
        "rejected update must leave both stored limits at their defaults"
    );
}

#[test]
fn set_aum_limits_rejects_non_admin() {
    let mut ctx = VaultCtx::fresh();
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .set_aum_limits_as(&impostor, 20, 20)
        .expect_err("non-admin must not set AUM limits");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
}

// ---- unpause: the DD report called out "direct non-admin unpause" as absent ----

#[test]
fn unpause_rejects_non_admin() {
    let mut ctx = VaultCtx::fresh();
    ctx.pause().expect("admin pauses");
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .unpause_as(&impostor)
        .expect_err("non-admin must not unpause");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert!(ctx.vault_state_data().paused, "vault must remain paused");
}

#[test]
fn unpause_restores_deposits() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(2 * DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");

    ctx.pause().expect("admin pauses");
    let err = ctx
        .deposit(DEPOSIT_AMOUNT)
        .expect_err("deposit must fail while paused");
    assert_anchor_err(&err, ErrorCode::VaultPaused);

    ctx.unpause().expect("admin unpauses");
    ctx.deposit(DEPOSIT_AMOUNT)
        .expect("deposit must succeed again after unpause");
}

// ---- set_operator / set_fee_recipient: non-admin rejection ----

#[test]
fn set_operator_rejects_non_admin() {
    let mut ctx = VaultCtx::fresh();
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let target = Keypair::new().pubkey();
    let err = ctx
        .set_operator_as(&impostor, target)
        .expect_err("non-admin must not replace the operator");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(
        ctx.vault_state_data().operator,
        ctx.operator.pubkey(),
        "operator must be unchanged after rejected update"
    );
}

#[test]
fn set_fee_recipient_rejects_non_admin() {
    let mut ctx = VaultCtx::fresh();
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let target = Keypair::new().pubkey();
    let err = ctx
        .set_fee_recipient_as(&impostor, target)
        .expect_err("non-admin must not replace the fee recipient");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(
        ctx.vault_state_data().fee_recipient,
        ctx.fee_recipient.pubkey(),
        "fee recipient must be unchanged after rejected update"
    );
}

// ---- admin nomination: two-step flow, wrong nominee, expiry ----

#[test]
fn nominate_admin_rejects_non_admin() {
    let mut ctx = VaultCtx::fresh();
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .nominate_admin_as(&impostor, impostor.pubkey())
        .expect_err("non-admin must not nominate");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
}

#[test]
fn admin_nomination_two_step_transfer_rotates_admin() {
    let mut ctx = VaultCtx::fresh();
    let new_admin = ctx.new_funded_keypair(1_000_000_000);
    // Accepting rotates `ctx.admin`, so keep the outgoing keypair now.
    let old_admin = ctx.admin.insecure_clone();

    ctx.nominate_admin(new_admin.pubkey()).expect("nominate");
    assert_eq!(
        ctx.vault_state_data().admin,
        old_admin.pubkey(),
        "nomination alone must not change the admin"
    );

    ctx.accept_admin_nomination_as(&new_admin).expect("accept");
    assert_eq!(ctx.vault_state_data().admin, new_admin.pubkey());

    // The nomination PDA is closed on accept; its rent goes to the receiver.
    assert!(
        ctx.svm
            .get_account(&ctx.nominated_admin_pda())
            .is_none_or(|a| a.lamports == 0),
        "nomination PDA must be closed after accept"
    );

    // The old admin is locked out; the new admin holds the role.
    let err = ctx
        .pause_as(&old_admin)
        .expect_err("old admin must be locked out");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    // The no-suffix helper signs with the harness's own admin keypair, so this
    // also pins that accepting the nomination rotated it to `new_admin`.
    assert_eq!(ctx.admin.pubkey(), new_admin.pubkey());
    ctx.pause().expect("new admin must hold the role");
}

#[test]
fn accept_admin_nomination_rejects_wrong_nominee() {
    let mut ctx = VaultCtx::fresh();
    let nominee = ctx.new_funded_keypair(1_000_000_000);
    let interloper = ctx.new_funded_keypair(1_000_000_000);

    ctx.nominate_admin(nominee.pubkey()).expect("nominate");
    let err = ctx
        .accept_admin_nomination_as(&interloper)
        .expect_err("only the nominee may accept");
    assert_anchor_err(&err, ErrorCode::InvalidNominatedAdmin);
    assert_eq!(
        ctx.vault_state_data().admin,
        ctx.admin.pubkey(),
        "admin must be unchanged after rejected accept"
    );
}

#[test]
fn accept_admin_nomination_rejects_expired_nomination() {
    let mut ctx = VaultCtx::fresh();
    let nominee = ctx.new_funded_keypair(1_000_000_000);

    ctx.nominate_admin(nominee.pubkey()).expect("nominate");
    ctx.warp_forward_seconds(ONE_DAY_IN_SECONDS + 1);

    let err = ctx
        .accept_admin_nomination_as(&nominee)
        .expect_err("nomination must expire after 24 hours");
    assert_anchor_err(&err, ErrorCode::NominationExpired);
    assert_eq!(
        ctx.vault_state_data().admin,
        ctx.admin.pubkey(),
        "admin must be unchanged after expired accept"
    );
}

#[test]
fn re_nomination_after_expiry_succeeds() {
    let mut ctx = VaultCtx::fresh();
    let nominee = ctx.new_funded_keypair(1_000_000_000);

    // First nomination expires unused; the PDA stays alive (init_if_needed).
    ctx.nominate_admin(nominee.pubkey()).expect("nominate");
    ctx.warp_forward_seconds(ONE_DAY_IN_SECONDS + 1);

    // Re-nominating refreshes the window through the load_mut path.
    ctx.nominate_admin(nominee.pubkey()).expect("re-nominate");
    ctx.accept_admin_nomination_as(&nominee)
        .expect("accept within the refreshed window");
    assert_eq!(ctx.vault_state_data().admin, nominee.pubkey());
}
