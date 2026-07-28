//! Negative-path and boundary tests for the operator instructions:
//! `operator_update_aum` (AUM change limits), `operator_withdraw`, and
//! `operator_deposit`.
//!
//! The July 2026 due-diligence review flagged `AumIncreaseTooBig` /
//! `AumDecreaseTooBig` as asserted only in test files omitted from the default
//! suite, and wrong-operator withdrawals plus zero-value operator transfers as
//! untested entirely. These tests provide passing runtime evidence for each.

use august_vault::errors::ErrorCode;
use integration_tests::harness::{assert_anchor_err, VaultCtx, DEPOSIT_DECIMALS};
use solana_sdk::signature::Signer;

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32); // 10 tokens
const DEPLOYED: u64 = 1_000_000_000; // 1 token deployed off-vault

/// Vault with `deployed_aum = DEPLOYED`, reached through a real
/// `operator_withdraw` (the only way deployed AUM grows without an
/// `operator_update_aum`, whose limits are the thing under test).
fn vault_with_deployed_aum() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");
    ctx.operator_withdraw(DEPLOYED).expect("operator withdraw");
    assert_eq!(ctx.vault_state_data().deployed_aum, DEPLOYED);
    ctx
}

// ---- operator_update_aum: ±0.2% default limits, exact boundaries ----
//
// Guard from `operator_update_aum::handler`:
//   new_aum * 10000 >= (10000 - decrease_bps) * deployed_aum   (decrease side)
//   new_aum * 10000 <= (10000 + increase_bps) * deployed_aum   (increase side)
// Both default to 20 bps (`VaultState::initialize`).

/// Default AUM change limits set by `VaultState::initialize`, in basis points.
const DEFAULT_LIMIT_BPS: u32 = 20;

/// Largest report the increase guard accepts: `floor(deployed * (10000 + bps) /
/// 10000)`. Multiply before dividing (in `u128`) so the boundary stays exact
/// for any `deployed` magnitude, not just multiples of 10 000.
fn max_accepted_aum(deployed: u64, increase_bps: u32) -> u64 {
    ((deployed as u128) * (10_000 + increase_bps as u128) / 10_000) as u64
}

/// Smallest report the decrease guard accepts: `ceil(deployed * (10000 - bps) /
/// 10000)` — ceiling, because the guard is a `>=` on the scaled-up value.
fn min_accepted_aum(deployed: u64, decrease_bps: u32) -> u64 {
    ((deployed as u128) * (10_000 - decrease_bps as u128)).div_ceil(10_000) as u64
}

#[test]
fn update_aum_accepts_exact_increase_boundary() {
    let mut ctx = vault_with_deployed_aum();
    let boundary = max_accepted_aum(DEPLOYED, DEFAULT_LIMIT_BPS); // +0.2% exactly
    ctx.operator_update_aum(boundary)
        .expect("exact +0.2% must be accepted");
    assert_eq!(ctx.vault_state_data().deployed_aum, boundary);
}

#[test]
fn update_aum_rejects_one_above_increase_boundary() {
    let mut ctx = vault_with_deployed_aum();
    let boundary = max_accepted_aum(DEPLOYED, DEFAULT_LIMIT_BPS);
    let err = ctx
        .operator_update_aum(boundary + 1)
        .expect_err("one unit above +0.2% must be rejected");
    assert_anchor_err(&err, ErrorCode::AumIncreaseTooBig);
    assert_eq!(
        ctx.vault_state_data().deployed_aum,
        DEPLOYED,
        "rejected update must not change deployed_aum"
    );
}

#[test]
fn update_aum_accepts_exact_decrease_boundary() {
    let mut ctx = vault_with_deployed_aum();
    let boundary = min_accepted_aum(DEPLOYED, DEFAULT_LIMIT_BPS); // −0.2% exactly
    ctx.operator_update_aum(boundary)
        .expect("exact −0.2% must be accepted");
    assert_eq!(ctx.vault_state_data().deployed_aum, boundary);
}

#[test]
fn update_aum_rejects_one_below_decrease_boundary() {
    let mut ctx = vault_with_deployed_aum();
    let boundary = min_accepted_aum(DEPLOYED, DEFAULT_LIMIT_BPS);
    let err = ctx
        .operator_update_aum(boundary - 1)
        .expect_err("one unit below −0.2% must be rejected");
    assert_anchor_err(&err, ErrorCode::AumDecreaseTooBig);
    assert_eq!(
        ctx.vault_state_data().deployed_aum,
        DEPLOYED,
        "rejected update must not change deployed_aum"
    );
}

#[test]
fn update_aum_honours_admin_configured_limits() {
    let mut ctx = vault_with_deployed_aum();
    // Admin widens the window to ±1%; +0.5% must now pass.
    ctx.set_aum_limits(100, 100).expect("admin widens limits");
    let half_percent_up = max_accepted_aum(DEPLOYED, 50);
    ctx.operator_update_aum(half_percent_up)
        .expect("+0.5% within the widened ±1% window");

    // And +2% relative to the new value must still fail.
    let two_percent_up = max_accepted_aum(half_percent_up, 200);
    let err = ctx
        .operator_update_aum(two_percent_up)
        .expect_err("+2% exceeds the ±1% window");
    assert_anchor_err(&err, ErrorCode::AumIncreaseTooBig);
}

/// Pins a structural property of the guard: with `deployed_aum = 0` every
/// nonzero report fails the increase check (`new_aum * 10000 <= limit * 0`),
/// so recorded deployed AUM can only ever leave zero through
/// `operator_withdraw`, never through a bare report.
#[test]
fn update_aum_from_zero_rejects_any_nonzero_report() {
    let mut ctx = VaultCtx::fresh();
    assert_eq!(ctx.vault_state_data().deployed_aum, 0);
    let err = ctx
        .operator_update_aum(1)
        .expect_err("nonzero report on zero deployed AUM must fail");
    assert_anchor_err(&err, ErrorCode::AumIncreaseTooBig);
}

#[test]
fn update_aum_rejects_non_operator() {
    let mut ctx = vault_with_deployed_aum();
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let err = ctx
        .operator_update_aum_as(&impostor, DEPLOYED)
        .expect_err("non-operator must not report AUM");
    assert_anchor_err(&err, ErrorCode::NotOperator);
}

/// The admin is a distinct role: holding it must not grant operator powers.
#[test]
fn update_aum_rejects_admin_signer() {
    let mut ctx = vault_with_deployed_aum();
    let admin = ctx.admin.insecure_clone();
    let err = ctx
        .operator_update_aum_as(&admin, DEPLOYED)
        .expect_err("admin must not report AUM");
    assert_anchor_err(&err, ErrorCode::NotOperator);
}

// ---- operator_withdraw: wrong signer + zero amount ----

#[test]
fn operator_withdraw_rejects_non_operator() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");

    let impostor = ctx.new_funded_keypair(1_000_000_000);
    // Give the impostor a real ATA so the associated-token constraint
    // resolves and the access-control constraint is what fires.
    let impostor_ata = ctx.create_deposit_ata_for(&impostor.pubkey());

    let before = ctx.snapshot();
    let err = ctx
        .operator_withdraw_as(&impostor, impostor_ata, 1)
        .expect_err("non-operator must not withdraw vault funds");
    assert_anchor_err(&err, ErrorCode::NotOperator);
    assert_eq!(
        ctx.snapshot(),
        before,
        "rejected withdraw must not move funds"
    );
}

#[test]
fn operator_withdraw_rejects_zero_amount() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");

    let before = ctx.snapshot();
    let err = ctx
        .operator_withdraw(0)
        .expect_err("zero-value operator withdraw must be rejected");
    assert_anchor_err(&err, ErrorCode::ZeroAmount);
    assert_eq!(ctx.snapshot(), before);
}

// ---- operator_deposit: wrong signer + zero amount ----

#[test]
fn operator_deposit_rejects_non_operator() {
    let mut ctx = vault_with_deployed_aum();
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let impostor_ata = ctx.create_deposit_ata_for(&impostor.pubkey());

    let before = ctx.snapshot();
    let err = ctx
        .operator_deposit_as(&impostor, impostor_ata, 1)
        .expect_err("non-operator must not run operator_deposit");
    assert_anchor_err(&err, ErrorCode::NotOperator);
    assert_eq!(ctx.snapshot(), before);
}

#[test]
fn operator_deposit_rejects_zero_amount() {
    let mut ctx = vault_with_deployed_aum();
    let before = ctx.snapshot();
    let err = ctx
        .operator_deposit(0)
        .expect_err("zero-value operator deposit must be rejected");
    assert_anchor_err(&err, ErrorCode::ZeroAmount);
    assert_eq!(ctx.snapshot(), before);
}
