// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Design decision 15: the admin or operator moves one request's eligibility to
//! now. Only ever widens the owner's window: `scheduled_eligible_at` and
//! `expires_at` are never touched, so it cannot revive a lapsed request or
//! shorten a deadline. Discretionary early-exit authority with no aggregate
//! cap, made auditable by the event rather than prevented. It grants the owner
//! an option, not a settlement: until `scheduled_eligible_at`, finalize accepts
//! only the owner or their named finalizer, so the operator cannot mark the
//! price down, expedite and settle a holder in one transaction.

use crate::auth::require_vault_admin_or_operator;
use crate::batch;
use crate::errors::ErrorCode;
use crate::events::WithdrawalExpedited;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_lang::AccountsExit;
use august_vault::state::vault::VaultState;

/// Moves `eligible_at` to `now` for a pending request that has not reached it.
/// An expired request is refused by name; an already-eligible one has nothing
/// to gain. Returns the previous `eligible_at` for the event.
fn expedite_one(request: &mut WithdrawalRequest, now: i64, expected_sequence: u64) -> Result<i64> {
    require!(
        request.sequence == expected_sequence,
        ErrorCode::StaleRequestSequence
    );
    require!(!request.is_expired(now), ErrorCode::RequestExpired);
    require!(!request.is_eligible(now), ErrorCode::RequestAlreadyEligible);
    let previous = request.eligible_at;
    request.eligible_at = now;
    Ok(previous)
}

pub fn handler(ctx: Context<ExpediteRequest>, expected_sequence: u64) -> Result<()> {
    require_vault_admin_or_operator(
        &ctx.accounts.queue,
        &ctx.accounts.vault_state,
        &ctx.accounts.authority,
    )?;
    let now = Clock::get()?.unix_timestamp;
    let previous = expedite_one(&mut ctx.accounts.request, now, expected_sequence)?;
    emit_cpi!(WithdrawalExpedited::snapshot(
        &ctx.accounts.request,
        ctx.accounts.queue.vault_state,
        ctx.accounts.request.key(),
        ctx.accounts.authority.key(),
        previous,
    ));
    Ok(())
}

/// The batch form: one trailing request account per sequence, ascending, all
/// or nothing. Each request is loaded, checked and written back by hand, since
/// trailing accounts are not declared fields.
pub fn handler_batch<'info>(
    ctx: Context<'_, '_, 'info, 'info, ExpediteRequests<'info>>,
    expected_sequences: Vec<u64>,
) -> Result<()> {
    require_vault_admin_or_operator(
        &ctx.accounts.queue,
        &ctx.accounts.vault_state,
        &ctx.accounts.authority,
    )?;
    let now = Clock::get()?.unix_timestamp;
    let queue_key = ctx.accounts.queue.key();
    let groups = batch::groups(
        ctx.remaining_accounts,
        &expected_sequences,
        1,
        batch::MAX_EXPEDITE_BATCH,
    )?;
    for (group, expected_sequence) in groups.into_iter().zip(expected_sequences) {
        let info = &group[0];
        batch::announce(info.key);
        let mut request = batch::load_request(info, &queue_key)?;
        let previous = expedite_one(&mut request, now, expected_sequence)?;
        request.exit(&crate::ID)?;
        emit_cpi!(WithdrawalExpedited::snapshot(
            &request,
            ctx.accounts.queue.vault_state,
            *info.key,
            ctx.accounts.authority.key(),
            previous,
        ));
    }
    Ok(())
}

/// The admin-or-operator check binds `vault_state` to the queue; the queue's
/// seeds bind the queue. Boxed as everywhere.
#[event_cpi]
#[derive(Accounts)]
pub struct ExpediteRequest<'info> {
    #[account(
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
        has_one = vault_state @ ErrorCode::VaultMismatch,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    pub vault_state: Box<Account<'info, VaultState>>,

    /// The vault's admin or operator.
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [
            WITHDRAWAL_REQUEST_SEED,
            request.queue.as_ref(),
            request.owner.as_ref(),
            &request.request_id.to_le_bytes(),
        ],
        bump = request.bump,
        has_one = queue,
    )]
    pub request: Box<Account<'info, WithdrawalRequest>>,
}

/// As `ExpediteRequest`, with the requests as trailing accounts.
#[event_cpi]
#[derive(Accounts)]
pub struct ExpediteRequests<'info> {
    #[account(
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
        has_one = vault_state @ ErrorCode::VaultMismatch,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    pub vault_state: Box<Account<'info, VaultState>>,

    /// The vault's admin or operator.
    pub authority: Signer<'info>,
}
