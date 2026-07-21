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
use anchor_spl::token_interface::Mint;

pub fn handler(ctx: Context<OperatorUpdateAum>, new_aum: u64) -> Result<()> {
    let state = &mut ctx.accounts.vault_state;

    // Calculate limits using configurable values (in basis points)
    let decrease_limit = (10000 - state.aum_decrease_limit) as u64;
    let increase_limit = (10000 + state.aum_increase_limit) as u64;

    require!(
        new_aum * 10000 >= decrease_limit * state.deployed_aum,
        ErrorCode::AumDecreaseTooBig
    );
    require!(
        new_aum * 10000 <= increase_limit * state.deployed_aum,
        ErrorCode::AumIncreaseTooBig
    );

    state.deployed_aum = new_aum;
    Ok(())
}

#[derive(Accounts)]
pub struct OperatorUpdateAum<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint = vault_state.operator == operator.key() @ ErrorCode::NotOperator
    )]
    pub operator: Signer<'info>,
}
