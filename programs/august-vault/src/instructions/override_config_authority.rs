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

/// Reset the vault-creation authority, authorized by the program's **upgrade
/// authority** rather than the current config authority.
///
/// Exists because `ProgramConfig` is created once (`initialize_config` uses
/// `init`) and `set_config_authority` requires the *current* authority to sign.
/// Without this, a single mistyped or lost rotation would permanently prevent
/// any new vault from being created, recoverable only by shipping a program
/// upgrade — a Fordefi-signed ceremony on mainnet.
///
/// This grants the upgrade authority no power it lacked: it can already replace
/// the whole program, so being able to reset one field is strictly weaker. It is
/// deliberately a separate instruction rather than an extra branch in
/// `set_config_authority`, so the privileged path is explicit at the call site
/// and in the IDL.
pub fn handler(ctx: Context<OverrideConfigAuthority>, new_authority: Pubkey) -> Result<()> {
    require!(
        new_authority != Pubkey::default(),
        ErrorCode::InvalidAuthority
    );
    ctx.accounts.program_config.authority = new_authority;
    Ok(())
}

#[derive(Accounts)]
pub struct OverrideConfigAuthority<'info> {
    #[account(
        mut,
        seeds = [PROGRAM_CONFIG_SEED],
        bump = program_config.bump[0],
    )]
    pub program_config: Account<'info, ProgramConfig>,

    /// Must be the program's current upgrade authority.
    pub upgrade_authority: Signer<'info>,

    /// This program's `ProgramData`, pinned by seeds so a caller cannot present
    /// the `ProgramData` of an unrelated program they happen to control.
    #[account(
        seeds = [crate::ID.as_ref()],
        bump,
        seeds::program = anchor_lang::solana_program::bpf_loader_upgradeable::ID,
        constraint = program_data.upgrade_authority_address == Some(upgrade_authority.key())
            @ ErrorCode::NotProtocolAuthority,
    )]
    pub program_data: Account<'info, ProgramData>,
}
