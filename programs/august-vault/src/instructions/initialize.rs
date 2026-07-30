// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use crate::state::config::*;
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

pub fn handler(
    ctx: Context<Initialize>,
    admin: Pubkey,
    operator: Pubkey,
    fee_recipient: Pubkey,
    vault_version: u8,
    share_offset: u64,
) -> Result<()> {
    // Validate before storing: an out-of-range or non-power-of-ten offset would
    // silently weaken the inflation and burn defences for the life of the vault,
    // and the field cannot be changed afterwards.
    require!(
        VaultState::is_valid_share_offset(share_offset as u128),
        ErrorCode::InvalidShareOffset
    );

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
        share_offset,
    );
    Ok(())
}

#[derive(Accounts)]
#[instruction(admin: Pubkey, operator: Pubkey, fee_recipient: Pubkey, vault_version: u8)]
pub struct Initialize<'info> {
    /// Gates vault creation: it is restricted to the authority recorded in
    /// `ProgramConfig`, rather than being open to any paying signer.
    ///
    /// **Declared first so a missing config fails closed early.** Anchor
    /// deserializes fields in declaration order, so if this account does not
    /// exist the instruction aborts with `AccountNotInitialized` before any
    /// account is created. That much *is* order-sensitive.
    ///
    /// The authority comparison below is **not**. It is a `constraint` on a
    /// non-`init` field, and `anchor_syn::codegen::accounts::try_accounts::
    /// generate_constraints` emits every `init` field's creation CPI ahead of
    /// *all* non-init access checks, regardless of declaration order — and
    /// within a single field `linearize` orders `Init` before `Raw`, so moving
    /// the constraint onto an `init` account would not help either. An
    /// unauthorized caller who is also their own `payer` and cannot fund the
    /// rent therefore fails with a System Program "insufficient lamports" error
    /// rather than `NotProtocolAuthority`.
    ///
    /// Authorization is still enforced in every case — only the error surfaced
    /// depends on the caller's balance. Pinned by
    /// `unauthorized_signer_creates_nothing_whatever_their_balance`.
    ///
    /// **Namespace note**: `vault_version` is a `u8` seed byte, so each deposit
    /// mint has 256 namespaces and each is single-use. `close_vault` cannot
    /// close the share mint (classic SPL Token has no close-mint instruction)
    /// and revokes its mint authority irreversibly, so a retired version can
    /// never be re-initialized. Retiring a vault therefore consumes one of the
    /// 256 permanently and strands its share-mint rent.
    #[account(
        seeds = [PROGRAM_CONFIG_SEED],
        bump = program_config.bump[0],
    )]
    pub program_config: Account<'info, ProgramConfig>,

    /// The protocol authority. Authorizes creation but does not fund it, so a
    /// cold or MPC-held key can hold this role without carrying SOL.
    #[account(
        constraint = program_config.authority == signer.key() @ ErrorCode::NotProtocolAuthority
    )]
    pub signer: Signer<'info>,

    /// Funds the three accounts below. May be the same key as `signer`.
    #[account(mut)]
    pub payer: Signer<'info>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer  = payer,
        seeds  = [VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &[vault_version]],
        bump,
        space = VaultState::LEN,
    )]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        init,
        payer = payer,
        seeds = [SHARE_MINT_SEED, deposit_mint.key().as_ref(), &[vault_version]],
        bump,
        mint::decimals   = deposit_mint.decimals,
        mint::authority  = vault_state
    )]
    pub share_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = payer,
        seeds = [VAULT_TOKEN_SEED, deposit_mint.key().as_ref(), &[vault_version]],
        bump,
        token::mint      = deposit_mint,
        token::authority = vault_state,
    )]
    pub vault_token_ata: InterfaceAccount<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
    pub rent: Sysvar<'info, Rent>,
}
