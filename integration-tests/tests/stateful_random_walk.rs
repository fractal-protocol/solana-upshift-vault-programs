//! Stateful property tests: random sequences of user deposits/redeems,
//! operator withdraw/return/AUM reports, and admin fee changes are executed
//! against a real LiteSVM vault, checking accounting invariants after every
//! step. This addresses the due-diligence recommendation for property testing
//! over "AUM and fee arithmetic, and stateful sequences of deposits, operator
//! actions, and redemptions".
//!
//! Invariants checked after every operation:
//! - `local_aum` equals the vault token account's actual balance (no
//!   accounting drift, with or without fees).
//! - Share-mint supply equals the single user's share balance (nothing else
//!   mints or burns).
//! - Any failed operation leaves every observable state slot unchanged
//!   (transaction-level atomicity backing the CEI audit finding).
//! - Every successful redeem conserves value exactly: the vault pays out
//!   `assets`, the fee recipient receives `ceil(assets * fee / 1e6)`, and the
//!   user receives the remainder.
//!
//! A second walk (no AUM reports, operator returns everything at the end)
//! checks the economic end-state property: with no yield injected, the user
//! can never withdraw more than they deposited.
//!
//! Case count is deliberately modest (each case boots a fresh SVM); override
//! with PROPTEST_CASES for a deeper local search.

use august_vault::state::vault::FEE_RATE_DENOMINATOR_VALUE;
use integration_tests::harness::{CeiSnapshot, VaultCtx};
use proptest::prelude::*;

/// Initial balance minted to the user; caps total value in play so the
/// `new_aum * 10000` guard math stays far from u64 overflow.
const INITIAL_USER_FUNDS: u64 = 1_000_000_000_000; // 1000 tokens at 9 decimals
/// Seed deposit so walks start from a live vault (past the first-deposit floor).
const SEED_DEPOSIT: u64 = 1_000_000_000;

#[derive(Debug, Clone)]
enum Op {
    /// User deposits a raw amount (may exceed their balance → must fail clean).
    UserDeposit(u64),
    /// User redeems this many per-mille of their current share balance.
    UserRedeem(u16),
    /// Operator withdraws this many per-mille of current `local_aum`.
    OperatorWithdraw(u16),
    /// Operator returns this many per-mille of its current token balance.
    OperatorReturn(u16),
    /// Operator reports AUM this many basis points away from the current
    /// `deployed_aum` (kept within the default ±20 bps window).
    UpdateAum(i8),
    /// Admin sets the withdrawal fee (always below the 10% cap).
    SetFee(u32),
}

fn op_strategy(include_yield_ops: bool) -> BoxedStrategy<Op> {
    let base = prop_oneof![
        (1u64..20_000_000_000).prop_map(Op::UserDeposit),
        (0u16..=1000).prop_map(Op::UserRedeem),
        (0u16..=1000).prop_map(Op::OperatorWithdraw),
        (0u16..=1000).prop_map(Op::OperatorReturn),
        (0u32..FEE_RATE_DENOMINATOR_VALUE / 10).prop_map(Op::SetFee),
    ];
    if include_yield_ops {
        prop_oneof![base, (-20i8..=20).prop_map(Op::UpdateAum)].boxed()
    } else {
        base.boxed()
    }
}

fn per_mille(value: u64, pm: u16) -> u64 {
    ((value as u128) * (pm as u128) / 1000) as u64
}

/// Mirror of the private `Redeem::ceil_div` fee rounding.
fn expected_fee(assets: u64, fee_rate: u32) -> u64 {
    let num = (assets as u128) * (fee_rate as u128);
    num.div_ceil(FEE_RATE_DENOMINATOR_VALUE as u128) as u64
}

fn fresh_walk_vault() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(INITIAL_USER_FUNDS);
    ctx.deposit(SEED_DEPOSIT).expect("seed deposit");
    ctx
}

/// Execute one op. Returns the pre-op snapshot, the stored fee rate at
/// execution time, and whether the op succeeded.
fn execute(ctx: &mut VaultCtx, op: &Op) -> (CeiSnapshot, u32, bool, bool) {
    // Identical op payloads repeat within a walk; rotate the blockhash so
    // LiteSVM's duplicate-transaction check never masks a real execution.
    ctx.advance_blockhash();

    let before = ctx.snapshot();
    let fee_rate = ctx.vault_state_data().withdrawal_fee;

    let (ok, is_redeem) = match *op {
        Op::UserDeposit(amount) => (ctx.deposit(amount).is_ok(), false),
        Op::UserRedeem(pm) => {
            let shares = per_mille(ctx.token_account_amount(&ctx.user_share_ata), pm);
            (ctx.redeem(shares).is_ok(), true)
        }
        Op::OperatorWithdraw(pm) => {
            let amount = per_mille(ctx.vault_state_data().local_aum, pm);
            (ctx.operator_withdraw(amount).is_ok(), false)
        }
        Op::OperatorReturn(pm) => {
            let amount = per_mille(ctx.token_account_amount(&ctx.operator_deposit_ata), pm);
            (ctx.operator_deposit(amount).is_ok(), false)
        }
        Op::UpdateAum(bps) => {
            let deployed = ctx.vault_state_data().deployed_aum;
            let delta = (deployed as u128) * (bps.unsigned_abs() as u128) / 10_000;
            // Rounding the |delta| down keeps the report strictly inside the
            // ±window, so a rejection here would be a genuine guard bug.
            let new_aum = if bps >= 0 {
                deployed + delta as u64
            } else {
                deployed - delta as u64
            };
            (ctx.operator_update_aum(new_aum).is_ok(), false)
        }
        Op::SetFee(fee) => (ctx.set_withdrawal_fee(fee).is_ok(), false),
    };
    (before, fee_rate, ok, is_redeem)
}

/// Accounting invariants that must hold in every reachable state.
fn assert_state_invariants(ctx: &VaultCtx) -> Result<(), TestCaseError> {
    let state = ctx.vault_state_data();
    prop_assert_eq!(
        state.local_aum,
        ctx.token_account_amount(&ctx.vault_token_pda),
        "local_aum drifted from the vault token account balance"
    );
    prop_assert_eq!(
        ctx.share_mint_supply(),
        ctx.token_account_amount(&ctx.user_share_ata),
        "share supply drifted from the sole user's balance"
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Full op mix (including in-window AUM reports): accounting invariants
    /// hold after every step; failures are perfectly atomic; every successful
    /// redeem conserves value with exact ceil-rounded fees.
    #[test]
    fn invariants_hold_under_random_op_sequences(
        ops in proptest::collection::vec(op_strategy(true), 1..25)
    ) {
        let mut ctx = fresh_walk_vault();
        assert_state_invariants(&ctx)?;

        for op in &ops {
            let (before, fee_rate, ok, is_redeem) = execute(&mut ctx, op);

            if !ok {
                prop_assert_eq!(
                    ctx.snapshot(), before,
                    "failed {:?} left side effects", op
                );
                continue;
            }

            assert_state_invariants(&ctx)?;

            if is_redeem {
                let after = ctx.snapshot();
                let assets = before.vault_tokens - after.vault_tokens;
                let fee_paid = after.fee_recipient_tokens - before.fee_recipient_tokens;
                let user_received = after.user_deposit - before.user_deposit;
                prop_assert_eq!(
                    assets, fee_paid + user_received,
                    "redeem leaked value: paid {} but recipients got {}",
                    assets, fee_paid + user_received
                );
                prop_assert_eq!(
                    fee_paid, expected_fee(assets, fee_rate),
                    "fee not ceil(assets * fee / 1e6) for assets={}, rate={}",
                    assets, fee_rate
                );
            }
        }
    }

    /// No-yield walks (AUM reports excluded): after the operator returns its
    /// full balance and the user exits completely, the user cannot hold more
    /// of the deposit token than they started with — rounding and fees only
    /// ever favour the vault.
    #[test]
    fn no_value_extraction_without_yield(
        ops in proptest::collection::vec(op_strategy(false), 1..25)
    ) {
        let mut ctx = fresh_walk_vault();

        for op in &ops {
            execute(&mut ctx, op); // failures are fine; atomicity checked above
        }

        // Unwind: operator returns everything it took…
        let operator_balance = ctx.token_account_amount(&ctx.operator_deposit_ata);
        if operator_balance > 0 {
            ctx.advance_blockhash();
            ctx.operator_deposit(operator_balance).expect("operator returns all");
        }
        // …and the user exits their entire position.
        let shares = ctx.token_account_amount(&ctx.user_share_ata);
        if shares > 0 {
            ctx.advance_blockhash();
            ctx.redeem(shares).expect("full exit after operator returned all");
        }

        prop_assert!(
            ctx.token_account_amount(&ctx.user_deposit_ata) <= INITIAL_USER_FUNDS,
            "user extracted value from a yield-free vault"
        );
    }
}
