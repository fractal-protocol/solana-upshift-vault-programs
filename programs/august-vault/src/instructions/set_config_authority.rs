// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use crate::state::config::*;
use anchor_lang::prelude::*;

/// Rotate the key permitted to create vaults.
///
/// Single-step, unlike the vault admin's two-step nomination. The reason is that
/// this role is recoverable and the vault admin's is not: if the incoming key is
/// wrong or later lost, the program's upgrade authority can reset it with
/// [`super::override_config_authority`] — no program upgrade required. The
/// zero-key is rejected outright, since it is the one value no one can ever sign
/// for and the recovery path is worth keeping cheap.
pub fn handler(ctx: Context<SetConfigAuthority>, new_authority: Pubkey) -> Result<()> {
    require!(
        new_authority != Pubkey::default(),
        ErrorCode::InvalidAuthority
    );
    ctx.accounts.program_config.authority = new_authority;
    Ok(())
}

#[derive(Accounts)]
pub struct SetConfigAuthority<'info> {
    #[account(
        mut,
        seeds = [PROGRAM_CONFIG_SEED],
        bump = program_config.bump[0],
    )]
    pub program_config: Account<'info, ProgramConfig>,

    #[account(
        constraint = program_config.authority == authority.key() @ ErrorCode::NotProtocolAuthority
    )]
    pub authority: Signer<'info>,
}
