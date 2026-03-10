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
    close_account, set_authority, spl_token_2022::instruction::AuthorityType, CloseAccount, Mint,
    SetAuthority, TokenAccount, TokenInterface,
};

/// Close an empty vault and reclaim rent
/// Only admin can call this, and vault must have zero shares outstanding
/// Note: The share_mint cannot be closed (SPL Token limitation) but its authority
/// is revoked, making it permanently unusable. The vault_token_ata and vault_state
/// are closed and rent is returned to admin.
pub fn handler(ctx: Context<CloseVault>) -> Result<()> {
    let state = &ctx.accounts.vault_state;

    // Ensure no shares are outstanding
    require!(
        ctx.accounts.share_mint.supply == 0,
        ErrorCode::VaultNotEmpty
    );

    // Ensure vault token account is empty
    require!(
        ctx.accounts.vault_token_ata.amount == 0,
        ErrorCode::VaultNotEmpty
    );

    let signer_seeds: &[&[&[u8]]] = &[&state.seeds()];

    // Close the vault token account (returns rent to admin)
    close_account(CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        CloseAccount {
            account: ctx.accounts.vault_token_ata.to_account_info(),
            destination: ctx.accounts.admin.to_account_info(),
            authority: ctx.accounts.vault_state.to_account_info(),
        },
        signer_seeds,
    ))?;

    // Revoke mint authority on share_mint (makes it permanently unusable)
    // Note: SPL Token mints cannot be closed, but revoking authority ensures
    // no new shares can ever be minted
    set_authority(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            SetAuthority {
                current_authority: ctx.accounts.vault_state.to_account_info(),
                account_or_mint: ctx.accounts.share_mint.to_account_info(),
            },
            signer_seeds,
        ),
        AuthorityType::MintTokens,
        None, // Revoke by setting to None
    )?;

    // vault_state will be closed automatically via the close constraint
    // This returns the vault_state rent to admin

    Ok(())
}

#[derive(Accounts)]
pub struct CloseVault<'info> {
    #[account(
        mut,
        seeds = [VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        close = admin
    )]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [b"mint", deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        mint::authority = vault_state
    )]
    pub share_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        seeds = [b"token_vault", deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        token::mint = deposit_mint,
        token::authority = vault_state
    )]
    pub vault_token_ata: InterfaceAccount<'info, TokenAccount>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}
