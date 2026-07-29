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

// ---- large-magnitude AUM: the guard scales both sides by 10 000 ----
//
// Scaling in `u64` overflows once `deployed_aum` passes
// `u64::MAX / (10000 + increase_limit)` — 1,840,992,422,525,903 at the default
// 20 bps, and only 922,337,203,685,477 if an admin widens the limits to their
// 10000 bps maximum (looser limits *lower* the ceiling, which is the opposite of
// the intuition). Past that point every legitimate report aborted, freezing yield
// reporting. `operator_update_aum` widens to `u128` first.
//
// These magnitudes are unreachable through deposits in a test, so the state is
// engineered directly. `local_aum` is left at 0 so nothing else overflows.

/// Above the old u64 ceiling, both `new_aum * 10000` and
/// `increase_limit * deployed_aum` overflowed. A legitimate in-window report at
/// that scale must now succeed.
#[test]
fn update_aum_accepts_in_window_report_past_the_u64_scaling_ceiling() {
    const HUGE: u64 = 2_000_000_000_000_000; // > u64::MAX / 10_020

    let mut ctx = VaultCtx::fresh();
    let mut state = ctx.vault_state_data();
    state.local_aum = 0;
    state.deployed_aum = HUGE;
    ctx.force_overwrite_vault_state(state);

    let boundary = max_accepted_aum(HUGE, DEFAULT_LIMIT_BPS);
    ctx.operator_update_aum(boundary)
        .expect("an in-window report must not abort merely because of magnitude");
    assert_eq!(ctx.vault_state_data().deployed_aum, boundary);
}

/// Not crashing is not enough — the guard must still *reject* out-of-window
/// reports at these magnitudes rather than let anything through.
#[test]
fn update_aum_still_enforces_the_window_past_the_u64_scaling_ceiling() {
    const HUGE: u64 = 2_000_000_000_000_000;

    let mut ctx = VaultCtx::fresh();
    let mut state = ctx.vault_state_data();
    state.local_aum = 0;
    state.deployed_aum = HUGE;
    ctx.force_overwrite_vault_state(state);

    let err = ctx
        .operator_update_aum(max_accepted_aum(HUGE, DEFAULT_LIMIT_BPS) + 1)
        .expect_err("one unit above the window must still be rejected");
    assert_anchor_err(&err, ErrorCode::AumIncreaseTooBig);

    let err = ctx
        .operator_update_aum(min_accepted_aum(HUGE, DEFAULT_LIMIT_BPS) - 1)
        .expect_err("one unit below the window must still be rejected");
    assert_anchor_err(&err, ErrorCode::AumDecreaseTooBig);
    assert_eq!(ctx.vault_state_data().deployed_aum, HUGE);
}

/// Widening the limits to the 10000 bps maximum doubles the multiplier and so
/// halves the old u64 ceiling — the amplification the audit report did not note.
#[test]
fn update_aum_handles_max_limits_at_large_magnitude() {
    const HUGE: u64 = 2_000_000_000_000_000; // > u64::MAX / 20_000 by ~2.2x
    const MAX_BPS: u32 = 10_000;

    let mut ctx = VaultCtx::fresh();
    ctx.set_aum_limits(MAX_BPS, MAX_BPS)
        .expect("admin widens the window to ±100%");
    let mut state = ctx.vault_state_data();
    state.local_aum = 0;
    state.deployed_aum = HUGE;
    ctx.force_overwrite_vault_state(state);

    // +100% is the edge of the widened window: 2x the current value.
    let doubled = max_accepted_aum(HUGE, MAX_BPS);
    ctx.operator_update_aum(doubled)
        .expect("+100% is in window once the limits allow it");
    assert_eq!(ctx.vault_state_data().deployed_aum, doubled);

    let err = ctx
        .operator_update_aum(max_accepted_aum(doubled, MAX_BPS) + 1)
        .expect_err("beyond +100% must still be rejected");
    assert_anchor_err(&err, ErrorCode::AumIncreaseTooBig);
}

/// The extreme: `deployed_aum` at `u64::MAX`. Scaling this by 10 000 needs 78
/// bits, so it is only expressible in `u128`.
#[test]
fn update_aum_handles_deployed_aum_at_u64_max() {
    let mut ctx = VaultCtx::fresh();
    let mut state = ctx.vault_state_data();
    state.local_aum = 0;
    state.deployed_aum = u64::MAX;
    ctx.force_overwrite_vault_state(state);

    // No increase is representable, but holding steady must be accepted, and a
    // decrease to the window's floor must be too.
    ctx.operator_update_aum(u64::MAX)
        .expect("reporting the same value must be in window");

    let floor = min_accepted_aum(u64::MAX, DEFAULT_LIMIT_BPS);
    ctx.operator_update_aum(floor)
        .expect("the decrease floor must be accepted at u64::MAX");
    assert_eq!(ctx.vault_state_data().deployed_aum, floor);
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
