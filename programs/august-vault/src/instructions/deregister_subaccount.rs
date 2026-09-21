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
use anchor_spl::token_interface::Mint;

pub fn handler(ctx: Context<DeregisterSubaccount>) -> Result<()> {
    // Removing a destination the vault is still owed funds from would lose the
    // record of what is outstanding, and with it the coverage rule's basis.
    require!(
        ctx.accounts.subaccount.principal == 0,
        ErrorCode::SubaccountNotEmpty
    );

    let state = &mut ctx.accounts.vault_state;
    // Saturating, not checked: undercounting the registry is benign, while an
    // underflow panic here would strand the only way out of subaccount mode.
    state.subaccount_count = state.subaccount_count.saturating_sub(1);
    Ok(())
}

#[derive(Accounts)]
pub struct DeregisterSubaccount<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        close = admin,
        seeds = [SUBACCOUNT_SEED, vault_state.key().as_ref(), subaccount.address.as_ref()],
        bump = subaccount.bump,
        constraint = subaccount.vault_state == vault_state.key() @ ErrorCode::InvalidSubaccount,
    )]
    pub subaccount: Account<'info, Subaccount>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,
}
