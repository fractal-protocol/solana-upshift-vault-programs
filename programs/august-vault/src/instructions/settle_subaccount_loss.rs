// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use crate::state::subaccount::*;
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

pub fn handler(ctx: Context<SettleSubaccountLoss>, amount: u64) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);

    let sub = &mut ctx.accounts.subaccount;

    // Only principal that is not sitting at the destination may be written
    // down. That is what keeps this from being a way to erase a live
    // obligation: if the tokens are there, return them instead.
    //
    // The balance is externally mutable, but only in the safe direction — a
    // donation shrinks the shortfall and so permits less, never more.
    let shortfall = sub
        .principal
        .saturating_sub(ctx.accounts.subaccount_ata.amount);
    require!(amount <= shortfall, ErrorCode::LossExceedsShortfall);

    msg!(
        "settle loss {} at subaccount {}: principal {} -> {}",
        amount,
        sub.address,
        sub.principal,
        sub.principal - amount
    );

    sub.principal -= amount;
    let state = &mut ctx.accounts.vault_state;
    state.deployed_principal = state.deployed_principal.saturating_sub(amount);
    Ok(())
}

/// Recognize a realized loss at one destination, so principal it will never
/// return stops blocking deregistration and vault closure.
///
/// Admin only, deliberately: the operator reducing principal is exactly the
/// capacity-reopening move the coverage rule exists to prevent. `deployed_aum`
/// is untouched — reported value stays the operator's to move, under its own
/// bps limits.
#[derive(Accounts)]
pub struct SettleSubaccountLoss<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [SUBACCOUNT_SEED, vault_state.key().as_ref(), subaccount.address.as_ref()],
        bump = subaccount.bump,
        constraint = subaccount.vault_state == vault_state.key() @ ErrorCode::InvalidSubaccount,
    )]
    pub subaccount: Account<'info, Subaccount>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    /// Read for its balance, to bound the write-down. Derived from the
    /// destination, so it cannot be substituted.
    #[account(
        associated_token::mint = deposit_mint,
        associated_token::authority = subaccount.address,
        associated_token::token_program = token_program,
    )]
    pub subaccount_ata: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,

    #[account(
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,
}
