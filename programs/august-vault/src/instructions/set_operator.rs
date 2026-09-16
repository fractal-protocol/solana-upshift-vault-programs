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

pub fn handler(ctx: Context<SetOperator>, new_operator: Pubkey) -> Result<()> {
    let state = &mut ctx.accounts.vault_state;

    // Otherwise `set_operator_subaccount`'s `subaccount != operator` rule is
    // one call away from being undone.
    require!(
        state.operator_subaccount() != Some(new_operator),
        ErrorCode::InvalidOperatorSubaccount
    );

    // Matches the three config-authority setters. Nobody can sign as the zero
    // key, so both operator handlers would become uncallable.
    require!(
        new_operator != Pubkey::default(),
        ErrorCode::InvalidAuthority
    );

    state.operator = new_operator;
    Ok(())
}

#[derive(Accounts)]
pub struct SetOperator<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,
}
