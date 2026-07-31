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

/// Create the singleton program config, naming the key that may create vaults.
///
/// Bootstrapped by the program's **current upgrade authority**, verified against
/// the loader's `ProgramData` account rather than a hardcoded pubkey — mainnet
/// and devnet have different upgrade authorities, and reading the on-chain value
/// keeps one code path correct on every cluster while introducing no trust
/// assumption beyond the one that already governs upgrades.
///
/// Must be run immediately after the upgrade that adds it: until the config
/// exists, `initialize` fails closed and no vault can be created.
pub fn handler(ctx: Context<InitializeConfig>, authority: Pubkey) -> Result<()> {
    ctx.accounts
        .program_config
        .init(authority, [ctx.bumps.program_config]);
    Ok(())
}

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    /// Must be the program's current upgrade authority.
    pub upgrade_authority: Signer<'info>,

    /// This program's `ProgramData`. The seed constraint pins it to *this*
    /// program, so a caller cannot present the `ProgramData` of some unrelated
    /// program they happen to control.
    ///
    /// Note that declaration order does **not** make this check run before
    /// `program_config` is created: Anchor emits every `init` field's creation
    /// CPI ahead of all non-init access checks. An upgrade-authority mismatch is
    /// still always caught — the whole transaction reverts — but a caller who
    /// cannot fund the config's rent sees the System Program's error rather than
    /// `NotProtocolAuthority`.
    #[account(
        seeds = [crate::ID.as_ref()],
        bump,
        seeds::program = anchor_lang::solana_program::bpf_loader_upgradeable::ID,
        constraint = program_data.upgrade_authority_address == Some(upgrade_authority.key())
            @ ErrorCode::NotProtocolAuthority,
    )]
    pub program_data: Account<'info, ProgramData>,

    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        init,
        payer = payer,
        seeds = [PROGRAM_CONFIG_SEED],
        bump,
        space = ProgramConfig::LEN,
    )]
    pub program_config: Account<'info, ProgramConfig>,

    pub system_program: Program<'info, System>,
}
