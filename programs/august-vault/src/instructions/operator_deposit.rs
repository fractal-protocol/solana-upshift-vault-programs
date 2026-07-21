// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

pub fn handler(ctx: Context<OperatorDeposit>, amount: u64) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);

    let state = &mut ctx.accounts.vault_state;

    transfer_checked(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.operator_token_account.to_account_info(),
                to: ctx.accounts.vault_deposit_ata.to_account_info(),
                authority: ctx.accounts.operator.to_account_info(),
                mint: ctx.accounts.deposit_mint.to_account_info(),
            },
        ),
        amount,
        ctx.accounts.deposit_mint.decimals,
    )?;

    if amount > state.deployed_aum {
        state.deployed_aum = 0;
    } else {
        state.deployed_aum -= amount;
    };

    state.local_aum += amount;

    Ok(())
}

#[derive(Accounts)]
pub struct OperatorDeposit<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [VAULT_TOKEN_SEED, deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        token::mint      = deposit_mint,
        token::authority = vault_state
    )]
    pub vault_deposit_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = deposit_mint,
        associated_token::authority = operator,
        associated_token::token_program = token_program,
    )]
    pub operator_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint = vault_state.operator == operator.key() @ ErrorCode::NotOperator
    )]
    pub operator: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}
