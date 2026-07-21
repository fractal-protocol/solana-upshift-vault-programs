// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

pub fn handler(
    ctx: Context<Initialize>,
    admin: Pubkey,
    operator: Pubkey,
    fee_recipient: Pubkey,
    vault_version: u8,
) -> Result<()> {
    let state = &mut ctx.accounts.vault_state;
    state.init(
        operator,
        admin,
        ctx.accounts.share_mint.key(),
        ctx.accounts.deposit_mint.key(),
        fee_recipient,
        0,
        [ctx.bumps.vault_state],
        [vault_version],
    );
    Ok(())
}

#[derive(Accounts)]
#[instruction(admin: Pubkey, operator: Pubkey, fee_recipient: Pubkey, vault_version: u8)]
pub struct Initialize<'info> {
    #[account(
        init,
        payer  = signer,
        seeds  = [VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &[vault_version]],
        bump,
        space = VaultState::LEN,
    )]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        init,
        payer = signer,
        seeds = [SHARE_MINT_SEED, deposit_mint.key().as_ref(), &[vault_version]],
        bump,
        mint::decimals   = deposit_mint.decimals,
        mint::authority  = vault_state
    )]
    pub share_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = signer,
        seeds = [VAULT_TOKEN_SEED, deposit_mint.key().as_ref(), &[vault_version]],
        bump,
        token::mint      = deposit_mint,
        token::authority = vault_state,
    )]
    pub vault_token_ata: InterfaceAccount<'info, TokenAccount>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub signer: Signer<'info>,
    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
    pub rent: Sysvar<'info, Rent>,
}
