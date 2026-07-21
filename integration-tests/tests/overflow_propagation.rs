//! Proves that the `checked_add` introduced in
//! `state::vault::VaultState::total_assets` actually propagates `MathError`
//! out through the deposit/redeem handlers — and that no state mutation
//! leaks when it does.
//!
//! The state needed (`deployed_aum` ≈ `u64::MAX`) is unreachable through
//! legal calls, so the harness uses `LiteSVM::set_account` to engineer it
//! directly.

use august_vault::errors::ErrorCode;
use integration_tests::harness::{assert_anchor_err, VaultCtx, DEPOSIT_DECIMALS};

const SEED_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);

/// Force the vault into a state where `local_aum + deployed_aum` overflows
/// `u64` on the next `total_assets()` call.
fn vault_at_overflow_boundary(local_aum: u64) -> VaultCtx {
    let mut ctx = VaultCtx::fresh();

    // Seed a normal deposit so `share_mint.supply > 0` and the vault is "live".
    ctx.mint_to_user(SEED_AMOUNT);
    ctx.deposit(SEED_AMOUNT).expect("seed deposit");

    let mut state = ctx.vault_state_data();
    state.local_aum = local_aum;
    state.deployed_aum = u64::MAX - 1; // Any new add ≥ 2 overflows.
    ctx.force_overwrite_vault_state(state);
    ctx
}

#[test]
fn deposit_fails_with_math_error_when_total_assets_would_overflow() {
    let mut ctx = vault_at_overflow_boundary(2);
    ctx.mint_to_user(10);
    let before = ctx.snapshot();

    let err = ctx
        .deposit(10)
        .expect_err("deposit must fail at the boundary");
    assert_anchor_err(&err, ErrorCode::MathError);

    // NOTE: `deposit` runs `transfer_in_ctx` BEFORE `total_assets()?`, so
    // tokens DO move from the user to the vault PDA before the math error
    // reverts the tx. The Solana runtime rolls back the entire transaction on
    // revert, so observable state is identical to before — but the program
    // is not internally CEI-clean on this path: a Token-2022 transfer hook
    // would have already executed CPI side effects by the time `MathError`
    // fires. The fix lives in the handler (move math before transfer); this
    // test only asserts the post-revert observable invariants the runtime
    // gives us.
    let after = ctx.snapshot();
    assert_eq!(after.local_aum, before.local_aum, "local_aum changed");
    assert_eq!(
        after.deployed_aum, before.deployed_aum,
        "deployed_aum changed"
    );
    assert_eq!(after.share_supply, before.share_supply, "shares minted");
    assert_eq!(after.user_deposit, before.user_deposit, "user tokens moved");
    assert_eq!(
        after.vault_tokens, before.vault_tokens,
        "vault tokens moved"
    );
}

#[test]
fn redeem_fails_with_math_error_when_total_assets_would_overflow() {
    let mut ctx = vault_at_overflow_boundary(2);
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert!(shares > 0, "fixture should leave shares to redeem");

    let before = ctx.snapshot();
    let err = ctx
        .redeem(shares)
        .expect_err("redeem must fail at the boundary");
    assert_anchor_err(&err, ErrorCode::MathError);

    // CEI: no shares burned, no fees moved, no AUM mutation on the math path.
    assert_eq!(
        ctx.snapshot(),
        before,
        "CEI violated on MathError redeem path"
    );
}
