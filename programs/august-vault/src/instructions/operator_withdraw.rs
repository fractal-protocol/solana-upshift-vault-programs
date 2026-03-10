// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE.BSL file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

pub fn handler(ctx: Context<OperatorWithdraw>, amount: u64) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);

    let state = &mut ctx.accounts.vault_state;
    let signer_seeds: &[&[&[u8]]] = &[&state.seeds()];

    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.vault_deposit_ata.to_account_info(),
                to: ctx.accounts.operator_token_account.to_account_info(),
                authority: state.to_account_info(),
                mint: ctx.accounts.deposit_mint.to_account_info(),
            },
            signer_seeds,
        ),
        amount,
        ctx.accounts.deposit_mint.decimals,
    )?;

    //This can happen whenever someone transfers X deposit_mint
    //directly to the Vault, without going through Deposit
    //operator should be able to compound X
    if amount > state.local_aum {
        state.local_aum = 0;
    } else {
        state.local_aum -= amount;
    };

    state.deployed_aum += amount;
    Ok(())
}

#[derive(Accounts)]
pub struct OperatorWithdraw<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        seeds=[b"token_vault", deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        token::mint      = deposit_mint,
        token::authority = vault_state
    )]
    pub vault_deposit_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        associated_token::mint = deposit_mint,
        associated_token::authority = operator,
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
