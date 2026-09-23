// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Escrow shares and open a request. No vault CPI: the vault is read for its
//! gate and never written, so a request can neither move the share price nor
//! touch the reserve.

use crate::errors::ErrorCode;
use crate::events::WithdrawalRequested;
use crate::recipient::require_valid_recipient;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};
use august_vault::state::vault::VaultState;

/// Checks first, then state, then the transfer. The gate check is decision 13:
/// a request exists only while the vault points at this queue.
pub fn handler(
    ctx: Context<RequestWithdrawal>,
    request_id: u64,
    shares: u64,
    finalizer: Pubkey,
) -> Result<()> {
    let queue_key = ctx.accounts.queue.key();
    require!(
        ctx.accounts.vault_state.withdrawal_queue() == Some(queue_key),
        ErrorCode::QueueNotActiveOnVault
    );
    require!(shares > 0, ErrorCode::ZeroShares);
    require_valid_recipient(&ctx.accounts.recipient_token_account, &ctx.accounts.queue)?;

    let now = Clock::get()?.unix_timestamp;
    let sequence = ctx.accounts.queue.open_request(shares)?;
    let request_key = ctx.accounts.request.key();
    ctx.accounts.request.open(
        queue_key,
        ctx.accounts.owner.key(),
        ctx.accounts.recipient_token_account.key(),
        finalizer,
        shares,
        request_id,
        sequence,
        ctx.bumps.request,
        now,
        ctx.accounts.queue.cooldown_seconds,
        ctx.accounts.queue.fulfillment_window_seconds,
    )?;

    transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.owner_share_account.to_account_info(),
                to: ctx.accounts.escrow_shares.to_account_info(),
                authority: ctx.accounts.owner.to_account_info(),
                mint: ctx.accounts.share_mint.to_account_info(),
            },
        ),
        shares,
        ctx.accounts.share_mint.decimals,
    )?;

    emit!(WithdrawalRequested::snapshot(
        &ctx.accounts.request,
        ctx.accounts.queue.vault_state,
        request_key,
    ));
    Ok(())
}

/// Every deserialized account is boxed; see `InitializeQueue` for why.
#[derive(Accounts)]
#[instruction(request_id: u64)]
pub struct RequestWithdrawal<'info> {
    #[account(
        mut,
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
        has_one = vault_state @ ErrorCode::VaultMismatch,
        has_one = escrow_shares,
        has_one = share_mint,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    pub vault_state: Box<Account<'info, VaultState>>,

    /// Signs the share transfer and pays the request's rent, which returns to
    /// them when the request closes.
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(mut, token::mint = share_mint, token::authority = owner)]
    pub owner_share_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub escrow_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Deposit-mint token account paid at finalization; not an escrow or the
    /// vault reserve.
    pub recipient_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Created here; fails if this owner already has a request with this id.
    #[account(
        init,
        payer = owner,
        space = WithdrawalRequest::LEN,
        seeds = [
            WITHDRAWAL_REQUEST_SEED,
            queue.key().as_ref(),
            owner.key().as_ref(),
            &request_id.to_le_bytes(),
        ],
        bump,
    )]
    pub request: Box<Account<'info, WithdrawalRequest>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}
