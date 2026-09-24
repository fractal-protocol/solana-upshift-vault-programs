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
        ctx.accounts.owner.key(),
    )?;
    emit_cpi!(event);
    Ok(())
}

/// The cancel settlement: the counters drop and the escrowed shares
/// move to `destination` under the queue's signature. Returns the event naming
/// `by` for the caller to emit, since `emit_cpi!` needs the caller's `ctx`. The
/// caller's `close = owner` constraint returns the rent on exit.
pub(crate) fn return_shares<'info>(
    queue: &mut Account<'info, WithdrawalQueue>,
    request: &Account<'info, WithdrawalRequest>,
    escrow_shares: AccountInfo<'info>,
    destination: AccountInfo<'info>,
    share_mint: &InterfaceAccount<'info, Mint>,
    token_program: AccountInfo<'info>,
    by: Pubkey,
) -> Result<WithdrawalCancelled> {
    let shares = request.shares;
    queue.close_request(shares)?;

    let seeds = queue.signer_seeds();
    let destination_key = destination.key();
    transfer_checked(
        CpiContext::new_with_signer(
            token_program,
            TransferChecked {
                from: escrow_shares,
                to: destination,
                authority: queue.to_account_info(),
                mint: share_mint.to_account_info(),
            },
            &[&seeds],
        ),
        shares,
        share_mint.decimals,
    )?;

    Ok(WithdrawalCancelled::snapshot(
        request,
        queue.vault_state,
        request.key(),
        by,
        destination_key,
    ))
}

/// Every deserialized account is boxed; see `InitializeQueue` for why.
///
/// The destination is any share account whose authority is the owner, not
/// necessarily their ATA: classic SPL lets an owner reassign an ATA's authority,
/// and a cancel that insisted on the ATA would then be blocked by the owner's
/// own action. It must already exist. The SDK prepends the idempotent ATA
/// create to the same transaction, so the owner pays for it exactly when it is
/// missing.
#[event_cpi]
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

    /// Seeds come from its own stored fields rather than the signer, so a wrong
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
