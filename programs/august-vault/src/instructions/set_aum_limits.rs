// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE.BSL file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::state::vault::{VaultState, VAULT_STATE_SEED};
use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

#[derive(Accounts)]
pub struct SetAumLimits<'info> {
    #[account(
        mut,
        seeds = [VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        constraint = admin.key() == vault_state.admin @ crate::errors::ErrorCode::NotAdmin
    )]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub admin: Signer<'info>,
}

/// Admin Updates the AUM Change Limits
///
/// Sets the maximum allowed increase and decrease percentages for AUM updates
/// ### Parameters
/// - `increase_limit` - Max increase in basis points (e.g., 20 = 0.2%, 100 = 1%)
/// - `decrease_limit` - Max decrease in basis points (e.g., 20 = 0.2%, 100 = 1%)
pub fn handler(ctx: Context<SetAumLimits>, increase_limit: u32, decrease_limit: u32) -> Result<()> {
    let state = &mut ctx.accounts.vault_state;

    // Validate limits are reasonable (max 100% = 10000 basis points)
    require!(
        increase_limit <= 10000,
        crate::errors::ErrorCode::AumLimitTooHigh
    );
    require!(
        decrease_limit <= 10000,
        crate::errors::ErrorCode::AumLimitTooHigh
    );

    state.aum_increase_limit = increase_limit;
    state.aum_decrease_limit = decrease_limit;

    Ok(())
}
