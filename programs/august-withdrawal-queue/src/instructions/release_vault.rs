// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Return the vault to instant redemption. The queue gives the co-signature the
//! vault's `detach_withdrawal_queue` demands, by CPI as its own PDA: this is the
//! only place that signature is ever produced. Pending requests survive the
//! release and finalize or cancel afterwards; the vault's gate is what stops
//! new ones, so the pending set closes by itself.

use crate::auth::require_vault_admin;
use crate::errors::ErrorCode;
use crate::events::VaultReleased;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;
use august_vault::cpi::accounts::DetachWithdrawalQueue;
use august_vault::program::AugustVault;
use august_vault::state::vault::VaultState;

/// No precondition of the queue's own: nothing here could hold liquidity back
/// for the pending set once the gate is off, so whether to release is the
/// admin's judgement. Pending requests still finalize or cancel afterwards.
/// Everything else, that a queue is attached and that it is this one, the vault
/// checks inside the CPI and its errors propagate.
pub fn handler(ctx: Context<ReleaseVault>) -> Result<()> {
    require_vault_admin(
        &ctx.accounts.queue,
        &ctx.accounts.vault_state,
        &ctx.accounts.admin,
    )?;

    let seeds = ctx.accounts.queue.signer_seeds();
    august_vault::cpi::detach_withdrawal_queue(CpiContext::new_with_signer(
        ctx.accounts.vault_program.to_account_info(),
        DetachWithdrawalQueue {
            vault_state: ctx.accounts.vault_state.to_account_info(),
            deposit_mint: ctx.accounts.deposit_mint.to_account_info(),
            admin: ctx.accounts.admin.to_account_info(),
            queue: ctx.accounts.queue.to_account_info(),
        },
        &[&seeds],
    ))?;

    let queue = &ctx.accounts.queue;
    emit_cpi!(VaultReleased {
        vault: queue.vault_state,
        queue: queue.key(),
        pending_requests: queue.pending_requests,
        pending_shares: queue.pending_shares,
    });
    Ok(())
}

/// Every deserialized account is boxed; see `InitializeQueue` for why. The
/// admin signs the outer transaction and is passed through to the vault, whose
/// own admin constraint is not waived by the queue's co-signature.
#[event_cpi]
#[derive(Accounts)]
pub struct ReleaseVault<'info> {
    #[account(
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
        has_one = vault_state @ ErrorCode::VaultMismatch,
        has_one = deposit_mint,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    /// Written by the vault inside the CPI, never by this program.
    #[account(mut)]
    pub vault_state: Box<Account<'info, VaultState>>,

    pub deposit_mint: Box<InterfaceAccount<'info, Mint>>,

    pub admin: Signer<'info>,

    pub vault_program: Program<'info, AugustVault>,
}
