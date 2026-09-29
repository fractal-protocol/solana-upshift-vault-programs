//! Stateful property tests: random sequences of deposits/redeems (plain and
//! slippage-checked) by several depositors, operator withdraw/return/AUM
//! reports, subaccount custody (register, withdraw to, return from, custodian
//! loss, settle, deregister), and admin fee, limit, pause and handover changes
//! are executed against a real LiteSVM vault, checking accounting invariants
//! after every step. This addresses the due-diligence recommendation for
//! property testing over "AUM and fee arithmetic, and stateful sequences of
//! deposits, operator actions, and redemptions".
//!
//! Invariants checked after every operation:
//! - An op whose outcome the walk's model can decide (a paused vault, an AUM
//!   report against the current limits, a slippage bound, a subaccount rule, a
//!   nomination and its expiry) succeeds exactly when the model says.
//! - `local_aum` equals the vault token account's actual balance (no
//!   accounting drift, with or without fees).
//! - Share-mint supply equals the depositors' share balances combined (nothing
//!   else mints or burns).
//! - While any subaccount is registered, `deployed_principal` equals their
//!   principals combined, each as the model tracked it.
//! - Any failed operation leaves every observable state slot unchanged
//!   (transaction-level atomicity backing the CEI audit finding).
//! - A successful operation moves no balance of a depositor it does not
//!   involve.
//! - Every successful redeem conserves value exactly: the vault pays out
//!   `assets`, the fee recipient receives `ceil(assets * fee / 1e6)`, and the
//!   redeemer receives the remainder.
//! - Every successful deposit mints, and every redeem pays, exactly what the
//!   vault's conversion math quotes on the state before it.
//! - In the queue walk, an instruction (a holder's, the queue's, the
//!   operator's or the admin's) forged with one account the program must bind
//!   swapped for a decoy (another vault's, another holder's, the wrong kind)
//!   is refused and changes nothing.
//!
//! A second walk (no AUM reports or custody losses; everything deployed comes
//! home at the end) checks the economic end-state property: with no yield
//! injected, the depositors together can never withdraw more than they
//! deposited. Not each one alone: a redeem's rounding dust stays in the vault
//! and lifts the price for whoever remains. It then checks `close_vault` is
//! refused while any share is out and succeeds once none is.
//!
//! Case count is deliberately modest (each case boots a fresh SVM); override
//! with PROPTEST_CASES for a deeper local search. `nightly-property.yml` does.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use anchor_lang::{InstructionData, ToAccountMetas};

use august_vault::errors::ErrorCode as VaultError;
use august_vault::state::vault::{BPS_DENOMINATOR, FEE_RATE_DENOMINATOR_VALUE};
use august_withdrawal_queue::errors::{ErrorCode as QueueError, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::state::WITHDRAWAL_REQUEST_SEED;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, event_authority_pda, expected_withdrawal_fee,
    program_config_pda, CeiSnapshot, Depositor, OtherVault, Subaccount, VaultCtx,
};
use proptest::prelude::*;
use proptest::test_runner::TestRunner;
use solana_sdk::{instruction::Instruction, pubkey::Pubkey, signature::Keypair, signer::Signer};

/// Initial balance minted to each depositor; caps total value in play so the
/// `new_aum * 10000` guard math stays far from u64 overflow.
const INITIAL_USER_FUNDS: u64 = 1_000_000_000_000; // 1000 tokens at 9 decimals
/// Seed deposit so walks start from a live vault (past the first-deposit floor).
const SEED_DEPOSIT: u64 = 1_000_000_000;
/// Depositors per walk. Three, so an op always has an actor, a counterparty
/// and a bystander.
const DEPOSITORS: usize = 3;

#[derive(Debug, Clone)]
enum Op {
    /// Depositor `who` deposits a raw amount (may exceed their balance → must
    /// fail clean).
    UserDeposit { who: usize, amount: u64 },
    /// Depositor `who` redeems this many per-mille of their share balance.
    UserRedeem { who: usize, pm: u16 },
    /// `deposit_checked` with `min_shares_out` set `slack` away from the quote.
    DepositChecked { who: usize, amount: u64, slack: i8 },
    /// `redeem_checked` with `min_assets_out` set `slack` away from the quoted
    /// net payout.
    RedeemChecked { who: usize, pm: u16, slack: i8 },
    /// Operator withdraws this many per-mille of current `local_aum` to its
    /// own account, which the vault refuses once a subaccount is registered.
    OperatorWithdraw(u16),
    /// Operator returns this many per-mille of its own token balance.
    OperatorReturn(u16),
    /// Operator reports AUM this many basis points away from the current
    /// `deployed_aum`, inside or outside the current limits.
    UpdateAum(i8),
    /// Admin sets the withdrawal fee (always below the 10% cap).
    SetFee(u32),
    /// Admin pauses a running vault or unpauses a paused one.
    TogglePause,
    /// Admin sets the AUM report limits in basis points; over 100% is refused.
    SetAumLimits { increase: u32, decrease: u32 },
    /// Admin registers a fresh custody subaccount, while fewer than
    /// [`MAX_SUBACCOUNTS`] are registered.
    RegisterSub,
    /// Operator withdraws this many per-mille of `local_aum` to subaccount
    /// `sub` (modulo the number registered).
    SubWithdraw { sub: usize, pm: u16 },
    /// Operator returns this many per-mille of subaccount `sub`'s balance.
    SubReturn { sub: usize, pm: u16 },
    /// Subaccount `sub`'s custodian loses this many per-mille of its balance.
    SubLose { sub: usize, pm: u16 },
    /// Admin settles this many per-mille of subaccount `sub`'s shortfall; over
    /// 1000 asks for more than the shortfall and is refused.
    SettleLoss { sub: usize, pm: u16 },
    /// Admin deregisters subaccount `sub`, refused while it carries principal.
    Deregister { sub: usize },
    /// Admin nominates admin candidate `who`.
    Nominate { who: usize },
    /// Admin candidate `who` accepts a nomination.
    Accept { who: usize },
}

/// Subaccounts registered at once. Two, so one can inherit the principal and
/// the other start empty.
const MAX_SUBACCOUNTS: usize = 2;
/// Admin candidates besides the sitting admin.
const CANDIDATES: usize = 2;
/// How long a nomination stays acceptable (`NominatedAdmin::initialize`).
const NOMINATION_WINDOW: i64 = 24 * 60 * 60;
/// What a subaccount lets the vault move; far above anything a walk deploys.
const SUBACCOUNT_ALLOWANCE: u64 = u64::MAX / 4;

fn op_strategy(include_yield_ops: bool) -> BoxedStrategy<Op> {
    let slack = prop_oneof![Just(-1i8), Just(0i8), Just(1i8)];
    let base = prop_oneof![
        4 => (0..DEPOSITORS, 1u64..20_000_000_000)
            .prop_map(|(who, amount)| Op::UserDeposit { who, amount }),
        4 => (0..DEPOSITORS, 0u16..=1000).prop_map(|(who, pm)| Op::UserRedeem { who, pm }),
        1 => (0..DEPOSITORS, 1u64..20_000_000_000, slack.clone())
            .prop_map(|(who, amount, slack)| Op::DepositChecked { who, amount, slack }),
        1 => (0..DEPOSITORS, 0u16..=1000, slack)
            .prop_map(|(who, pm, slack)| Op::RedeemChecked { who, pm, slack }),
        2 => (0u16..=1000).prop_map(Op::OperatorWithdraw),
        2 => (0u16..=1000).prop_map(Op::OperatorReturn),
        1 => (0u32..FEE_RATE_DENOMINATOR_VALUE / 10).prop_map(Op::SetFee),
        1 => Just(Op::TogglePause),
        1 => (
            prop_oneof![0u32..=100, 0u32..=BPS_DENOMINATOR, BPS_DENOMINATOR..=BPS_DENOMINATOR + 50],
            prop_oneof![0u32..=100, 0u32..=BPS_DENOMINATOR, BPS_DENOMINATOR..=BPS_DENOMINATOR + 50],
        )
            .prop_map(|(increase, decrease)| Op::SetAumLimits { increase, decrease }),
        1 => Just(Op::RegisterSub),
        2 => (any::<usize>(), 0u16..=1000).prop_map(|(sub, pm)| Op::SubWithdraw { sub, pm }),
        2 => (any::<usize>(), 0u16..=1000).prop_map(|(sub, pm)| Op::SubReturn { sub, pm }),
        // Half the settlements ask for more than the shortfall, up to twice it.
        1 => (any::<usize>(), prop_oneof![0u16..=1000, 1001u16..=2000])
            .prop_map(|(sub, pm)| Op::SettleLoss { sub, pm }),
        1 => any::<usize>().prop_map(|sub| Op::Deregister { sub }),
        1 => (0..CANDIDATES).prop_map(|who| Op::Nominate { who }),
        1 => (0..CANDIDATES).prop_map(|who| Op::Accept { who }),
    ];
    if include_yield_ops {
        // AUM reports and custody losses move the vault's value outside its
        // own accounts, so the no-yield walks leave both out. `prop_oneof!`
        // weights are per-arm at the level they appear, so `base` is weighted
        // by its own total to keep each about as likely as an operator
        // withdrawal.
        prop_oneof![
            29 => base,
            2 => (-100i8..=100).prop_map(Op::UpdateAum),
            2 => (any::<usize>(), 0u16..=1000).prop_map(|(sub, pm)| Op::SubLose { sub, pm }),
        ]
        .boxed()
    } else {
        base.boxed()
    }
}

fn per_mille(value: u64, pm: u16) -> u64 {
    ((value as u128) * (pm as u128) / 1000) as u64
}

/// A balance that must have fallen between two snapshots. Fails the case
/// instead of wrapping when it moved the wrong way, so a reversed-transfer
/// regression can never satisfy the conservation equality by underflow.
fn decrease(before: u64, after: u64, what: &str) -> Result<u64, TestCaseError> {
    before.checked_sub(after).ok_or_else(|| {
        TestCaseError::fail(format!(
            "{what} rose from {before} to {after}, expected a fall"
        ))
    })
}

/// Companion to [`decrease`] for a balance that must have risen.
fn increase(before: u64, after: u64, what: &str) -> Result<u64, TestCaseError> {
    after.checked_sub(before).ok_or_else(|| {
        TestCaseError::fail(format!(
            "{what} fell from {before} to {after}, expected a rise"
        ))
    })
}

/// What the walk tracks of the vault beside the harness: the registered
/// subaccounts with the principal each must carry, the admin candidates, and
/// the pending nomination.
struct VaultModel {
    subs: Vec<(Subaccount, u64)>,
    candidates: Vec<Keypair>,
    /// Nominee and the instant the nomination stops being acceptable.
    nomination: Option<(Pubkey, i64)>,
    /// Where a custodian's losses go: a token account nobody in the walk owns.
    sink: Pubkey,
}

/// A fresh vault and its depositors. The first is the harness's own user, who
/// makes the seed deposit; the rest start with the same funds and no shares.
fn fresh_walk_vault() -> (VaultCtx, Vec<Depositor>, VaultModel) {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(INITIAL_USER_FUNDS);
    ctx.deposit(SEED_DEPOSIT).expect("seed deposit");
    let mut users = vec![Depositor {
        keypair: ctx.user.insecure_clone(),
        deposit_ata: ctx.user_deposit_ata,
        share_ata: ctx.user_share_ata,
    }];
    for _ in 1..DEPOSITORS {
        users.push(ctx.new_depositor(INITIAL_USER_FUNDS));
    }
    let candidates = (0..CANDIDATES)
        .map(|_| ctx.new_funded_keypair(1_000_000_000))
        .collect();
    let sink_owner = Keypair::new().pubkey();
    let deposit_mint = ctx.deposit_mint;
    let sink = ctx.create_ata_for(&sink_owner, &deposit_mint);
    let model = VaultModel {
        subs: Vec::new(),
        candidates,
        nomination: None,
        sink,
    };
    (ctx, users, model)
}

/// One depositor's deposit-token and share balances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Holdings {
    deposit: u64,
    shares: u64,
}

fn holdings(ctx: &VaultCtx, users: &[Depositor]) -> Vec<Holdings> {
    users
        .iter()
        .map(|u| Holdings {
            deposit: ctx.token_account_amount(&u.deposit_ata),
            shares: ctx.token_account_amount(&u.share_ata),
        })
        .collect()
}

/// Every depositor's holdings except `who`'s must be unchanged; `who` may be
/// `None` for an op that involves no depositor.
fn assert_bystanders_untouched(
    before: &[Holdings],
    after: &[Holdings],
    who: Option<usize>,
    what: &str,
) -> Result<(), TestCaseError> {
    for (i, (b, a)) in before.iter().zip(after).enumerate() {
        if Some(i) == who {
            continue;
        }
        prop_assert_eq!(b, a, "{} moved bystander {}'s balances", what, i);
    }
    Ok(())
}

/// What one executed op tells the invariant checks.
struct OpOutcome {
    /// Every observable state slot immediately before the op ran.
    before: CeiSnapshot,
    /// Every depositor's holdings immediately before the op ran.
    holdings: Vec<Holdings>,
    /// Withdrawal fee stored at execution time (a `SetFee` earlier in the walk
    /// may have moved it).
    fee_rate: u32,
    /// Whether the transaction succeeded.
    ok: bool,
    /// The depositor the op acts for, if any; nobody else's balances may move.
    actor: Option<usize>,
    /// Whether this op was a redeem, i.e. whether the value-conservation and
    /// fee-rounding checks apply.
    is_redeem: bool,
    /// The shares a deposit should mint, or the net a redeem should pay, by the
    /// vault's own conversion math on the state before the op. This checks the
    /// handler feeds the math the right inputs; `property_arithmetic.rs` checks
    /// the math. `None` for other ops, or when the math refuses.
    quote: Option<u64>,
}

/// Runs `op` on a copy of the chain and reports whether it succeeded, leaving
/// the real chain untouched.
fn succeeds_on_copy(ctx: &mut VaultCtx, op: impl FnOnce(&mut VaultCtx) -> bool) -> bool {
    let saved = ctx.svm.clone();
    let ok = op(ctx);
    ctx.svm = saved;
    ok
}

/// Execute one op against the live vault, and check it succeeded exactly when
/// the model says it must.
fn execute(
    ctx: &mut VaultCtx,
    users: &[Depositor],
    model: &mut VaultModel,
    op: &Op,
) -> Result<OpOutcome, TestCaseError> {
    let before = ctx.snapshot();
    let holdings_before = holdings(ctx, users);
    let state = ctx.vault_state_data();
    let fee_rate = state.withdrawal_fee;
    let supply = ctx.share_mint_supply();
    let total_assets = state.total_assets().expect("total assets");
    let now = ctx.now();
    let mut quote = None;
    // Whether the op must succeed, where the model decides it; `None` leaves
    // the outcome to the program and checks only its effects.
    let mut expected = None;
    let admin = ctx.admin.insecure_clone();
    let sub_count = model.subs.len();

    let (ok, actor, is_redeem) = match *op {
        Op::UserDeposit { who, amount } => {
            if let Ok(shares) = state.shares_for_deposit(supply, total_assets, amount) {
                quote = Some(shares);
            }
            if state.paused || amount > holdings_before[who].deposit {
                expected = Some(false);
            }
            (
                ctx.deposit_as(&users[who], amount).is_ok(),
                Some(who),
                false,
            )
        }
        Op::UserRedeem { who, pm } => {
            let shares = per_mille(holdings_before[who].shares, pm);
            if let Ok(gross) = state.assets_for_redeem(supply, total_assets, shares) {
                quote = Some(gross - expected_withdrawal_fee(gross, fee_rate));
            }
            if state.paused {
                expected = Some(false);
            }
            (ctx.redeem_as(&users[who], shares).is_ok(), Some(who), true)
        }
        Op::DepositChecked { who, amount, slack } => {
            if let Ok(shares) = state.shares_for_deposit(supply, total_assets, amount) {
                quote = Some(shares);
            }
            let min = (quote.unwrap_or(0) as i128 + slack as i128).max(0) as u64;
            // The bound is the only difference from a plain deposit, so the
            // plain one on a copy decides everything else.
            let plain = succeeds_on_copy(ctx, |c| c.deposit_as(&users[who], amount).is_ok());
            expected = Some(plain && quote.unwrap_or(0) >= min);
            let ok = ctx.deposit_checked_as(&users[who], amount, min).is_ok();
            (ok, Some(who), false)
        }
        Op::RedeemChecked { who, pm, slack } => {
            let shares = per_mille(holdings_before[who].shares, pm);
            if let Ok(gross) = state.assets_for_redeem(supply, total_assets, shares) {
                quote = Some(gross - expected_withdrawal_fee(gross, fee_rate));
            }
            let min = (quote.unwrap_or(0) as i128 + slack as i128).max(0) as u64;
            let plain = succeeds_on_copy(ctx, |c| c.redeem_as(&users[who], shares).is_ok());
            expected = Some(plain && quote.unwrap_or(0) >= min);
            let ok = ctx.redeem_checked_as(&users[who], shares, min).is_ok();
            (ok, Some(who), true)
        }
        Op::OperatorWithdraw(pm) => {
            let amount = per_mille(state.local_aum, pm);
            expected = Some(sub_count == 0 && amount > 0);
            (ctx.operator_withdraw(amount).is_ok(), None, false)
        }
        Op::OperatorReturn(pm) => {
            let amount = per_mille(ctx.token_account_amount(&ctx.operator_deposit_ata), pm);
            expected = Some(sub_count == 0 && amount > 0);
            (ctx.operator_deposit(amount).is_ok(), None, false)
        }
        Op::UpdateAum(bps) => {
            let deployed = state.deployed_aum;
            let delta = (deployed as u128) * (bps.unsigned_abs() as u128) / 10_000;
            let new_aum = if bps >= 0 {
                deployed + delta as u64
            } else {
                deployed - delta as u64
            };
            let bps_den = BPS_DENOMINATOR as u128;
            let scaled = new_aum as u128 * bps_den;
            let floor = (bps_den - state.aum_decrease_limit as u128) * deployed as u128;
            let ceiling = (bps_den + state.aum_increase_limit as u128) * deployed as u128;
            expected = Some(floor <= scaled && scaled <= ceiling);
            (ctx.operator_update_aum(new_aum).is_ok(), None, false)
        }
        Op::SetFee(fee) => {
            expected = Some(true);
            (ctx.set_withdrawal_fee(fee).is_ok(), None, false)
        }
        Op::TogglePause => {
            expected = Some(true);
            let ok = if state.paused {
                ctx.unpause().is_ok()
            } else {
                ctx.pause().is_ok()
            };
            (ok, None, false)
        }
        Op::SetAumLimits { increase, decrease } => {
            expected = Some(increase <= BPS_DENOMINATOR && decrease <= BPS_DENOMINATOR);
            let ok = ctx.set_aum_limits_as(&admin, increase, decrease).is_ok();
            (ok, None, false)
        }
        Op::RegisterSub => {
            if sub_count >= MAX_SUBACCOUNTS {
                return Ok(noop(before, holdings_before, fee_rate));
            }
            let sub = ctx.new_delegated_subaccount(SUBACCOUNT_ALLOWANCE);
            // The first subaccount inherits everything the operator has out.
            let inherited = if sub_count == 0 {
                state.deployed_principal
            } else {
                0
            };
            expected = Some(true);
            let ok = ctx.register_subaccount_as(&admin, &sub).is_ok();
            if ok {
                model.subs.push((sub, inherited));
            }
            (ok, None, false)
        }
        Op::SubWithdraw { sub, pm } | Op::SubReturn { sub, pm } | Op::SubLose { sub, pm } => {
            if sub_count == 0 {
                return Ok(noop(before, holdings_before, fee_rate));
            }
            let i = sub % sub_count;
            let custody = model.subs[i].0.deposit_ata;
            let ok = match *op {
                Op::SubWithdraw { .. } => {
                    let amount = per_mille(state.local_aum, pm);
                    expected = Some(amount > 0);
                    let ok = ctx.operator_withdraw_to(&model.subs[i].0, amount).is_ok();
                    if ok {
                        model.subs[i].1 += amount;
                    }
                    ok
                }
                Op::SubReturn { .. } => {
                    let amount = per_mille(ctx.token_account_amount(&custody), pm);
                    expected = Some(amount > 0);
                    let ok = ctx.operator_deposit_from(&model.subs[i].0, amount).is_ok();
                    if ok {
                        model.subs[i].1 -= amount.min(model.subs[i].1);
                    }
                    ok
                }
                _ => {
                    // The custodian moves funds away: not a vault instruction,
                    // just the loss `settle_subaccount_loss` exists for.
                    let amount = per_mille(ctx.token_account_amount(&custody), pm);
                    if amount > 0 {
                        let owner = model.subs[i].0.keypair.insecure_clone();
                        let sink = model.sink;
                        ctx.transfer_tokens_as(&owner, &custody, &sink, amount);
                    }
                    true
                }
            };
            (ok, None, false)
        }
        Op::SettleLoss { sub, pm } => {
            if sub_count == 0 {
                return Ok(noop(before, holdings_before, fee_rate));
            }
            let i = sub % sub_count;
            let balance = ctx.token_account_amount(&model.subs[i].0.deposit_ata);
            let shortfall = model.subs[i].1.saturating_sub(balance);
            let amount = per_mille(shortfall, pm);
            expected = Some(amount > 0 && amount <= shortfall);
            let ok = ctx
                .settle_subaccount_loss_as(&admin, &model.subs[i].0, amount)
                .is_ok();
            if ok {
                model.subs[i].1 -= amount;
            }
            (ok, None, false)
        }
        Op::Deregister { sub } => {
            if sub_count == 0 {
                return Ok(noop(before, holdings_before, fee_rate));
            }
            let i = sub % sub_count;
            expected = Some(model.subs[i].1 == 0);
            let ok = ctx
                .deregister_subaccount_as(&admin, &model.subs[i].0)
                .is_ok();
            if ok {
                model.subs.remove(i);
            }
            (ok, None, false)
        }
        Op::Nominate { who } => {
            let nominee = model.candidates[who].pubkey();
            expected = Some(true);
            let ok = ctx.nominate_admin_as(&admin, nominee).is_ok();
            if ok {
                model.nomination = Some((nominee, now + NOMINATION_WINDOW));
            }
            (ok, None, false)
        }
        Op::Accept { who } => {
            let candidate = model.candidates[who].insecure_clone();
            let valid = match model.nomination {
                Some((nominee, until)) => nominee == candidate.pubkey() && now < until,
                None => false,
            };
            expected = Some(valid);
            // On success the harness makes the candidate `ctx.admin`, so the
            // walk's admin ops follow the handover.
            let ok = ctx.accept_admin_nomination_as(&candidate).is_ok();
            if ok {
                prop_assert_eq!(ctx.vault_state_data().admin, candidate.pubkey());
                prop_assert_eq!(ctx.admin.pubkey(), candidate.pubkey());
                // Accepting closes the nomination, and the old admin, now a
                // candidate, has lost its powers.
                model.nomination = None;
                model.candidates[who] = admin.insecure_clone();
                prop_assert!(
                    ctx.set_withdrawal_fee_as(&admin, fee_rate).is_err(),
                    "the replaced admin still set the fee"
                );
            }
            (ok, None, false)
        }
    };
    if let Some(expected) = expected {
        prop_assert_eq!(ok, expected, "{:?} at {}", op, now);
    }
    Ok(OpOutcome {
        before,
        holdings: holdings_before,
        fee_rate,
        ok,
        actor,
        is_redeem,
        quote,
    })
}

/// The outcome of an op with nothing to act on: it ran nothing, so it
/// succeeded and moved nothing.
fn noop(before: CeiSnapshot, holdings: Vec<Holdings>, fee_rate: u32) -> OpOutcome {
    OpOutcome {
        before,
        holdings,
        fee_rate,
        ok: true,
        actor: None,
        is_redeem: false,
        quote: None,
    }
}

/// The subaccount registry matches the model, and while any subaccount is
/// registered `deployed_principal` is exactly their principals combined: the
/// first inherits it on registration, and every later movement changes a
/// subaccount's principal and the total together.
fn assert_model_invariants(ctx: &VaultCtx, model: &VaultModel) -> Result<(), TestCaseError> {
    let state = ctx.vault_state_data();
    prop_assert_eq!(state.subaccount_count, model.subs.len() as u64);
    let mut total: u64 = 0;
    for (sub, principal) in &model.subs {
        let recorded = ctx.subaccount_data(sub).principal;
        prop_assert_eq!(recorded, *principal, "subaccount {} principal", sub.key());
        total += recorded;
    }
    if !model.subs.is_empty() {
        prop_assert_eq!(
            state.deployed_principal,
            total,
            "deployed_principal is not the subaccounts' principal"
        );
    }
    Ok(())
}

/// Brings everything deployed home: each subaccount returns its balance, the
/// rest of its principal is settled as a loss and it is deregistered, and then
/// the operator, free of the subaccount rule, returns its own balance.
fn recall_deployed(ctx: &mut VaultCtx, model: &mut VaultModel) {
    let admin = ctx.admin.insecure_clone();
    for (sub, _) in std::mem::take(&mut model.subs) {
        let balance = ctx.token_account_amount(&sub.deposit_ata);
        if balance > 0 {
            ctx.operator_deposit_from(&sub, balance)
                .expect("subaccount returns all");
        }
        let principal = ctx.subaccount_data(&sub).principal;
        if principal > 0 {
            ctx.settle_subaccount_loss_as(&admin, &sub, principal)
                .expect("settle what did not come back");
        }
        ctx.deregister_subaccount_as(&admin, &sub)
            .expect("deregister");
    }
    let operator_balance = ctx.token_account_amount(&ctx.operator_deposit_ata);
    if operator_balance > 0 {
        ctx.operator_deposit(operator_balance)
            .expect("operator returns all");
    }
}

/// While shares are out the vault cannot close, even with its subaccounts gone
/// and its reserve empty. Tried on a copy of the chain with the reserve taken
/// out, so the supply is the one thing that can refuse it.
fn assert_close_refused_while_held(ctx: &mut VaultCtx) -> Result<(), TestCaseError> {
    let supply = ctx.share_mint_supply();
    if supply == 0 {
        return Ok(());
    }
    let saved = ctx.svm.clone();
    let reserve = ctx.token_account_amount(&ctx.vault_token_pda);
    if reserve > 0 {
        ctx.operator_withdraw(reserve).expect("empty the reserve");
    }
    let admin = ctx.admin.insecure_clone();
    let closed = ctx.close_vault_as(&admin).is_ok();
    ctx.svm = saved;
    prop_assert!(!closed, "close_vault with {} shares out", supply);
    Ok(())
}

/// With every holder out, the vault closes exactly when no share is left. The
/// operator first takes out whatever is left in the reserve, rounding dust or
/// the value of shares too small to redeem, so the reserve cannot be the
/// reason for a refusal and the supply is the only thing under test.
fn assert_closes_when_empty(ctx: &mut VaultCtx) -> Result<(), TestCaseError> {
    if ctx.vault_state_data().paused {
        ctx.unpause().expect("unpause");
    }
    let empty = ctx.share_mint_supply() == 0;
    let dust = ctx.token_account_amount(&ctx.vault_token_pda);
    if dust > 0 {
        ctx.operator_withdraw(dust).expect("empty the reserve");
    }
    let admin = ctx.admin.insecure_clone();
    let ok = ctx.close_vault_as(&admin).is_ok();
    prop_assert_eq!(
        ok,
        empty,
        "close_vault with supply {}",
        ctx.share_mint_supply()
    );
    Ok(())
}

/// Accounting invariants that must hold in every reachable state.
fn assert_state_invariants(
    ctx: &VaultCtx,
    users: &[Depositor],
    model: &VaultModel,
) -> Result<(), TestCaseError> {
    let state = ctx.vault_state_data();
    prop_assert_eq!(
        state.local_aum,
        ctx.token_account_amount(&ctx.vault_token_pda),
        "local_aum drifted from the vault token account balance"
    );
    let held: u64 = holdings(ctx, users).iter().map(|h| h.shares).sum();
    prop_assert_eq!(
        ctx.share_mint_supply(),
        held,
        "share supply drifted from the depositors' balances"
    );
    assert_model_invariants(ctx, model)
}

/// A successful redeem paid `recipient` (deposit balance before and after):
/// the vault's outflow is exactly the fee plus the recipient's receipt, and the
/// fee is `ceil(assets * fee / 1e6)`. Returns what the recipient received.
fn assert_redeem_conserves(
    before: &CeiSnapshot,
    after: &CeiSnapshot,
    recipient: (u64, u64),
    fee_rate: u32,
) -> Result<u64, TestCaseError> {
    let assets = decrease(before.vault_tokens, after.vault_tokens, "vault balance")?;
    let fee_paid = increase(
        before.fee_recipient_tokens,
        after.fee_recipient_tokens,
        "fee recipient balance",
    )?;
    let received = increase(recipient.0, recipient.1, "recipient balance")?;
    let paid_out = fee_paid
        .checked_add(received)
        .ok_or_else(|| TestCaseError::fail("redeem payouts overflowed u64"))?;
    prop_assert_eq!(
        assets,
        paid_out,
        "redeem leaked value: vault paid {} but recipients got {}",
        assets,
        paid_out
    );
    prop_assert_eq!(
        fee_paid,
        expected_withdrawal_fee(assets, fee_rate),
        "fee not ceil(assets * fee / 1e6) for assets={}, rate={}",
        assets,
        fee_rate
    );
    Ok(received)
}

/// The effects every successful vault op must have: no bystander moved, a
/// deposit minted what the vault's math quotes for the amount it took, and a
/// redeem conserved value and paid what the math quotes for the shares.
fn assert_vault_op_effects(
    ctx: &VaultCtx,
    users: &[Depositor],
    op: &Op,
    outcome: &OpOutcome,
) -> Result<(), TestCaseError> {
    let after = holdings(ctx, users);
    assert_bystanders_untouched(&outcome.holdings, &after, outcome.actor, &format!("{op:?}"))?;
    let before = &outcome.holdings;
    match *op {
        Op::UserDeposit { who, amount } | Op::DepositChecked { who, amount, .. } => {
            let paid = decrease(before[who].deposit, after[who].deposit, "depositor balance")?;
            prop_assert_eq!(paid, amount, "deposit took {} for {}", paid, amount);
            let vault = ctx.snapshot();
            let taken = increase(
                outcome.before.vault_tokens,
                vault.vault_tokens,
                "vault balance",
            )?;
            prop_assert_eq!(taken, amount, "vault received {} for {}", taken, amount);
            let minted = increase(before[who].shares, after[who].shares, "depositor shares")?;
            prop_assert_eq!(
                Some(minted),
                outcome.quote,
                "deposit of {} minted {} shares",
                amount,
                minted
            );
        }
        Op::UserRedeem { who, .. } | Op::RedeemChecked { who, .. } => {
            let recipient = (before[who].deposit, after[who].deposit);
            let received = assert_redeem_conserves(
                &outcome.before,
                &ctx.snapshot(),
                recipient,
                outcome.fee_rate,
            )?;
            prop_assert_eq!(Some(received), outcome.quote, "redeem paid {}", received);
        }
        _ => {}
    }
    Ok(())
}

/// Every depositor redeems all their shares. A holding so small it is worth
/// nothing is refused with `ZeroAmount` and left behind; any other refusal
/// fails the case.
fn exit_all(ctx: &mut VaultCtx, users: &[Depositor]) -> Result<(), TestCaseError> {
    if ctx.vault_state_data().paused {
        ctx.unpause().expect("unpause");
    }
    for user in users {
        let shares = ctx.token_account_amount(&user.share_ata);
        if shares == 0 {
            continue;
        }
        let worthless = ctx.quote_redeem(shares).0 == 0;
        match ctx.redeem_as(user, shares) {
            Ok(()) => prop_assert!(!worthless, "redeemed {} worthless shares", shares),
            Err(e) if worthless => assert_anchor_err(&e, VaultError::ZeroAmount),
            Err(e) => return Err(TestCaseError::fail(format!("full exit refused: {e:?}"))),
        }
    }
    Ok(())
}

/// What the depositors hold of the deposit token, together.
fn total_deposit_tokens(ctx: &VaultCtx, users: &[Depositor]) -> u64 {
    holdings(ctx, users).iter().map(|h| h.deposit).sum()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Full op mix (including AUM reports): every op the model can decide
    /// succeeds exactly when it says; accounting invariants hold after every
    /// step; failures are perfectly atomic; no op moves a bystander; every
    /// successful redeem conserves value with exact ceil-rounded fees.
    #[test]
    fn invariants_hold_under_random_op_sequences(
        ops in proptest::collection::vec(op_strategy(true), 1..60)
    ) {
        let (mut ctx, users, mut model) = fresh_walk_vault();
        assert_state_invariants(&ctx, &users, &model)?;

        for op in &ops {
            let outcome = execute(&mut ctx, &users, &mut model, op)?;

            if !outcome.ok {
                prop_assert_eq!(
                    ctx.snapshot(), outcome.before,
                    "failed {:?} left side effects", op
                );
                prop_assert_eq!(
                    holdings(&ctx, &users), outcome.holdings,
                    "failed {:?} moved a depositor's balances", op
                );
                continue;
            }

            assert_state_invariants(&ctx, &users, &model)?;
            assert_vault_op_effects(&ctx, &users, op, &outcome)?;
        }
    }

    /// No-yield walks (AUM reports excluded): after everything deployed comes
    /// home and every depositor exits completely, they cannot together hold
    /// more of the deposit token than they started with — rounding, fees and
    /// custody losses only ever cost them. Then the vault closes exactly when
    /// no share is left.
    #[test]
    fn no_value_extraction_without_yield(
        ops in proptest::collection::vec(op_strategy(false), 1..60)
    ) {
        let (mut ctx, users, mut model) = fresh_walk_vault();

        for op in &ops {
            execute(&mut ctx, &users, &mut model, op)?;
        }

        recall_deployed(&mut ctx, &mut model);
        assert_close_refused_while_held(&mut ctx)?;
        exit_all(&mut ctx, &users)?;

        prop_assert!(
            total_deposit_tokens(&ctx, &users) <= INITIAL_USER_FUNDS * DEPOSITORS as u64,
            "depositors extracted value from a yield-free vault"
        );
        assert_closes_when_empty(&mut ctx)?;
    }
}

// ---- the queue walk (WQ-10) ----

/// Request ids come from a small pool, so a walk both collides with a live id
/// and reuses one after it closes, which is what the sequence stamp guards.
/// Each depositor draws from the same pool, so the same id is live under
/// several owners at once.
const REQUEST_ID_POOL: u64 = 4;
const QUEUE_WALK_CASES: u32 = 40;
const QUEUE_WALK_STEPS: usize = 250;
/// WQ-10's floor on the steps one run of the queue walk executes. Every case
/// runs all its steps or fails the test, so the constants alone decide it.
const MIN_QUEUE_WALK_STEPS: usize = 10_000;
const _: () = assert!(QUEUE_WALK_CASES as usize * QUEUE_WALK_STEPS >= MIN_QUEUE_WALK_STEPS);
const DAY: u64 = 24 * 60 * 60;
const INITIAL_COOLDOWN: u64 = 60 * 60;

#[derive(Debug, Clone, Copy)]
enum Caller {
    Owner,
    Keeper,
    Stranger,
    /// A depositor other than the owner: no more rights than a stranger.
    OtherDepositor,
}

#[derive(Debug, Clone)]
enum QueueOp {
    Vault(Op),
    /// Depositor `who` escrows this many per-mille of their shares under `id`,
    /// paying out to depositor `pay_to` (possibly themselves) and naming the
    /// keeper as the only other finalizer or leaving finalization open.
    Request {
        who: usize,
        id: u64,
        pm: u16,
        keeper_only: bool,
        pay_to: usize,
    },
    /// `pick` selects a live request by index, modulo the number live; every
    /// request op is a no-op when none are.
    Finalize {
        pick: usize,
        caller: Caller,
    },
    /// Finalize by the owner with a stamp one below the request's own.
    FinalizeStale {
        pick: usize,
    },
    /// Cancel by the owner, or by another depositor into their own account.
    Cancel {
        pick: usize,
        by_other: bool,
    },
    Expedite {
        pick: usize,
        by_operator: bool,
    },
    Warp(u64),
    /// Depositor `who` sends this many per-mille of their shares straight to
    /// the escrow.
    Stray {
        who: usize,
        pm: u16,
    },
    /// Admin sweeps the stray to depositor `to`.
    Sweep {
        to: usize,
    },
    SetCooldown(u64),
    SetWindow(u64),
    Release,
    Attach,
    /// Builds `target` with every account right, for depositor `who` or the
    /// live request `pick` selects, then forges it with decoys drawn from
    /// `decoy`. See [`QueueWalk::substitute`].
    Substitute {
        target: Target,
        who: usize,
        pick: usize,
        decoy: usize,
    },
}

/// The instructions [`QueueOp::Substitute`] forges: the holders' and the
/// queue's, then the operator's and the admin's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Deposit,
    Redeem,
    Request,
    Finalize,
    Cancel,
    Expedite,
    Sweep,
    OperatorWithdraw,
    OperatorReturn,
    SubWithdraw,
    SubReturn,
    UpdateAum,
    SetFee,
    Pause,
    SetAumLimits,
    SettleLoss,
    Deregister,
    Nominate,
    SetCooldown,
    SetWindow,
    Release,
    Attach,
}

const TARGETS: [Target; 22] = [
    Target::Deposit,
    Target::Redeem,
    Target::Request,
    Target::Finalize,
    Target::Cancel,
    Target::Expedite,
    Target::Sweep,
    Target::OperatorWithdraw,
    Target::OperatorReturn,
    Target::SubWithdraw,
    Target::SubReturn,
    Target::UpdateAum,
    Target::SetFee,
    Target::Pause,
    Target::SetAumLimits,
    Target::SettleLoss,
    Target::Deregister,
    Target::Nominate,
    Target::SetCooldown,
    Target::SetWindow,
    Target::Release,
    Target::Attach,
];

fn vault_ix(accounts: impl ToAccountMetas, data: impl InstructionData) -> Instruction {
    Instruction {
        program_id: august_vault::ID,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

fn queue_ix(accounts: impl ToAccountMetas, data: impl InstructionData) -> Instruction {
    Instruction {
        program_id: august_withdrawal_queue::ID,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

fn queue_op_strategy(include_yield_ops: bool) -> BoxedStrategy<QueueOp> {
    let caller = prop_oneof![
        Just(Caller::Owner),
        Just(Caller::Keeper),
        Just(Caller::Stranger),
        Just(Caller::OtherDepositor)
    ];
    // Most requests pay their owner; the rest pay another depositor, which
    // the queue allows and which moves the payout away from the share holder.
    let pay_to = prop_oneof![3 => Just(None), 1 => (0..DEPOSITORS).prop_map(Some)];
    let queue = prop_oneof![
        3 => (0..DEPOSITORS, 0..REQUEST_ID_POOL, 0u16..=1000, any::<bool>(), pay_to)
            .prop_map(|(who, id, pm, keeper_only, pay_to)| QueueOp::Request {
                who,
                id,
                pm,
                keeper_only,
                pay_to: pay_to.unwrap_or(who),
            }),
        3 => (any::<usize>(), caller).prop_map(|(pick, caller)| QueueOp::Finalize { pick, caller }),
        1 => any::<usize>().prop_map(|pick| QueueOp::FinalizeStale { pick }),
        1 => (any::<usize>(), prop::bool::weighted(0.25))
            .prop_map(|(pick, by_other)| QueueOp::Cancel { pick, by_other }),
        1 => (any::<usize>(), any::<bool>())
            .prop_map(|(pick, by_operator)| QueueOp::Expedite { pick, by_operator }),
        2 => (0u64..=2 * DAY).prop_map(QueueOp::Warp),
        1 => (0..DEPOSITORS, 0u16..=50).prop_map(|(who, pm)| QueueOp::Stray { who, pm }),
        1 => (0..DEPOSITORS).prop_map(|to| QueueOp::Sweep { to }),
        1 => (0u64..=DAY).prop_map(QueueOp::SetCooldown),
        1 => prop_oneof![Just(0u64), 1u64..=3 * DAY].prop_map(QueueOp::SetWindow),
        1 => Just(QueueOp::Release),
        1 => Just(QueueOp::Attach),
        // Heavier than any one op: it spreads over every target in `TARGETS`.
        6 => (0..TARGETS.len(), 0..DEPOSITORS, any::<usize>(), any::<usize>())
            .prop_map(|(t, who, pick, decoy)| QueueOp::Substitute {
                target: TARGETS[t],
                who,
                pick,
                decoy,
            }),
    ];
    prop_oneof![
        1 => op_strategy(include_yield_ops).prop_map(QueueOp::Vault),
        3 => queue,
    ]
    .boxed()
}

/// The walk's own record of a live request, kept independently of the chain.
#[derive(Debug, Clone, Copy)]
struct Pending {
    sequence: u64,
    shares: u64,
    keeper_only: bool,
    /// The depositor the payout goes to.
    pay_to: usize,
    scheduled_eligible_at: i64,
    eligible_at: i64,
    expires_at: i64,
}

/// Every account a queue op may write, raw, so that a refused op can be shown
/// to have changed nothing.
#[derive(Debug, PartialEq, Eq)]
struct QueueSnapshot {
    vault: CeiSnapshot,
    holdings: Vec<Holdings>,
    queue: Vec<u8>,
    escrow_shares: u64,
    escrow_assets: u64,
    requests: Vec<Option<Vec<u8>>>,
}

/// Funds `other` through `holder` and leaves a live request in its queue, so
/// a decoy drawn from it is a real vault with real balances: a missing
/// cross-vault check cannot hide behind an empty account. Returns the
/// request's address.
fn fund_foreign_vault(ctx: &mut VaultCtx, holder: &Depositor, other: &OtherVault) -> Pubkey {
    const FOREIGN_DEPOSIT: u64 = 10 * SEED_DEPOSIT;
    let owner = holder.keypair.pubkey();
    let deposit_ata = ctx.create_ata_for(&owner, &other.deposit_mint);
    let share_ata = ctx.create_ata_for(&owner, &other.share_mint);
    let payer = ctx.payer.insecure_clone();
    let mint = spl_token::instruction::mint_to(
        &spl_token::ID,
        &other.deposit_mint,
        &deposit_ata,
        &payer.pubkey(),
        &[],
        FOREIGN_DEPOSIT,
    )
    .expect("mint_to");
    ctx.send_instructions(&payer, &[mint])
        .expect("fund the foreign holder");

    let deposit = Instruction {
        program_id: august_vault::ID,
        accounts: august_vault::accounts::Deposit {
            vault_state: other.vault_state,
            vault_token_ata: other.vault_token,
            sender_token_account: deposit_ata,
            sender_share_account: share_ata,
            share_mint: other.share_mint,
            deposit_mint: other.deposit_mint,
            signer: owner,
            token_program: spl_token::ID,
        }
        .to_account_metas(None),
        data: august_vault::instruction::Deposit {
            amount: FOREIGN_DEPOSIT,
        }
        .data(),
    };
    ctx.send_instructions(&holder.keypair, &[deposit])
        .expect("foreign deposit");

    let admin = ctx.admin.insecure_clone();
    let attach = Instruction {
        program_id: august_vault::ID,
        accounts: august_vault::accounts::AttachWithdrawalQueue {
            vault_state: other.vault_state,
            deposit_mint: other.deposit_mint,
            admin: admin.pubkey(),
            queue: other.queue,
        }
        .to_account_metas(None),
        data: august_vault::instruction::AttachWithdrawalQueue {}.data(),
    };
    ctx.send_instructions(&admin, &[attach])
        .expect("attach the foreign queue");

    let request = Pubkey::find_program_address(
        &[
            WITHDRAWAL_REQUEST_SEED,
            other.queue.as_ref(),
            owner.as_ref(),
            &0u64.to_le_bytes(),
        ],
        &august_withdrawal_queue::ID,
    )
    .0;
    let open = Instruction {
        program_id: august_withdrawal_queue::ID,
        accounts: august_withdrawal_queue::accounts::RequestWithdrawal {
            queue: other.queue,
            vault_state: other.vault_state,
            owner,
            owner_share_account: share_ata,
            escrow_shares: other.escrow_shares,
            share_mint: other.share_mint,
            recipient_token_account: deposit_ata,
            request,
            token_program: spl_token::ID,
            system_program: solana_sdk::system_program::ID,
            event_authority: event_authority_pda(),
            program: august_withdrawal_queue::ID,
        }
        .to_account_metas(None),
        data: august_withdrawal_queue::instruction::RequestWithdrawal {
            request_id: 0,
            shares: SEED_DEPOSIT,
            finalizer: Pubkey::default(),
        }
        .data(),
    };
    ctx.send_instructions(&holder.keypair, &[open])
        .expect("foreign request");
    request
}

/// An account's owner program, size and, for a token account, mint. Only a
/// decoy of the same kind gets past type checks to the constraint under test.
type AccountKind = (Pubkey, usize, Vec<u8>);

fn account_kind(ctx: &VaultCtx, key: &Pubkey) -> AccountKind {
    let account = ctx.svm.get_account(key).unwrap_or_default();
    let mint = if account.owner == spl_token::ID && account.data.len() == 165 {
        account.data[..32].to_vec()
    } else {
        Vec::new()
    };
    (account.owner, account.data.len(), mint)
}

struct QueueWalk {
    ctx: VaultCtx,
    users: Vec<Depositor>,
    model: VaultModel,
    keeper: Keypair,
    stranger: Keypair,
    /// Live requests by (owner index, request id).
    live: BTreeMap<(usize, u64), Pending>,
    sequence: u64,
    /// Shares sent to the escrow outside a request, which only a sweep removes.
    stray: u64,
    attached: bool,
    /// Accounts a forged instruction may name in place of the right one; the
    /// live requests' own PDAs are added when a substitution is drawn.
    decoys: Vec<Pubkey>,
    /// Forged instructions per [`TARGETS`] entry whose honest twin succeeded,
    /// i.e. the ones that tested something.
    forged: [usize; TARGETS.len()],
}

impl QueueWalk {
    fn new() -> Self {
        let (mut ctx, users, model) = fresh_walk_vault();
        ctx.open_queue(INITIAL_COOLDOWN);
        let keeper = ctx.new_funded_keypair(1_000_000_000);
        let stranger = ctx.new_funded_keypair(1_000_000_000);
        let other = ctx.new_vault_with_queue();
        let foreign_request = fund_foreign_vault(&mut ctx, &users[1], &other);
        let mut decoys = vec![
            other.vault_state,
            other.deposit_mint,
            other.share_mint,
            other.vault_token,
            other.queue,
            other.escrow_shares,
            other.escrow_assets,
            foreign_request,
            ctx.vault_state,
            ctx.deposit_mint,
            ctx.share_mint,
            ctx.vault_token_pda,
            ctx.withdrawal_queue_pda(),
            ctx.queue_escrow(&ctx.share_mint),
            ctx.queue_escrow(&ctx.deposit_mint),
            ctx.fee_recipient_deposit_ata,
            ctx.operator_deposit_ata,
            keeper.pubkey(),
            stranger.pubkey(),
            solana_sdk::system_program::ID,
            spl_token::ID,
            spl_token_2022::ID,
            august_vault::ID,
            august_withdrawal_queue::ID,
            event_authority_pda(),
            program_config_pda(),
        ];
        for user in &users {
            decoys.push(user.deposit_ata);
            decoys.push(user.share_ata);
        }
        QueueWalk {
            ctx,
            users,
            model,
            keeper,
            stranger,
            live: BTreeMap::new(),
            sequence: 0,
            stray: 0,
            attached: true,
            decoys,
            forged: [0; TARGETS.len()],
        }
    }

    fn owner(&self, who: usize) -> Pubkey {
        self.users[who].keypair.pubkey()
    }

    /// The depositor after `who`, a counterparty who is never `who`.
    fn other(who: usize) -> usize {
        (who + 1) % DEPOSITORS
    }

    fn pick(&self, pick: usize) -> Option<((usize, u64), Pending)> {
        if self.live.is_empty() {
            return None;
        }
        let (key, pending) = self.live.iter().nth(pick % self.live.len())?;
        Some((*key, *pending))
    }

    /// A request account's data, or `None` once it is closed.
    fn request_account(&self, who: usize, id: u64) -> Option<Vec<u8>> {
        let account = self
            .ctx
            .svm
            .get_account(&self.ctx.request_pda(&self.owner(who), id))?;
        if account.lamports == 0 {
            return None;
        }
        Some(account.data)
    }

    fn holdings(&self) -> Vec<Holdings> {
        holdings(&self.ctx, &self.users)
    }

    fn snapshot(&self) -> QueueSnapshot {
        let queue = self
            .ctx
            .svm
            .get_account(&self.ctx.withdrawal_queue_pda())
            .expect("queue account exists");
        let mut requests = Vec::new();
        for who in 0..DEPOSITORS {
            for id in 0..REQUEST_ID_POOL {
                requests.push(self.request_account(who, id));
            }
        }
        QueueSnapshot {
            vault: self.ctx.snapshot(),
            holdings: self.holdings(),
            queue: queue.data,
            escrow_shares: self.escrow_shares(),
            escrow_assets: self.escrow_assets(),
            requests,
        }
    }

    fn escrow_shares(&self) -> u64 {
        let escrow = self.ctx.queue_escrow(&self.ctx.share_mint);
        self.ctx.token_account_amount(&escrow)
    }

    fn escrow_assets(&self) -> u64 {
        let escrow = self.ctx.queue_escrow(&self.ctx.deposit_mint);
        self.ctx.token_account_amount(&escrow)
    }

    fn shares_of(&self, who: usize) -> u64 {
        self.ctx.token_account_amount(&self.users[who].share_ata)
    }

    /// Runs one op, checks it succeeded exactly when the model says it should
    /// and moved exactly what it should, then checks every invariant.
    fn step(&mut self, op: &QueueOp) -> Result<(), TestCaseError> {
        let before = self.snapshot();
        let ok = self.execute(op)?;
        if !ok {
            prop_assert_eq!(
                &self.snapshot(),
                &before,
                "failed {:?} left side effects",
                op
            );
        }
        self.assert_invariants()
    }

    fn execute(&mut self, op: &QueueOp) -> Result<bool, TestCaseError> {
        match *op {
            QueueOp::Vault(ref op) => {
                let outcome = execute(&mut self.ctx, &self.users, &mut self.model, op)?;
                if outcome.ok {
                    prop_assert!(
                        !(outcome.is_redeem && self.attached),
                        "a direct redeem got past the attached queue"
                    );
                    assert_vault_op_effects(&self.ctx, &self.users, op, &outcome)?;
                }
                Ok(outcome.ok)
            }
            QueueOp::Request {
                who,
                id,
                pm,
                keeper_only,
                pay_to,
            } => {
                let shares = per_mille(self.shares_of(who), pm);
                let finalizer = if keeper_only {
                    self.keeper.pubkey()
                } else {
                    Pubkey::default()
                };
                let queue = self.ctx.queue_state_data();
                let now = self.ctx.now();
                let before = self.holdings();
                let owner = self.users[who].keypair.insecure_clone();
                let (share_ata, recipient) =
                    (self.users[who].share_ata, self.users[pay_to].deposit_ata);
                let ok = self
                    .ctx
                    .request_withdrawal_as(&owner, share_ata, recipient, id, shares, finalizer)
                    .is_ok();
                let expected = self.attached && shares > 0 && !self.live.contains_key(&(who, id));
                prop_assert_eq!(
                    ok,
                    expected,
                    "request {} of {} shares by {}",
                    id,
                    shares,
                    who
                );
                if ok {
                    let after = self.holdings();
                    prop_assert_eq!(
                        before[who].shares - after[who].shares,
                        shares,
                        "request escrowed the wrong amount"
                    );
                    assert_bystanders_untouched(&before, &after, Some(who), "request")?;
                    prop_assert_eq!(before[who].deposit, after[who].deposit);
                    self.sequence += 1;
                    let scheduled = now + queue.cooldown_seconds as i64;
                    let expires_at = if queue.fulfillment_window_seconds == 0 {
                        0
                    } else {
                        scheduled + queue.fulfillment_window_seconds as i64
                    };
                    self.live.insert(
                        (who, id),
                        Pending {
                            sequence: self.sequence,
                            shares,
                            keeper_only,
                            pay_to,
                            scheduled_eligible_at: scheduled,
                            eligible_at: scheduled,
                            expires_at,
                        },
                    );
                }
                Ok(ok)
            }
            QueueOp::Finalize { pick, caller } => {
                let Some(((who, id), pending)) = self.pick(pick) else {
                    return Ok(true);
                };
                let signer = match caller {
                    Caller::Owner => self.users[who].keypair.insecure_clone(),
                    Caller::Keeper => self.keeper.insecure_clone(),
                    Caller::Stranger => self.stranger.insecure_clone(),
                    Caller::OtherDepositor => self.users[Self::other(who)].keypair.insecure_clone(),
                };
                let permitted = match caller {
                    Caller::Owner | Caller::Keeper => true,
                    Caller::Stranger | Caller::OtherDepositor => !pending.keeper_only,
                };
                let now = self.ctx.now();
                let expired = pending.expires_at != 0 && now >= pending.expires_at;
                let (gross, net) = self.ctx.quote_redeem(pending.shares);
                let state = self.ctx.vault_state_data();
                let expected = permitted
                    && now >= pending.eligible_at
                    && !expired
                    && !state.paused
                    && gross > 0
                    && gross <= state.local_aum;

                let before = self.ctx.snapshot();
                let holdings_before = self.holdings();
                let owner = self.owner(who);
                let ok = self
                    .ctx
                    .finalize_withdrawal_as(&signer, &owner, id, pending.sequence)
                    .is_ok();
                prop_assert_eq!(
                    ok,
                    expected,
                    "finalize {:?} of {} by {:?} at {}",
                    pending,
                    who,
                    caller,
                    now
                );
                if ok {
                    let after = self.ctx.snapshot();
                    let holdings_after = self.holdings();
                    let paid = pending.pay_to;
                    let recipient = (holdings_before[paid].deposit, holdings_after[paid].deposit);
                    let received =
                        assert_redeem_conserves(&before, &after, recipient, state.withdrawal_fee)?;
                    prop_assert_eq!(
                        received,
                        net,
                        "queue paid {} where a direct redeem at the same state pays {}",
                        received,
                        net
                    );
                    // Only the recipient's deposit balance moves: the shares
                    // came out of the escrow, not out of anyone's account.
                    let mut expected_holdings = holdings_before.clone();
                    expected_holdings[paid].deposit = holdings_after[paid].deposit;
                    prop_assert_eq!(
                        &holdings_after,
                        &expected_holdings,
                        "finalize moved a balance other than the recipient's deposit"
                    );
                    self.live.remove(&(who, id));
                }
                Ok(ok)
            }
            QueueOp::FinalizeStale { pick } => {
                let Some(((who, id), pending)) = self.pick(pick) else {
                    return Ok(true);
                };
                let (signer, owner) = (self.users[who].keypair.insecure_clone(), self.owner(who));
                let err = self
                    .ctx
                    .finalize_withdrawal_as(&signer, &owner, id, pending.sequence - 1)
                    .expect_err("finalize with a stale stamp");
                assert_anchor_framework_err(
                    &err,
                    QueueError::StaleRequestSequence as u32 + ANCHOR_USER_ERROR_OFFSET,
                );
                Ok(false)
            }
            QueueOp::Cancel { pick, by_other } => {
                let Some(((who, id), pending)) = self.pick(pick) else {
                    return Ok(true);
                };
                let owner = self.owner(who);
                if by_other {
                    let other = &self.users[Self::other(who)];
                    let (signer, destination) = (other.keypair.insecure_clone(), other.share_ata);
                    let err = self
                        .ctx
                        .cancel_withdrawal_as(&signer, &owner, id, pending.sequence, destination)
                        .expect_err("cancel by a depositor who does not own the request");
                    assert_anchor_framework_err(
                        &err,
                        QueueError::NotRequestOwner as u32 + ANCHOR_USER_ERROR_OFFSET,
                    );
                    return Ok(false);
                }
                let before = self.holdings();
                let (signer, destination) = (
                    self.users[who].keypair.insecure_clone(),
                    self.users[who].share_ata,
                );
                self.ctx
                    .cancel_withdrawal_as(&signer, &owner, id, pending.sequence, destination)
                    .map_err(|e| TestCaseError::fail(format!("cancel refused: {e:?}")))?;
                let after = self.holdings();
                prop_assert_eq!(
                    after[who].shares - before[who].shares,
                    pending.shares,
                    "cancel returned the wrong amount"
                );
                assert_bystanders_untouched(&before, &after, Some(who), "cancel")?;
                self.live.remove(&(who, id));
                Ok(true)
            }
            QueueOp::Expedite { pick, by_operator } => {
                let Some(((who, id), pending)) = self.pick(pick) else {
                    return Ok(true);
                };
                let authority = if by_operator {
                    self.ctx.operator.insecure_clone()
                } else {
                    self.ctx.admin.insecure_clone()
                };
                let now = self.ctx.now();
                let expired = pending.expires_at != 0 && now >= pending.expires_at;
                let expected = !expired && now < pending.eligible_at;
                let owner = self.owner(who);
                let ok = self
                    .ctx
                    .expedite_request_as(&authority, &owner, id, pending.sequence)
                    .is_ok();
                prop_assert_eq!(ok, expected, "expedite {:?} at {}", pending, now);
                if ok {
                    self.live.get_mut(&(who, id)).expect("picked").eligible_at = now;
                }
                Ok(ok)
            }
            QueueOp::Warp(secs) => {
                self.ctx.warp_forward_seconds(secs as i64);
                Ok(true)
            }
            QueueOp::Stray { who, pm } => {
                let amount = per_mille(self.shares_of(who), pm);
                let user = self.users[who].keypair.insecure_clone();
                let (mint, from) = (self.ctx.share_mint, self.users[who].share_ata);
                let escrow = self.ctx.queue_escrow(&mint);
                self.ctx
                    .transfer_mint_tokens_as(&user, &mint, &from, &escrow, amount);
                self.stray += amount;
                Ok(true)
            }
            QueueOp::Sweep { to } => {
                let before = self.holdings();
                let destination = self.users[to].share_ata;
                let ok = self.ctx.sweep_escrow_shares(destination).is_ok();
                prop_assert_eq!(ok, self.stray > 0, "sweep with {} stray", self.stray);
                if ok {
                    let after = self.holdings();
                    prop_assert_eq!(
                        after[to].shares - before[to].shares,
                        self.stray,
                        "sweep moved the wrong amount"
                    );
                    assert_bystanders_untouched(&before, &after, Some(to), "sweep")?;
                    self.stray = 0;
                    prop_assert_eq!(
                        self.escrow_shares(),
                        self.ctx.queue_state_data().pending_shares,
                        "escrow holds more than the pending shares right after a sweep"
                    );
                }
                Ok(ok)
            }
            QueueOp::SetCooldown(seconds) => {
                self.ctx
                    .set_cooldown(seconds)
                    .map_err(|e| TestCaseError::fail(format!("set_cooldown refused: {e:?}")))?;
                Ok(true)
            }
            QueueOp::SetWindow(seconds) => {
                self.ctx.set_fulfillment_window(seconds).map_err(|e| {
                    TestCaseError::fail(format!("set_fulfillment_window refused: {e:?}"))
                })?;
                Ok(true)
            }
            QueueOp::Release => {
                let ok = self.ctx.release_vault().is_ok();
                prop_assert_eq!(ok, self.attached, "release_vault");
                if ok {
                    self.attached = false;
                }
                Ok(ok)
            }
            QueueOp::Attach => {
                let pda = self.ctx.withdrawal_queue_pda();
                let ok = self.ctx.attach_withdrawal_queue(pda).is_ok();
                prop_assert_eq!(ok, !self.attached, "attach_withdrawal_queue");
                if ok {
                    self.attached = true;
                }
                Ok(ok)
            }
            QueueOp::Substitute {
                target,
                who,
                pick,
                decoy,
            } => self.substitute(target, who, pick, decoy),
        }
    }

    /// `target` with every account right, its signer, and the accounts the
    /// caller may legitimately choose, or `None` when there is nothing to
    /// build it from.
    fn honest(
        &self,
        target: Target,
        who: usize,
        pick: usize,
    ) -> Option<(Keypair, Instruction, Vec<Pubkey>)> {
        let ctx = &self.ctx;
        let user = &self.users[who];
        let admin = ctx.admin.insecure_clone();
        let built = match target {
            Target::Deposit => {
                let amount = ctx.token_account_amount(&user.deposit_ata) / 100 + 1;
                let ix = Instruction {
                    program_id: august_vault::ID,
                    accounts: august_vault::accounts::Deposit {
                        vault_state: ctx.vault_state,
                        vault_token_ata: ctx.vault_token_pda,
                        sender_token_account: user.deposit_ata,
                        sender_share_account: user.share_ata,
                        share_mint: ctx.share_mint,
                        deposit_mint: ctx.deposit_mint,
                        signer: user.keypair.pubkey(),
                        token_program: spl_token::ID,
                    }
                    .to_account_metas(None),
                    data: august_vault::instruction::Deposit { amount }.data(),
                };
                (user.keypair.insecure_clone(), ix, vec![])
            }
            Target::Redeem => {
                let shares = ctx.token_account_amount(&user.share_ata) / 2;
                if shares == 0 {
                    return None;
                }
                let ix = Instruction {
                    program_id: august_vault::ID,
                    accounts: august_vault::accounts::Redeem {
                        vault_state: ctx.vault_state,
                        vault_deposit_ata: ctx.vault_token_pda,
                        sender_token_account: user.deposit_ata,
                        sender_share_account: user.share_ata,
                        fee_recipient_account: ctx.fee_recipient_deposit_ata,
                        share_mint: ctx.share_mint,
                        deposit_mint: ctx.deposit_mint,
                        signer: user.keypair.pubkey(),
                        token_program: spl_token::ID,
                    }
                    .to_account_metas(None),
                    data: august_vault::instruction::Redeem { shares }.data(),
                };
                (user.keypair.insecure_clone(), ix, vec![])
            }
            Target::Request => {
                let id = (0..REQUEST_ID_POOL).find(|id| !self.live.contains_key(&(who, *id)))?;
                let shares = ctx.token_account_amount(&user.share_ata) / 2;
                let owner = user.keypair.pubkey();
                let ix = Instruction {
                    program_id: august_withdrawal_queue::ID,
                    accounts: ctx
                        .request_withdrawal_accounts(&owner, user.share_ata, user.deposit_ata, id)
                        .to_account_metas(None),
                    data: august_withdrawal_queue::instruction::RequestWithdrawal {
                        request_id: id,
                        shares,
                        finalizer: Pubkey::default(),
                    }
                    .data(),
                };
                // The owner names the payout account; any valid one will do.
                (user.keypair.insecure_clone(), ix, vec![user.deposit_ata])
            }
            Target::Finalize | Target::Cancel | Target::Expedite => {
                // Draw from the requests the honest instruction can act on, or
                // most finalizes and expedites would be refused on timing and
                // forge nothing.
                let now = ctx.now();
                let actionable: Vec<((usize, u64), Pending)> = self
                    .live
                    .iter()
                    .filter(|(_, p)| {
                        let expired = p.expires_at != 0 && now >= p.expires_at;
                        match target {
                            Target::Finalize => now >= p.eligible_at && !expired,
                            Target::Expedite => now < p.eligible_at && !expired,
                            _ => true,
                        }
                    })
                    .map(|(key, p)| (*key, *p))
                    .collect();
                if actionable.is_empty() {
                    return None;
                }
                let ((owner_ix, id), pending) = actionable[pick % actionable.len()];
                let owner = &self.users[owner_ix];
                let key = owner.keypair.pubkey();
                match target {
                    Target::Finalize => (
                        owner.keypair.insecure_clone(),
                        ctx.finalize_withdrawal_ix(&key, &key, id, pending.sequence),
                        vec![],
                    ),
                    Target::Cancel => {
                        let ix = Instruction {
                            program_id: august_withdrawal_queue::ID,
                            accounts: ctx
                                .cancel_withdrawal_accounts(&key, &key, id, owner.share_ata)
                                .to_account_metas(None),
                            data: august_withdrawal_queue::instruction::CancelWithdrawal {
                                expected_sequence: pending.sequence,
                            }
                            .data(),
                        };
                        (owner.keypair.insecure_clone(), ix, vec![])
                    }
                    _ => (
                        admin.insecure_clone(),
                        ctx.expedite_request_ix(&admin.pubkey(), &key, id, pending.sequence),
                        vec![],
                    ),
                }
            }
            Target::Sweep => {
                let ix = Instruction {
                    program_id: august_withdrawal_queue::ID,
                    accounts: ctx
                        .sweep_escrow_shares_accounts(&admin.pubkey(), user.share_ata)
                        .to_account_metas(None),
                    data: august_withdrawal_queue::instruction::SweepEscrowShares {}.data(),
                };
                // The admin chooses where the stray goes.
                (admin, ix, vec![user.share_ata])
            }
            _ => self.honest_privileged(target, who)?,
        };
        Some(built)
    }

    /// The operator's and the admin's instructions for [`Self::honest`], each
    /// built to change nothing it need not: the fee, limits, cooldown and
    /// window are set to what they already are, the AUM is reported as it
    /// stands. `who` picks the subaccount or the candidate.
    fn honest_privileged(
        &self,
        target: Target,
        who: usize,
    ) -> Option<(Keypair, Instruction, Vec<Pubkey>)> {
        let ctx = &self.ctx;
        let state = ctx.vault_state_data();
        let queue = ctx.queue_state_data();
        let admin = ctx.admin.insecure_clone();
        let operator = ctx.operator.insecure_clone();
        let subs = &self.model.subs;
        let sub = if subs.is_empty() {
            None
        } else {
            Some(&subs[who % subs.len()])
        };
        let queue_admin = || august_withdrawal_queue::accounts::QueueAdmin {
            queue: ctx.withdrawal_queue_pda(),
            vault_state: ctx.vault_state,
            admin: admin.pubkey(),
            event_authority: event_authority_pda(),
            program: august_withdrawal_queue::ID,
        };
        let built = match target {
            Target::OperatorWithdraw | Target::OperatorReturn => {
                if !subs.is_empty() {
                    return None;
                }
                let accounts = august_vault::accounts::OperatorWithdraw {
                    vault_state: ctx.vault_state,
                    vault_deposit_ata: ctx.vault_token_pda,
                    operator_token_account: ctx.operator_deposit_ata,
                    subaccount: None,
                    deposit_mint: ctx.deposit_mint,
                    operator: operator.pubkey(),
                    token_program: spl_token::ID,
                };
                let ix = if target == Target::OperatorWithdraw {
                    let amount = state.local_aum / 10;
                    vault_ix(
                        accounts,
                        august_vault::instruction::OperatorWithdraw { amount },
                    )
                } else {
                    let amount = ctx.token_account_amount(&ctx.operator_deposit_ata) / 2;
                    let accounts = august_vault::accounts::OperatorDeposit {
                        vault_state: accounts.vault_state,
                        vault_deposit_ata: accounts.vault_deposit_ata,
                        operator_token_account: accounts.operator_token_account,
                        subaccount: None,
                        deposit_mint: accounts.deposit_mint,
                        operator: accounts.operator,
                        token_program: accounts.token_program,
                    };
                    vault_ix(
                        accounts,
                        august_vault::instruction::OperatorDeposit { amount },
                    )
                };
                (operator, ix, vec![])
            }
            Target::SubWithdraw | Target::SubReturn => {
                let (sub, _) = sub?;
                let ix = if target == Target::SubWithdraw {
                    let amount = state.local_aum / 10;
                    let accounts = august_vault::accounts::OperatorWithdraw {
                        vault_state: ctx.vault_state,
                        vault_deposit_ata: ctx.vault_token_pda,
                        operator_token_account: sub.deposit_ata,
                        subaccount: Some(sub.pda),
                        deposit_mint: ctx.deposit_mint,
                        operator: operator.pubkey(),
                        token_program: spl_token::ID,
                    };
                    vault_ix(
                        accounts,
                        august_vault::instruction::OperatorWithdraw { amount },
                    )
                } else {
                    let amount = ctx.token_account_amount(&sub.deposit_ata) / 2;
                    let accounts = august_vault::accounts::OperatorDeposit {
                        vault_state: ctx.vault_state,
                        vault_deposit_ata: ctx.vault_token_pda,
                        operator_token_account: sub.deposit_ata,
                        subaccount: Some(sub.pda),
                        deposit_mint: ctx.deposit_mint,
                        operator: operator.pubkey(),
                        token_program: spl_token::ID,
                    };
                    vault_ix(
                        accounts,
                        august_vault::instruction::OperatorDeposit { amount },
                    )
                };
                (operator, ix, vec![])
            }
            Target::UpdateAum => {
                let accounts = august_vault::accounts::OperatorUpdateAum {
                    vault_state: ctx.vault_state,
                    deposit_mint: ctx.deposit_mint,
                    operator: operator.pubkey(),
                };
                let data = august_vault::instruction::OperatorUpdateAum {
                    new_aum: state.deployed_aum,
                };
                (operator, vault_ix(accounts, data), vec![])
            }
            Target::SetFee => {
                let accounts = august_vault::accounts::SetWithdrawalFee {
                    vault_state: ctx.vault_state,
                    deposit_mint: ctx.deposit_mint,
                    admin: admin.pubkey(),
                };
                let data = august_vault::instruction::SetWithdrawalFee {
                    new_fee: state.withdrawal_fee,
                };
                (admin, vault_ix(accounts, data), vec![])
            }
            Target::Pause => {
                let ix = if state.paused {
                    let accounts = august_vault::accounts::Unpause {
                        vault_state: ctx.vault_state,
                        deposit_mint: ctx.deposit_mint,
                        admin: admin.pubkey(),
                    };
                    vault_ix(accounts, august_vault::instruction::Unpause {})
                } else {
                    let accounts = august_vault::accounts::Pause {
                        vault_state: ctx.vault_state,
                        deposit_mint: ctx.deposit_mint,
                        admin: admin.pubkey(),
                    };
                    vault_ix(accounts, august_vault::instruction::Pause {})
                };
                (admin, ix, vec![])
            }
            Target::SetAumLimits => {
                let accounts = august_vault::accounts::SetAumLimits {
                    vault_state: ctx.vault_state,
                    deposit_mint: ctx.deposit_mint,
                    admin: admin.pubkey(),
                };
                let data = august_vault::instruction::SetAumLimits {
                    increase_limit: state.aum_increase_limit,
                    decrease_limit: state.aum_decrease_limit,
                };
                (admin, vault_ix(accounts, data), vec![])
            }
            Target::SettleLoss => {
                let (sub, principal) = sub?;
                let amount = principal.saturating_sub(ctx.token_account_amount(&sub.deposit_ata));
                let accounts = august_vault::accounts::SettleSubaccountLoss {
                    vault_state: ctx.vault_state,
                    subaccount: sub.pda,
                    deposit_mint: ctx.deposit_mint,
                    subaccount_ata: sub.deposit_ata,
                    token_program: spl_token::ID,
                    admin: admin.pubkey(),
                };
                let data = august_vault::instruction::SettleSubaccountLoss { amount };
                (admin, vault_ix(accounts, data), vec![])
            }
            Target::Deregister => {
                let (sub, _) = subs.iter().find(|(_, principal)| *principal == 0)?;
                let accounts = august_vault::accounts::DeregisterSubaccount {
                    vault_state: ctx.vault_state,
                    subaccount: sub.pda,
                    deposit_mint: ctx.deposit_mint,
                    admin: admin.pubkey(),
                };
                let data = august_vault::instruction::DeregisterSubaccount {};
                // Which empty subaccount goes is the admin's choice: nothing
                // else in the instruction ties the registry entry down.
                (admin, vault_ix(accounts, data), vec![sub.pda])
            }
            Target::Nominate => {
                let candidates = &self.model.candidates;
                let nominee = candidates[who % candidates.len()].pubkey();
                let accounts = august_vault::accounts::NominateAdmin {
                    vault_state: ctx.vault_state,
                    deposit_mint: ctx.deposit_mint,
                    nominated_admin_pda: ctx.nominated_admin_pda(),
                    admin: admin.pubkey(),
                    payer: admin.pubkey(),
                    system_program: solana_sdk::system_program::ID,
                };
                let data = august_vault::instruction::NominateAdmin { new_admin: nominee };
                (admin, vault_ix(accounts, data), vec![])
            }
            Target::SetCooldown => {
                let data = august_withdrawal_queue::instruction::SetCooldown {
                    seconds: queue.cooldown_seconds,
                };
                let ix = queue_ix(queue_admin(), data);
                (admin, ix, vec![])
            }
            Target::SetWindow => {
                let data = august_withdrawal_queue::instruction::SetFulfillmentWindow {
                    seconds: queue.fulfillment_window_seconds,
                };
                let ix = queue_ix(queue_admin(), data);
                (admin, ix, vec![])
            }
            Target::Release => {
                let accounts = ctx.release_vault_accounts(&admin.pubkey());
                let ix = queue_ix(
                    accounts,
                    august_withdrawal_queue::instruction::ReleaseVault {},
                );
                (admin, ix, vec![])
            }
            Target::Attach => {
                let accounts = august_vault::accounts::AttachWithdrawalQueue {
                    vault_state: ctx.vault_state,
                    deposit_mint: ctx.deposit_mint,
                    admin: admin.pubkey(),
                    queue: ctx.withdrawal_queue_pda(),
                };
                let ix = vault_ix(
                    accounts,
                    august_vault::instruction::AttachWithdrawalQueue {},
                );
                (admin, ix, vec![])
            }
            _ => return None,
        };
        Some(built)
    }

    /// Sends `target` once per account the program must bind, each time with
    /// that one account swapped for a decoy: never a signer (wrong signers are
    /// the caller ops' job) and never an account the caller may choose. Every
    /// forgery must be refused and, like any refused op, change nothing, so
    /// they cannot disturb one another. They are only sent when the honest
    /// instruction succeeds on a copy of the chain, so the decoy is the one
    /// thing that can explain each refusal.
    fn substitute(
        &mut self,
        target: Target,
        who: usize,
        pick: usize,
        decoy: usize,
    ) -> Result<bool, TestCaseError> {
        let Some((signer, ix, chosen)) = self.honest(target, who, pick) else {
            return Ok(false);
        };
        let saved = self.ctx.svm.clone();
        let honest_ok = self
            .ctx
            .send_instructions(&signer, std::slice::from_ref(&ix))
            .is_ok();
        self.ctx.svm = saved;
        if !honest_ok {
            return Ok(false);
        }

        let mut pool = self.decoys.clone();
        for (owner, id) in self.live.keys() {
            pool.push(self.ctx.request_pda(&self.owner(*owner), *id));
        }
        for (sub, _) in &self.model.subs {
            pool.push(sub.pda);
            pool.push(sub.deposit_ata);
        }
        pool.push(self.ctx.nominated_admin_pda());
        pool.push(self.model.sink);
        // Refused forgeries change no account, so the kinds hold throughout.
        let pool: Vec<(Pubkey, AccountKind)> = pool
            .into_iter()
            .map(|key| (key, account_kind(&self.ctx, &key)))
            .collect();

        let index = TARGETS.iter().position(|t| *t == target);
        let index = index.expect("listed target");
        for (n, meta) in ix.accounts.iter().enumerate() {
            // The queue's own id fills the `program` account `#[event_cpi]`
            // adds, which Anchor leaves unchecked: `emit_cpi!` invokes
            // `crate::ID`, not that account, so no value there redirects
            // anything.
            if meta.is_signer
                || chosen.contains(&meta.pubkey)
                || meta.pubkey == august_withdrawal_queue::ID
            {
                continue;
            }
            let real = meta.pubkey;
            let real_kind = account_kind(&self.ctx, &real);
            let others: Vec<&(Pubkey, AccountKind)> =
                pool.iter().filter(|(key, _)| *key != real).collect();
            let alike: Vec<Pubkey> = others
                .iter()
                .filter(|(_, kind)| *kind == real_kind)
                .map(|(key, _)| *key)
                .collect();
            // A different draw for each account; three in four take a
            // look-alike when there is one.
            let draw = decoy.wrapping_add(n.wrapping_mul(7_919));
            let decoy = if draw % 4 != 0 && !alike.is_empty() {
                alike[draw / 4 % alike.len()]
            } else {
                others[draw % others.len()].0
            };

            let mut forged = ix.clone();
            forged.accounts[n].pubkey = decoy;
            let accepted = self.ctx.send_instructions(&signer, &[forged]).is_ok();
            prop_assert!(
                !accepted,
                "{:?} accepted {} in account {} in place of {}",
                target,
                decoy,
                n,
                real
            );
            self.forged[index] += 1;
        }
        Ok(false)
    }

    /// What must hold in every reachable state of a vault with a queue.
    fn assert_invariants(&self) -> Result<(), TestCaseError> {
        let state = self.ctx.vault_state_data();
        let queue = self.ctx.queue_state_data();
        prop_assert_eq!(
            state.local_aum,
            self.ctx.token_account_amount(&self.ctx.vault_token_pda),
            "local_aum drifted from the vault token account balance"
        );
        let held: u64 = self.holdings().iter().map(|h| h.shares).sum();
        prop_assert_eq!(
            self.ctx.share_mint_supply(),
            held + self.escrow_shares(),
            "share supply drifted from the depositors' and the escrow's balances"
        );

        let pending_shares: u64 = self.live.values().map(|p| p.shares).sum();
        prop_assert_eq!(queue.pending_requests, self.live.len() as u64);
        prop_assert_eq!(queue.pending_shares, pending_shares);
        prop_assert_eq!(queue.sequence, self.sequence);
        prop_assert!(
            self.escrow_shares() >= queue.pending_shares,
            "escrow holds {} shares but {} are pending",
            self.escrow_shares(),
            queue.pending_shares
        );
        prop_assert_eq!(
            self.escrow_shares(),
            queue.pending_shares + self.stray,
            "escrow is not the pending shares plus the stray"
        );
        prop_assert_eq!(
            self.escrow_assets(),
            0,
            "assets left behind in the asset escrow"
        );

        assert_model_invariants(&self.ctx, &self.model)?;

        let gate = if self.attached {
            self.ctx.withdrawal_queue_pda()
        } else {
            Pubkey::default()
        };
        prop_assert_eq!(state.withdrawal_queue_authority, gate);

        for who in 0..DEPOSITORS {
            for id in 0..REQUEST_ID_POOL {
                match self.live.get(&(who, id)) {
                    None => prop_assert!(
                        self.request_account(who, id).is_none(),
                        "request {} of {} not closed",
                        id,
                        who
                    ),
                    Some(p) => {
                        let r = self.ctx.request_state_data(&self.owner(who), id);
                        prop_assert_eq!(r.sequence, p.sequence);
                        prop_assert_eq!(r.shares, p.shares);
                        prop_assert_eq!(
                            r.recipient_token_account,
                            self.users[p.pay_to].deposit_ata
                        );
                        prop_assert_eq!(r.scheduled_eligible_at, p.scheduled_eligible_at);
                        prop_assert_eq!(r.eligible_at, p.eligible_at);
                        prop_assert_eq!(r.expires_at, p.expires_at);
                    }
                }
            }
        }
        Ok(())
    }
}

/// `floor`, or `PROPTEST_CASES` when that asks for more. The `proptest!` walks
/// read the variable themselves; these set their count explicitly, which would
/// otherwise override it, and must never drop below the step floor.
fn queue_walk_cases(floor: u32) -> u32 {
    let Ok(raw) = std::env::var("PROPTEST_CASES") else {
        return floor;
    };
    match raw.parse::<u32>() {
        Ok(requested) if requested > floor => requested,
        _ => floor,
    }
}

/// Runs `cases` walks of `QUEUE_WALK_STEPS` ops each, or more when
/// `PROPTEST_CASES` asks for them, and returns how many forgeries per
/// [`TARGETS`] entry tested something across them.
fn run_queue_walk(
    include_yield_ops: bool,
    cases: u32,
    unwind: fn(&mut QueueWalk) -> Result<(), TestCaseError>,
) -> [usize; TARGETS.len()] {
    let forged: [AtomicUsize; TARGETS.len()] = Default::default();
    let mut runner = TestRunner::new(ProptestConfig {
        cases: queue_walk_cases(cases),
        source_file: Some(file!()),
        ..ProptestConfig::default()
    });
    let strategy =
        proptest::collection::vec(queue_op_strategy(include_yield_ops), QUEUE_WALK_STEPS);
    let result = runner.run(&strategy, |ops| {
        let mut walk = QueueWalk::new();
        walk.assert_invariants()?;
        for op in &ops {
            walk.step(op)?;
        }
        for (total, n) in forged.iter().zip(walk.forged) {
            total.fetch_add(n, Ordering::Relaxed);
        }
        unwind(&mut walk)
    });
    if let Err(e) = result {
        panic!("{e}");
    }
    forged.map(|n| n.into_inner())
}

/// The full op mix, vault and queue: every op succeeds exactly when the model
/// says it may, a refused op changes nothing, no op moves a bystander, every
/// finalize pays its recipient what a direct redeem at the same state would,
/// every instruction forged with a decoy account is refused, and the
/// invariants hold after every step.
#[test]
fn queue_walk_invariants_hold_over_ten_thousand_steps() {
    let forged = run_queue_walk(true, QUEUE_WALK_CASES, |_| Ok(()));
    // A target whose honest instruction never succeeds would forge nothing and
    // still pass, so each must have been exercised.
    for (target, n) in TARGETS.iter().zip(forged) {
        assert!(n > 0, "no forged {target:?} tested anything");
    }
    println!("forgeries that tested something: {forged:?}");
}

/// No-yield queue walks: once the operator returns everything, every request is
/// cancelled, the stray is swept back and every depositor exits, they together
/// hold no more of the deposit token than they started with.
#[test]
fn queue_walk_extracts_no_value_without_yield() {
    run_queue_walk(false, 16, |walk| {
        recall_deployed(&mut walk.ctx, &mut walk.model);
        assert_close_refused_while_held(&mut walk.ctx)?;
        for ((who, id), pending) in std::mem::take(&mut walk.live) {
            let user = &walk.users[who];
            let (signer, destination) = (user.keypair.insecure_clone(), user.share_ata);
            walk.ctx
                .cancel_withdrawal_as(&signer, &signer.pubkey(), id, pending.sequence, destination)
                .expect("cancel");
        }
        if walk.stray > 0 {
            let destination = walk.users[0].share_ata;
            walk.ctx.sweep_escrow_shares(destination).expect("sweep");
        }
        if walk.attached {
            walk.ctx.release_vault().expect("release");
        }
        exit_all(&mut walk.ctx, &walk.users)?;
        prop_assert!(
            total_deposit_tokens(&walk.ctx, &walk.users) <= INITIAL_USER_FUNDS * DEPOSITORS as u64,
            "depositors extracted value from a yield-free vault through the queue"
        );
        assert_closes_when_empty(&mut walk.ctx)
    });
}
