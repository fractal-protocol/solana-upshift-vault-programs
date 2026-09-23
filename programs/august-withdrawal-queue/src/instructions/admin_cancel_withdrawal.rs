// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! The admin returns a request's shares to its owner without the owner's
//! involvement, so abandoned escrow cannot hold shares in supply forever and
//! block `close_vault`. Only once the vault has been released for
//! `ADMIN_CANCEL_DELAY_SECONDS`: while the queue is live no one but the owner
//! may touch a request, and the delay guarantees every owner a real day of
//! instant redemption first. Without it, release, cancel and re-attach would
//! fit in one transaction and reset a waiting user.

use crate::auth::require_vault_admin;
use crate::errors::ErrorCode;
use crate::instructions::cancel_withdrawal::return_shares;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};
use august_vault::state::vault::VaultState;

/// Admin, released vault, delay elapsed, matching stamp; then the same settlement as the
/// owner's cancel, with the admin named in the event.
pub fn handler(ctx: Context<AdminCancelWithdrawal>, expected_sequence: u64) -> Result<()> {
    require_vault_admin(
        &ctx.accounts.queue,
        &ctx.accounts.vault_state,
        &ctx.accounts.admin,
    )?;
    require!(
        ctx.accounts.vault_state.withdrawal_queue() != Some(ctx.accounts.queue.key()),
        ErrorCode::QueueStillAttached
    );
    let now = Clock::get()?.unix_timestamp;
    require!(
        ctx.accounts.queue.admin_cancel_delay_elapsed(now)?,
        ErrorCode::AdminCancelTooEarly
    );
    require!(
        ctx.accounts.request.sequence == expected_sequence,
        ErrorCode::StaleRequestSequence
    );
    let event = return_shares(
        &mut ctx.accounts.queue,
        &ctx.accounts.request,
        ctx.accounts.escrow_shares.to_account_info(),
        ctx.accounts.destination_share_account.to_account_info(),
        &ctx.accounts.share_mint,
        ctx.accounts.token_program.to_account_info(),
        ctx.accounts.admin.key(),
    )?;
    emit_cpi!(event);
    Ok(())
}

/// Every deserialized account is boxed; see `InitializeQueue` for why.
///
/// The destination is any share account whose authority is the request's owner,
/// as for the owner's own cancel; it must exist. An owner who gave their ATA
/// away cannot block this: the admin creates a plain token account with the
/// owner as authority, which anyone may do for any owner, in the same
/// transaction and pays for it.
#[event_cpi]
#[derive(Accounts)]
pub struct AdminCancelWithdrawal<'info> {
    #[account(
        mut,
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
        has_one = vault_state @ ErrorCode::VaultMismatch,
        has_one = escrow_shares,
        has_one = share_mint,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    /// Read for its admin and its authority, never written.
    pub vault_state: Box<Account<'info, VaultState>>,

    pub admin: Signer<'info>,

    /// Closed with its rent to `owner`.
    #[account(
        mut,
        close = owner,
        seeds = [
            WITHDRAWAL_REQUEST_SEED,
            request.queue.as_ref(),
            request.owner.as_ref(),
            &request.request_id.to_le_bytes(),
        ],
        bump = request.bump,
        has_one = queue,
        has_one = owner,
    )]
    pub request: Box<Account<'info, WithdrawalRequest>>,

    /// CHECK: the request's owner, bound by `has_one`. Receives the rent and
    /// need not sign.
    #[account(mut)]
    pub owner: UncheckedAccount<'info>,

    #[account(mut)]
    pub escrow_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut, token::mint = share_mint, token::authority = owner)]
    pub destination_share_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}
