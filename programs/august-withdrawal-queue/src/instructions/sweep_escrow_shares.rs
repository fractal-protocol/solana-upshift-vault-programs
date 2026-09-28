// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Move shares sent to the escrow that no request owns to an account the admin
//! names. Anyone can transfer shares into `escrow_shares`, and nothing else ever
//! moves them out, so without this they would be stuck there for good: lost to
//! whoever sent them by mistake, and a stray share would keep `close_vault`,
//! which needs a zero supply, from ever running (audit L-2). Moving them rather
//! than burning them lets the admin return them to the sender, instead of
//! spreading their value across every holder.

use crate::auth::require_vault_admin;
use crate::errors::ErrorCode;
use crate::events::EscrowSwept;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};
use august_vault::state::vault::VaultState;

/// Admin only. Moves `escrow_shares.amount - pending_shares`, the part of the
/// escrow no pending request accounts for, to `destination`, signed by the
/// queue PDA as the escrow's owner. Pending requests' shares are never touched.
pub fn handler(ctx: Context<SweepEscrowShares>) -> Result<()> {
    require_vault_admin(
        &ctx.accounts.queue,
        &ctx.accounts.vault_state,
        &ctx.accounts.admin,
    )?;
    let queue = &ctx.accounts.queue;
    let stray = ctx
        .accounts
        .escrow_shares
        .amount
        .checked_sub(queue.pending_shares)
        .ok_or(ErrorCode::MathError)?;
    require!(stray > 0, ErrorCode::NothingToSweep);

    let seeds = queue.signer_seeds();
    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.escrow_shares.to_account_info(),
                to: ctx.accounts.destination.to_account_info(),
                authority: queue.to_account_info(),
                mint: ctx.accounts.share_mint.to_account_info(),
            },
            &[&seeds],
        ),
        stray,
        ctx.accounts.share_mint.decimals,
    )?;

    emit_cpi!(EscrowSwept {
        vault: queue.vault_state,
        queue: queue.key(),
        shares: stray,
        destination: ctx.accounts.destination.key(),
        by: ctx.accounts.admin.key(),
    });
    Ok(())
}

/// Every deserialized account is boxed; see `InitializeQueue` for why.
#[event_cpi]
#[derive(Accounts)]
pub struct SweepEscrowShares<'info> {
    #[account(
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
        has_one = vault_state @ ErrorCode::VaultMismatch,
        has_one = escrow_shares,
        has_one = share_mint,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    /// Read for its admin, never written.
    pub vault_state: Box<Account<'info, VaultState>>,

    pub admin: Signer<'info>,

    #[account(mut)]
    pub escrow_shares: Box<InterfaceAccount<'info, TokenAccount>>,

    pub share_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Any share account neither PDA owns: nothing moves shares out of an
    /// account the queue owns, the escrow included, or one the vault owns, so
    /// shares sent to either would be stuck again.
    #[account(
        mut,
        token::mint = share_mint,
        constraint = destination.owner != queue.key() @ ErrorCode::InvalidSweepDestination,
        constraint = destination.owner != queue.vault_state @ ErrorCode::InvalidSweepDestination,
    )]
    pub destination: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}
