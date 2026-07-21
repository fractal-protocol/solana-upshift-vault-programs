// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::state::nominated_admin::{NominatedAdmin, NOMINATED_ADMIN_PDA_SEED};
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

#[derive(Accounts)]
pub struct AcceptAdminNomination<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        close = receiver,
        seeds = [
            NOMINATED_ADMIN_PDA_SEED.as_ref(),
            deposit_mint.key().as_ref(),
            &vault_state.vault_version
        ],
        bump,
    )]
    pub nominated_admin_pda: AccountLoader<'info, NominatedAdmin>,

    pub new_admin: Signer<'info>,

    /// CHECK: rent receiver account
    #[account(mut)]
    pub receiver: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<AcceptAdminNomination>) -> Result<()> {
    let nominated_admin_pda = ctx.accounts.nominated_admin_pda.load()?;
    nominated_admin_pda.valid_accept_nomination(ctx.accounts.new_admin.key)?;
    let vault: &mut Account<VaultState> = &mut ctx.accounts.vault_state;
    msg!("Current admin: {}", vault.admin);
    vault.admin = nominated_admin_pda.nominated_admin();
    msg!("New admin: {}", vault.admin);
    Ok(())
}
