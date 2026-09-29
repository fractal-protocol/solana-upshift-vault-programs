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

use std::collections::BTreeMap;

use august_vault::state::vault::FEE_RATE_DENOMINATOR_VALUE;
use august_withdrawal_queue::errors::{ErrorCode as QueueError, ANCHOR_USER_ERROR_OFFSET};
use integration_tests::harness::{
    assert_anchor_framework_err, expected_withdrawal_fee, CeiSnapshot, VaultCtx,
};
use proptest::prelude::*;
use proptest::test_runner::TestRunner;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};

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

fn fresh_walk_vault() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(INITIAL_USER_FUNDS);
    ctx.deposit(SEED_DEPOSIT).expect("seed deposit");
    ctx
}

/// What one executed op tells the invariant checks.
struct OpOutcome {
    /// Every observable state slot immediately before the op ran.
    before: CeiSnapshot,
    /// Withdrawal fee stored at execution time (a `SetFee` earlier in the walk
    /// may have moved it).
    fee_rate: u32,
    /// Whether the transaction succeeded.
    ok: bool,
    /// Whether this op was a redeem, i.e. whether the value-conservation and
    /// fee-rounding checks apply.
    is_redeem: bool,
}

/// Execute one op against the live vault.
fn execute(ctx: &mut VaultCtx, op: &Op) -> OpOutcome {
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
    OpOutcome {
        before,
        fee_rate,
        ok,
        is_redeem,
    }
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

/// A successful redeem paid the user: the vault's outflow is exactly the fee
/// plus the user's receipt, and the fee is `ceil(assets * fee / 1e6)`. Returns
/// what the user received.
fn assert_redeem_conserves(
    before: &CeiSnapshot,
    after: &CeiSnapshot,
    fee_rate: u32,
) -> Result<u64, TestCaseError> {
    let assets = decrease(before.vault_tokens, after.vault_tokens, "vault balance")?;
    let fee_paid = increase(
        before.fee_recipient_tokens,
        after.fee_recipient_tokens,
        "fee recipient balance",
    )?;
    let user_received = increase(before.user_deposit, after.user_deposit, "user balance")?;
    let paid_out = fee_paid
        .checked_add(user_received)
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
    Ok(user_received)
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
            let outcome = execute(&mut ctx, op);

            if !outcome.ok {
                prop_assert_eq!(
                    ctx.snapshot(), outcome.before,
                    "failed {:?} left side effects", op
                );
                continue;
            }

            assert_state_invariants(&ctx)?;

            if outcome.is_redeem {
                assert_redeem_conserves(&outcome.before, &ctx.snapshot(), outcome.fee_rate)?;
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
            ctx.operator_deposit(operator_balance).expect("operator returns all");
        }
        // …and the user exits their entire position.
        let shares = ctx.token_account_amount(&ctx.user_share_ata);
        if shares > 0 {
            ctx.redeem(shares).expect("full exit after operator returned all");
        }

        prop_assert!(
            ctx.token_account_amount(&ctx.user_deposit_ata) <= INITIAL_USER_FUNDS,
            "user extracted value from a yield-free vault"
        );
    }
}

// ---- the queue walk (WQ-10) ----

/// Request ids come from a small pool, so a walk both collides with a live id
/// and reuses one after it closes, which is what the sequence stamp guards.
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
}

#[derive(Debug, Clone)]
enum QueueOp {
    Vault(Op),
    /// User escrows this many per-mille of their shares under `id`, naming the
    /// keeper as the only other finalizer or leaving finalization open.
    Request {
        id: u64,
        pm: u16,
        keeper_only: bool,
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
    Cancel {
        pick: usize,
    },
    Expedite {
        pick: usize,
        by_operator: bool,
    },
    Warp(u64),
    /// User sends this many per-mille of their shares straight to the escrow.
    Stray(u16),
    Sweep,
    SetCooldown(u64),
    SetWindow(u64),
    Release,
    Attach,
}

fn queue_op_strategy(include_yield_ops: bool) -> BoxedStrategy<QueueOp> {
    let caller = prop_oneof![
        Just(Caller::Owner),
        Just(Caller::Keeper),
        Just(Caller::Stranger)
    ];
    let queue = prop_oneof![
        3 => (0..REQUEST_ID_POOL, 0u16..=1000, any::<bool>())
            .prop_map(|(id, pm, keeper_only)| QueueOp::Request { id, pm, keeper_only }),
        3 => (any::<usize>(), caller).prop_map(|(pick, caller)| QueueOp::Finalize { pick, caller }),
        1 => any::<usize>().prop_map(|pick| QueueOp::FinalizeStale { pick }),
        1 => any::<usize>().prop_map(|pick| QueueOp::Cancel { pick }),
        1 => (any::<usize>(), any::<bool>())
            .prop_map(|(pick, by_operator)| QueueOp::Expedite { pick, by_operator }),
        2 => (0u64..=2 * DAY).prop_map(QueueOp::Warp),
        1 => (0u16..=50).prop_map(QueueOp::Stray),
        1 => Just(QueueOp::Sweep),
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
    scheduled_eligible_at: i64,
    eligible_at: i64,
    expires_at: i64,
}

/// Every account a queue op may write, raw, so that a refused op can be shown
/// to have changed nothing.
#[derive(Debug, PartialEq, Eq)]
struct QueueSnapshot {
    vault: CeiSnapshot,
    queue: Vec<u8>,
    escrow_shares: u64,
    escrow_assets: u64,
    requests: Vec<Option<Vec<u8>>>,
}

struct QueueWalk {
    ctx: VaultCtx,
    keeper: Keypair,
    stranger: Keypair,
    live: BTreeMap<u64, Pending>,
    sequence: u64,
    /// Shares sent to the escrow outside a request, which only a sweep removes.
    stray: u64,
    attached: bool,
}

impl QueueWalk {
    fn new() -> Self {
        let mut ctx = fresh_walk_vault();
        ctx.open_queue(INITIAL_COOLDOWN);
        let keeper = ctx.new_funded_keypair(1_000_000_000);
        let stranger = ctx.new_funded_keypair(1_000_000_000);
        QueueWalk {
            ctx,
            keeper,
            stranger,
            live: BTreeMap::new(),
            sequence: 0,
            stray: 0,
            attached: true,
        }
    }

    fn user(&self) -> Pubkey {
        self.ctx.user.pubkey()
    }

    fn pick(&self, pick: usize) -> Option<(u64, Pending)> {
        if self.live.is_empty() {
            return None;
        }
        let (id, pending) = self.live.iter().nth(pick % self.live.len())?;
        Some((*id, *pending))
    }

    /// A request account's data, or `None` once it is closed.
    fn request_account(&self, id: u64) -> Option<Vec<u8>> {
        let account = self
            .ctx
            .svm
            .get_account(&self.ctx.request_pda(&self.user(), id))?;
        if account.lamports == 0 {
            return None;
        }
        Some(account.data)
    }

    fn snapshot(&self) -> QueueSnapshot {
        let queue = self
            .ctx
            .svm
            .get_account(&self.ctx.withdrawal_queue_pda())
            .expect("queue account exists");
        QueueSnapshot {
            vault: self.ctx.snapshot(),
            queue: queue.data,
            escrow_shares: self.escrow_shares(),
            escrow_assets: self.escrow_assets(),
            requests: (0..REQUEST_ID_POOL)
                .map(|id| self.request_account(id))
                .collect(),
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

    fn user_shares(&self) -> u64 {
        self.ctx.token_account_amount(&self.ctx.user_share_ata)
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
        let user = self.ctx.user.insecure_clone();
        match *op {
            QueueOp::Vault(ref op) => {
                let outcome = execute(&mut self.ctx, op);
                if outcome.ok && outcome.is_redeem {
                    prop_assert!(
                        !self.attached,
                        "a direct redeem got past the attached queue"
                    );
                    assert_redeem_conserves(
                        &outcome.before,
                        &self.ctx.snapshot(),
                        outcome.fee_rate,
                    )?;
                }
                Ok(outcome.ok)
            }
            QueueOp::Request {
                id,
                pm,
                keeper_only,
            } => {
                let shares = per_mille(self.user_shares(), pm);
                let finalizer = if keeper_only {
                    self.keeper.pubkey()
                } else {
                    Pubkey::default()
                };
                let queue = self.ctx.queue_state_data();
                let now = self.ctx.now();
                let before = self.user_shares();
                let (share_ata, deposit_ata) = (self.ctx.user_share_ata, self.ctx.user_deposit_ata);
                let ok = self
                    .ctx
                    .request_withdrawal_as(&user, share_ata, deposit_ata, id, shares, finalizer)
                    .is_ok();
                let expected = self.attached && shares > 0 && !self.live.contains_key(&id);
                prop_assert_eq!(ok, expected, "request {} of {} shares", id, shares);
                if ok {
                    prop_assert_eq!(
                        before - self.user_shares(),
                        shares,
                        "request escrowed the wrong amount"
                    );
                    self.sequence += 1;
                    let scheduled = now + queue.cooldown_seconds as i64;
                    let expires_at = if queue.fulfillment_window_seconds == 0 {
                        0
                    } else {
                        scheduled + queue.fulfillment_window_seconds as i64
                    };
                    self.live.insert(
                        id,
                        Pending {
                            sequence: self.sequence,
                            shares,
                            keeper_only,
                            scheduled_eligible_at: scheduled,
                            eligible_at: scheduled,
                            expires_at,
                        },
                    );
                }
                Ok(ok)
            }
            QueueOp::Finalize { pick, caller } => {
                let Some((id, pending)) = self.pick(pick) else {
                    return Ok(true);
                };
                let signer = match caller {
                    Caller::Owner => user.insecure_clone(),
                    Caller::Keeper => self.keeper.insecure_clone(),
                    Caller::Stranger => self.stranger.insecure_clone(),
                };
                let permitted = match caller {
                    Caller::Owner | Caller::Keeper => true,
                    Caller::Stranger => !pending.keeper_only,
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
                let owner = self.user();
                let ok = self
                    .ctx
                    .finalize_withdrawal_as(&signer, &owner, id, pending.sequence)
                    .is_ok();
                prop_assert_eq!(
                    ok,
                    expected,
                    "finalize {:?} by {:?} at {}",
                    pending,
                    caller,
                    now
                );
                if ok {
                    let after = self.ctx.snapshot();
                    let received = assert_redeem_conserves(&before, &after, state.withdrawal_fee)?;
                    prop_assert_eq!(
                        received,
                        net,
                        "queue paid {} where a direct redeem at the same state pays {}",
                        received,
                        net
                    );
                    prop_assert_eq!(after.user_shares, before.user_shares);
                    self.live.remove(&id);
                }
                Ok(ok)
            }
            QueueOp::FinalizeStale { pick } => {
                let Some((id, pending)) = self.pick(pick) else {
                    return Ok(true);
                };
                let owner = self.user();
                let err = self
                    .ctx
                    .finalize_withdrawal_as(&user, &owner, id, pending.sequence - 1)
                    .expect_err("finalize with a stale stamp");
                assert_anchor_framework_err(
                    &err,
                    QueueError::StaleRequestSequence as u32 + ANCHOR_USER_ERROR_OFFSET,
                );
                Ok(false)
            }
            QueueOp::Cancel { pick } => {
                let Some((id, pending)) = self.pick(pick) else {
                    return Ok(true);
                };
                let before = self.user_shares();
                self.ctx
                    .cancel_withdrawal(id, pending.sequence)
                    .map_err(|e| TestCaseError::fail(format!("cancel refused: {e:?}")))?;
                prop_assert_eq!(
                    self.user_shares() - before,
                    pending.shares,
                    "cancel returned the wrong amount"
                );
                self.live.remove(&id);
                Ok(true)
            }
            QueueOp::Expedite { pick, by_operator } => {
                let Some((id, pending)) = self.pick(pick) else {
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
                let owner = self.user();
                let ok = self
                    .ctx
                    .expedite_request_as(&authority, &owner, id, pending.sequence)
                    .is_ok();
                prop_assert_eq!(ok, expected, "expedite {:?} at {}", pending, now);
                if ok {
                    self.live.get_mut(&id).expect("picked").eligible_at = now;
                }
                Ok(ok)
            }
            QueueOp::Warp(secs) => {
                self.ctx.warp_forward_seconds(secs as i64);
                Ok(true)
            }
            QueueOp::Stray(pm) => {
                let amount = per_mille(self.user_shares(), pm);
                let (mint, from) = (self.ctx.share_mint, self.ctx.user_share_ata);
                let escrow = self.ctx.queue_escrow(&mint);
                self.ctx
                    .transfer_mint_tokens_as(&user, &mint, &from, &escrow, amount);
                self.stray += amount;
                Ok(true)
            }
            QueueOp::Sweep => {
                let before = self.user_shares();
                let destination = self.ctx.user_share_ata;
                let ok = self.ctx.sweep_escrow_shares(destination).is_ok();
                prop_assert_eq!(ok, self.stray > 0, "sweep with {} stray", self.stray);
                if ok {
                    prop_assert_eq!(
                        self.user_shares() - before,
                        self.stray,
                        "sweep moved the wrong amount"
                    );
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
        prop_assert_eq!(
            self.ctx.share_mint_supply(),
            self.user_shares() + self.escrow_shares(),
            "share supply drifted from the user's and the escrow's balances"
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

        for id in 0..REQUEST_ID_POOL {
            match self.live.get(&id) {
                None => prop_assert!(
                    self.request_account(id).is_none(),
                    "request {} not closed",
                    id
                ),
                Some(p) => {
                    let r = self.ctx.request_state_data(&self.user(), id);
                    prop_assert_eq!(r.sequence, p.sequence);
                    prop_assert_eq!(r.shares, p.shares);
                    prop_assert_eq!(r.scheduled_eligible_at, p.scheduled_eligible_at);
                    prop_assert_eq!(r.eligible_at, p.eligible_at);
                    prop_assert_eq!(r.expires_at, p.expires_at);
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
/// says it may, a refused op changes nothing, every finalize pays what a direct
/// redeem at the same state would, and the invariants hold after every step.
#[test]
fn queue_walk_invariants_hold_over_ten_thousand_steps() {
    run_queue_walk(true, QUEUE_WALK_CASES, |_| Ok(()));
}

/// No-yield queue walks: once the operator returns everything, every request is
/// cancelled, the stray is swept back and the user exits, they hold no more of
/// the deposit token than they started with.
#[test]
fn queue_walk_extracts_no_value_without_yield() {
    run_queue_walk(false, 16, |walk| {
        let ctx = &mut walk.ctx;
        let operator_balance = ctx.token_account_amount(&ctx.operator_deposit_ata);
        if operator_balance > 0 {
            ctx.operator_deposit(operator_balance)
                .expect("operator returns all");
        }
        for (id, pending) in std::mem::take(&mut walk.live) {
            walk.ctx
                .cancel_withdrawal(id, pending.sequence)
                .expect("cancel");
        }
        if walk.stray > 0 {
            let destination = walk.ctx.user_share_ata;
            walk.ctx.sweep_escrow_shares(destination).expect("sweep");
        }
        if walk.attached {
            walk.ctx.release_vault().expect("release");
        }
        let shares = walk.user_shares();
        if shares > 0 {
            walk.ctx
                .redeem(shares)
                .expect("full exit after operator returned all");
        }
        prop_assert!(
            walk.ctx.token_account_amount(&walk.ctx.user_deposit_ata) <= INITIAL_USER_FUNDS,
            "user extracted value from a yield-free vault through the queue"
        );
        Ok(())
    });
}
