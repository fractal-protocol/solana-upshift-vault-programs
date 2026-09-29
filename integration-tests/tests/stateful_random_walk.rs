//! Stateful property tests: random sequences of deposits/redeems by several
//! depositors, operator withdraw/return/AUM reports, and admin fee changes are
//! executed against a real LiteSVM vault, checking accounting invariants after
//! every step. This addresses the due-diligence recommendation for property
//! testing over "AUM and fee arithmetic, and stateful sequences of deposits,
//! operator actions, and redemptions".
//!
//! Invariants checked after every operation:
//! - `local_aum` equals the vault token account's actual balance (no
//!   accounting drift, with or without fees).
//! - Share-mint supply equals the depositors' share balances combined (nothing
//!   else mints or burns).
//! - Any failed operation leaves every observable state slot unchanged
//!   (transaction-level atomicity backing the CEI audit finding).
//! - A successful operation moves no balance of a depositor it does not
//!   involve.
//! - Every successful redeem conserves value exactly: the vault pays out
//!   `assets`, the fee recipient receives `ceil(assets * fee / 1e6)`, and the
//!   redeemer receives the remainder.
//! - Every successful deposit mints, and every redeem pays, exactly what the
//!   vault's conversion math quotes on the state before it.
//!
//! A second walk (no AUM reports, operator returns everything at the end)
//! checks the economic end-state property: with no yield injected, the
//! depositors together can never withdraw more than they deposited. Not each
//! one alone: a redeem's rounding dust stays in the vault and lifts the price
//! for whoever remains.
//!
//! Case count is deliberately modest (each case boots a fresh SVM); override
//! with PROPTEST_CASES for a deeper local search.

use std::collections::BTreeMap;

use august_vault::errors::ErrorCode as VaultError;
use august_vault::state::vault::FEE_RATE_DENOMINATOR_VALUE;
use august_withdrawal_queue::errors::{ErrorCode as QueueError, ANCHOR_USER_ERROR_OFFSET};
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, expected_withdrawal_fee, CeiSnapshot,
    Depositor, VaultCtx,
};
use proptest::prelude::*;
use proptest::test_runner::TestRunner;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};

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
        (0..DEPOSITORS, 1u64..20_000_000_000)
            .prop_map(|(who, amount)| Op::UserDeposit { who, amount }),
        (0..DEPOSITORS, 0u16..=1000).prop_map(|(who, pm)| Op::UserRedeem { who, pm }),
        (0u16..=1000).prop_map(Op::OperatorWithdraw),
        (0u16..=1000).prop_map(Op::OperatorReturn),
        (0u32..FEE_RATE_DENOMINATOR_VALUE / 10).prop_map(Op::SetFee),
    ];
    if include_yield_ops {
        // `prop_oneof!` weights are per-arm at the level they appear, so a bare
        // `prop_oneof![base, aum]` would hand AUM reports half of every walk
        // and starve the deposit/redeem interleavings. Weighting `base` by its
        // arm count keeps all six ops equally likely.
        prop_oneof![5 => base, 1 => (-20i8..=20).prop_map(Op::UpdateAum)].boxed()
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

/// A fresh vault and its depositors. The first is the harness's own user, who
/// makes the seed deposit; the rest start with the same funds and no shares.
fn fresh_walk_vault() -> (VaultCtx, Vec<Depositor>) {
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
    (ctx, users)
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

/// Execute one op against the live vault.
fn execute(ctx: &mut VaultCtx, users: &[Depositor], op: &Op) -> OpOutcome {
    let before = ctx.snapshot();
    let holdings_before = holdings(ctx, users);
    let state = ctx.vault_state_data();
    let fee_rate = state.withdrawal_fee;
    let supply = ctx.share_mint_supply();
    let total_assets = state.total_assets().expect("total assets");
    let mut quote = None;

    let (ok, actor, is_redeem) = match *op {
        Op::UserDeposit { who, amount } => {
            if let Ok(shares) = state.shares_for_deposit(supply, total_assets, amount) {
                quote = Some(shares);
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
            (ctx.redeem_as(&users[who], shares).is_ok(), Some(who), true)
        }
        Op::OperatorWithdraw(pm) => {
            let amount = per_mille(ctx.vault_state_data().local_aum, pm);
            (ctx.operator_withdraw(amount).is_ok(), None, false)
        }
        Op::OperatorReturn(pm) => {
            let amount = per_mille(ctx.token_account_amount(&ctx.operator_deposit_ata), pm);
            (ctx.operator_deposit(amount).is_ok(), None, false)
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
            (ctx.operator_update_aum(new_aum).is_ok(), None, false)
        }
        Op::SetFee(fee) => (ctx.set_withdrawal_fee(fee).is_ok(), None, false),
    };
    OpOutcome {
        before,
        holdings: holdings_before,
        fee_rate,
        ok,
        actor,
        is_redeem,
        quote,
    }
}

/// Accounting invariants that must hold in every reachable state.
fn assert_state_invariants(ctx: &VaultCtx, users: &[Depositor]) -> Result<(), TestCaseError> {
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
    Ok(())
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
        Op::UserDeposit { who, amount } => {
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
        Op::UserRedeem { who, .. } => {
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

    /// Full op mix (including in-window AUM reports): accounting invariants
    /// hold after every step; failures are perfectly atomic; no op moves a
    /// bystander; every successful redeem conserves value with exact
    /// ceil-rounded fees.
    #[test]
    fn invariants_hold_under_random_op_sequences(
        ops in proptest::collection::vec(op_strategy(true), 1..25)
    ) {
        let (mut ctx, users) = fresh_walk_vault();
        assert_state_invariants(&ctx, &users)?;

        for op in &ops {
            let outcome = execute(&mut ctx, &users, op);

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

            assert_state_invariants(&ctx, &users)?;
            assert_vault_op_effects(&ctx, &users, op, &outcome)?;
        }
    }

    /// No-yield walks (AUM reports excluded): after the operator returns its
    /// full balance and every depositor exits completely, they cannot together
    /// hold more of the deposit token than they started with — rounding and
    /// fees only ever favour the vault.
    #[test]
    fn no_value_extraction_without_yield(
        ops in proptest::collection::vec(op_strategy(false), 1..25)
    ) {
        let (mut ctx, users) = fresh_walk_vault();

        for op in &ops {
            execute(&mut ctx, &users, op); // failures are fine; atomicity checked above
        }

        // Unwind: operator returns everything it took…
        let operator_balance = ctx.token_account_amount(&ctx.operator_deposit_ata);
        if operator_balance > 0 {
            ctx.operator_deposit(operator_balance).expect("operator returns all");
        }
        // …and every depositor exits their entire position.
        exit_all(&mut ctx, &users)?;

        prop_assert!(
            total_deposit_tokens(&ctx, &users) <= INITIAL_USER_FUNDS * DEPOSITORS as u64,
            "depositors extracted value from a yield-free vault"
        );
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

struct QueueWalk {
    ctx: VaultCtx,
    users: Vec<Depositor>,
    keeper: Keypair,
    stranger: Keypair,
    /// Live requests by (owner index, request id).
    live: BTreeMap<(usize, u64), Pending>,
    sequence: u64,
    /// Shares sent to the escrow outside a request, which only a sweep removes.
    stray: u64,
    attached: bool,
}

impl QueueWalk {
    fn new() -> Self {
        let (mut ctx, users) = fresh_walk_vault();
        ctx.open_queue(INITIAL_COOLDOWN);
        let keeper = ctx.new_funded_keypair(1_000_000_000);
        let stranger = ctx.new_funded_keypair(1_000_000_000);
        QueueWalk {
            ctx,
            users,
            keeper,
            stranger,
            live: BTreeMap::new(),
            sequence: 0,
            stray: 0,
            attached: true,
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
                let outcome = execute(&mut self.ctx, &self.users, op);
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
        }
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
/// `PROPTEST_CASES` asks for them.
fn run_queue_walk(
    include_yield_ops: bool,
    cases: u32,
    unwind: fn(&mut QueueWalk) -> Result<(), TestCaseError>,
) {
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
        unwind(&mut walk)
    });
    if let Err(e) = result {
        panic!("{e}");
    }
}

/// The full op mix, vault and queue: every op succeeds exactly when the model
/// says it may, a refused op changes nothing, no op moves a bystander, every
/// finalize pays its recipient what a direct redeem at the same state would,
/// and the invariants hold after every step.
#[test]
fn queue_walk_invariants_hold_over_ten_thousand_steps() {
    run_queue_walk(true, QUEUE_WALK_CASES, |_| Ok(()));
}

/// No-yield queue walks: once the operator returns everything, every request is
/// cancelled, the stray is swept back and every depositor exits, they together
/// hold no more of the deposit token than they started with.
#[test]
fn queue_walk_extracts_no_value_without_yield() {
    run_queue_walk(false, 16, |walk| {
        let ctx = &mut walk.ctx;
        let operator_balance = ctx.token_account_amount(&ctx.operator_deposit_ata);
        if operator_balance > 0 {
            ctx.operator_deposit(operator_balance)
                .expect("operator returns all");
        }
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
        Ok(())
    });
}
