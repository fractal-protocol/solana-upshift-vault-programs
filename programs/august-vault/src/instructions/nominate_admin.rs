// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use crate::state::nominated_admin::{NominatedAdmin, NOMINATED_ADMIN_PDA_SEED};
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

pub fn handler(ctx: Context<NominateAdmin>, nominated_admin: Pubkey) -> Result<()> {
    let mut nominated_admin_pda = match ctx.accounts.nominated_admin_pda.load_init() {
        Ok(nominated_admin_pda) => nominated_admin_pda,
        Err(_) => ctx.accounts.nominated_admin_pda.load_mut()?,
    };
    nominated_admin_pda.initialize(nominated_admin);
    msg!("Current admin: {}", ctx.accounts.vault_state.admin);
    msg!("Nominated admin: {}", nominated_admin);
    msg!("Valid until: {}", nominated_admin_pda.valid_until());
    Ok(())
}

#[derive(Accounts)]
pub struct NominateAdmin<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init_if_needed,
        space = NominatedAdmin::LEN,
        seeds = [
            NOMINATED_ADMIN_PDA_SEED.as_ref(),
            deposit_mint.key().as_ref(),
            &vault_state.vault_version
        ],
        bump,
        payer = payer
    )]
    pub nominated_admin_pda: AccountLoader<'info, NominatedAdmin>,

    #[account(
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}
