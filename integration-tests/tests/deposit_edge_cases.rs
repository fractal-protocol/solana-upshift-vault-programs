//! Deposit edge cases the July 2026 due-diligence review listed as absent
//! from the passing suite: the zero-amount rejection, the minimum-first-
//! deposit floor (`InsufficientAmount`), truncation/rounding-to-zero, and
//! the `NumberOverflow` path when `local_aum + amount` exceeds `u64::MAX`.
//!
//! States unreachable through legal calls (extreme share supply / recorded
//! AUM) are engineered via the harness force-overwrite helpers, mirroring
//! `overflow_propagation.rs`.

use august_vault::{errors::ErrorCode, state::vault::VaultState};
use integration_tests::harness::{assert_anchor_err, VaultCtx, DEPOSIT_DECIMALS};

/// The program's own first-deposit floor for the harness mint, so these
/// boundary tests track the formula instead of re-deriving it. A fn rather than
/// a const because `min_first_deposit` is not `const`.
fn min_first_deposit() -> u64 {
    VaultState::min_first_deposit(DEPOSIT_DECIMALS)
}

// ---- zero amount ----

#[test]
fn deposit_zero_amount_is_rejected_with_no_side_effects() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(min_first_deposit());
    let before = ctx.snapshot();

    let err = ctx.deposit(0).expect_err("zero deposit must be rejected");
    assert_anchor_err(&err, ErrorCode::ZeroAmount);
    assert_eq!(ctx.snapshot(), before, "CEI violated on ZeroAmount deposit");
}

// ---- minimum first deposit ----

#[test]
fn first_deposit_below_minimum_is_rejected() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(min_first_deposit());
    let before = ctx.snapshot();

    let err = ctx
        .deposit(min_first_deposit() - 1)
        .expect_err("first deposit below the floor must be rejected");
    assert_anchor_err(&err, ErrorCode::InsufficientAmount);
    assert_eq!(ctx.snapshot(), before, "CEI violated on InsufficientAmount");
}

#[test]
fn first_deposit_at_exact_minimum_succeeds() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(min_first_deposit());
    ctx.deposit(min_first_deposit())
        .expect("the floor itself must be accepted");
    assert_eq!(ctx.share_mint_supply(), min_first_deposit());
}

/// The floor applies to the *first* deposit only: once supply is nonzero a
/// single-unit deposit is legal (and still mints ≥1 share at a 1:1 price).
#[test]
fn subsequent_deposit_below_minimum_is_allowed() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(min_first_deposit() + 1);
    ctx.deposit(min_first_deposit()).expect("first deposit");
    ctx.deposit(1)
        .expect("post-first deposits are not subject to the floor");
    assert_eq!(ctx.share_mint_supply(), min_first_deposit() + 1);
}

// ---- truncation: share math rounds to zero ----

#[test]
fn deposit_rounding_to_zero_shares_is_rejected_with_no_side_effects() {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(min_first_deposit() + 1);
    ctx.deposit(min_first_deposit()).expect("seed deposit");

    // Inflate the share price ~10^6× by faking externally deployed assets:
    // shares = 1 * (supply + EXTRA_SHARES) / (total_assets + VIRTUAL_ASSETS),
    // and the pro-rata floor, both truncate to 0.
    let mut state = ctx.vault_state_data();
    state.deployed_aum = 1_000_000_000_000;
    ctx.force_overwrite_vault_state(state);

    let supply = ctx.share_mint_supply();
    let total = ctx.vault_state_data().total_assets().unwrap();
    assert_eq!(
        VaultState::shares_for_deposit(supply, total, 1).unwrap(),
        0,
        "fixture must actually sit in the truncation regime"
    );

    let before = ctx.snapshot();
    let err = ctx
        .deposit(1)
        .expect_err("a deposit worth zero shares must be rejected");
    assert_anchor_err(&err, ErrorCode::ZeroAmount);

    // The handler transfers tokens in BEFORE the shares>0 check; the runtime
    // rollback must return them (same caveat as overflow_propagation.rs).
    assert_eq!(ctx.snapshot(), before, "CEI violated on zero-share deposit");
}

// ---- NumberOverflow: local_aum + amount exceeds u64::MAX ----

#[test]
fn deposit_overflowing_local_aum_fails_with_number_overflow() {
    const AMOUNT: u64 = 100;

    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(AMOUNT);

    // Engineer: shares > 0 must survive the truncation check while
    // `local_aum + AMOUNT` overflows. With supply ≈ total_assets the share
    // price stays ≈1, so a 100-unit deposit mints ≥1 share; local_aum sits
    // 50 units below u64::MAX so the checked_add is what fires.
    ctx.force_overwrite_share_mint_supply(u64::MAX / 100);
    let mut state = ctx.vault_state_data();
    state.local_aum = u64::MAX - 50;
    state.deployed_aum = 0; // keep total_assets() itself from overflowing
    ctx.force_overwrite_vault_state(state);

    let supply = ctx.share_mint_supply();
    let total = ctx.vault_state_data().total_assets().unwrap();
    let shares = VaultState::shares_for_deposit(supply, total, AMOUNT).unwrap();
    assert!(shares > 0, "fixture must pass the zero-share check");

    let before = ctx.snapshot();
    let err = ctx
        .deposit(AMOUNT)
        .expect_err("local_aum accumulation past u64::MAX must fail");
    assert_anchor_err(&err, ErrorCode::NumberOverflow);
    assert_eq!(ctx.snapshot(), before, "CEI violated on NumberOverflow");
}
