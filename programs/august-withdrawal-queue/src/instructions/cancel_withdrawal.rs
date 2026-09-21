// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! The owner takes a pending request back: escrowed shares return to a share
//! account they control and the request closes. No vault account is involved,
//! so it works while the vault is paused and whether or not the queue is
//! attached: nothing escrowed is ever stranded.

use crate::errors::ErrorCode;
use crate::events::WithdrawalCancelled;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

/// Any time while the request exists: before eligibility, after expiry, in
/// either vault state. `expected_sequence` must match, as everywhere. Counters
/// drop before the transfer; the account closes on exit with its rent to the
/// owner.
pub fn handler(ctx: Context<CancelWithdrawal>, expected_sequence: u64) -> Result<()> {
    let request = &ctx.accounts.request;
    require!(
        request.sequence == expected_sequence,
        ErrorCode::StaleRequestSequence
    );
    let shares = request.shares;
    ctx.accounts.queue.close_request(shares)?;

    let seeds = ctx.accounts.queue.signer_seeds();
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.escrow_shares.to_account_info(),
                to: ctx.accounts.destination_share_account.to_account_info(),
                authority: ctx.accounts.queue.to_account_info(),
                mint: ctx.accounts.share_mint.to_account_info(),
            },
            &[&seeds],
        ),
        shares,
        ctx.accounts.share_mint.decimals,
    )?;

    emit!(WithdrawalCancelled::snapshot(
        &ctx.accounts.request,
        ctx.accounts.queue.vault_state,
        ctx.accounts.request.key(),
        ctx.accounts.owner.key(),
        ctx.accounts.destination_share_account.key(),
    ));
    Ok(())
}

/// Every deserialized account is boxed; see `InitializeQueue` for why.
///
/// The destination is any share account whose authority is the owner, not
/// necessarily their ATA: classic SPL lets an owner reassign an ATA's authority,
/// and a cancel that insisted on the ATA would then be blocked by the owner's
/// own action. It must already exist. The SDK prepends the idempotent ATA
/// create to the same transaction, so the owner pays for it exactly when it is
/// missing.
#[derive(Accounts)]
pub struct CancelWithdrawal<'info> {
    #[account(
        mut,
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
        has_one = escrow_shares,
        has_one = share_mint,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    /// Signs, and receives the request's rent.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// Seeds come from its own stored fields, as in `UpdateRequest`, so a wrong
    /// signer is reported as `NotRequestOwner`.
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
        has_one = owner @ ErrorCode::NotRequestOwner,
    )]
    pub request: Box<Account<'info, WithdrawalRequest>>,

    #[account(mut)]
    pub escrow_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(mut, token::mint = share_mint, token::authority = owner)]
    pub destination_share_account: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}
