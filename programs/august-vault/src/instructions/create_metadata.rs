// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE.BSL file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::state::vault::{VaultState, VAULT_STATE_SEED};
use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token};
use anchor_spl::token_interface::Mint as InterfaceMint;
use mpl_token_metadata::{instructions::CreateMetadataAccountV3, types::DataV2};

#[derive(Accounts)]
pub struct CreateShareTokenMetadata<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// The admin of the vault - only they can create/update metadata
    #[account(
        mut,
        constraint = admin.key() == vault_state.admin @ crate::errors::ErrorCode::UnauthorizedAdmin
    )]
    pub admin: Signer<'info>,

    /// The vault state PDA that owns the mint authority
    #[account(
        mut,
        seeds = [VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        constraint = !vault_state.paused @ crate::errors::ErrorCode::VaultPaused
    )]
    pub vault_state: Account<'info, VaultState>,

    /// The deposit mint used for PDA derivation
    pub deposit_mint: InterfaceAccount<'info, InterfaceMint>,

    /// The share token mint
    #[account(
        mut,
        mint::authority = vault_state,
    )]
    pub share_mint: Account<'info, Mint>,

    /// CHECK: This is the metadata account that will be created
    #[account(
        mut,
        seeds = [
            b"metadata",
            mpl_token_metadata::ID.as_ref(),
            share_mint.key().as_ref(),
        ],
        bump,
        seeds::program = mpl_token_metadata::ID,
    )]
    pub metadata_account: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    /// CHECK: Metaplex Token Metadata Program
    #[account(address = mpl_token_metadata::ID)]
    pub token_metadata_program: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

/// Create metadata for the share token
///
/// SECURITY: Only the vault admin can create/update metadata
/// - Requires admin signature
/// - Vault must not be paused
/// - Vault State PDA is set as the update authority
pub fn create_share_token_metadata(
    ctx: Context<CreateShareTokenMetadata>,
    name: String,
    symbol: String,
    uri: String,
) -> Result<()> {
    let vault_seeds = ctx.accounts.vault_state.seeds();
    let signer_seeds = &[&vault_seeds[..]];

    let data_v2 = DataV2 {
        name,
        symbol,
        uri,
        seller_fee_basis_points: 0,
        creators: None,
        collection: None,
        uses: None,
    };

    let create_metadata_account_ix = CreateMetadataAccountV3 {
        metadata: ctx.accounts.metadata_account.key(),
        mint: ctx.accounts.share_mint.key(),
        mint_authority: ctx.accounts.vault_state.key(),
        payer: ctx.accounts.payer.key(),
        update_authority: (ctx.accounts.vault_state.key(), true),
        system_program: ctx.accounts.system_program.key(),
        rent: Some(ctx.accounts.rent.key()),
    };

    let create_metadata_account_ix = create_metadata_account_ix.instruction(
        mpl_token_metadata::instructions::CreateMetadataAccountV3InstructionArgs {
            data: data_v2,
            is_mutable: true,
            collection_details: None,
        },
    );

    anchor_lang::solana_program::program::invoke_signed(
        &create_metadata_account_ix,
        &[
            ctx.accounts.metadata_account.to_account_info(),
            ctx.accounts.share_mint.to_account_info(),
            ctx.accounts.vault_state.to_account_info(),
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            ctx.accounts.rent.to_account_info(),
            ctx.accounts.token_metadata_program.to_account_info(),
        ],
        signer_seeds,
    )?;

    Ok(())
}
