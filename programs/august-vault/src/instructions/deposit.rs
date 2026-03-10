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
    mint_to_checked, transfer_checked, Mint, MintToChecked, TokenAccount, TokenInterface,
    TransferChecked,
};
pub fn handler(ctx: Context<Deposit>, amount: u64) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);
    require!(!ctx.accounts.vault_state.paused, ErrorCode::VaultPaused);

    // Check if this is the first deposit (vault has no assets)
    let is_first_deposit = ctx.accounts.share_mint.supply == 0;

    // Require minimum deposit only for the first deposit
    if is_first_deposit {
        // Calculate dynamic minimum based on token decimals
        // Minimum of 0.001 tokens in the token's native units
        let min_deposit = match ctx.accounts.deposit_mint.decimals {
            0 => 1,         // 0.001 tokens = 1 unit (0 decimals)
            1 => 1,         // 0.001 tokens = 1 unit (1 decimal)
            2 => 1,         // 0.001 tokens = 1 unit (2 decimals)
            3 => 1,         // 0.001 tokens = 1 unit (3 decimals)
            4 => 10,        // 0.001 tokens = 10 units (4 decimals)
            5 => 100,       // 0.001 tokens = 100 units (5 decimals)
            6 => 1_000,     // 0.001 tokens = 1,000 units (6 decimals)
            7 => 10_000,    // 0.001 tokens = 10,000 units (7 decimals)
            8 => 100_000,   // 0.001 tokens = 100,000 units (8 decimals)
            9 => 1_000_000, // 0.001 tokens = 1,000,000 units (9 decimals)
            _ => 10_u64.pow(ctx.accounts.deposit_mint.decimals.saturating_sub(3) as u32), // For 10+ decimals
        };

        require!(amount >= min_deposit, ErrorCode::InsufficientAmount);
    }

    ctx.accounts.transfer_in_ctx(amount)?;
    let local_aum: u64 = ctx.accounts.vault_state.local_aum;

    let supply = ctx.accounts.share_mint.supply;
    let total_assets = local_aum + ctx.accounts.vault_state.deployed_aum;

    let shares_eff = (supply as u128).checked_add(EXTRA_SHARES).unwrap();
    let assets_eff = (total_assets as u128).checked_add(1).unwrap();

    let shares = u64::try_from(
        (amount as u128)
            .checked_mul(shares_eff)
            .unwrap()
            .checked_div(assets_eff)
            .unwrap(),
    )?;

    require!(shares > 0, ErrorCode::ZeroAmount);

    ctx.accounts.mint_to(shares)?;
    ctx.accounts.vault_state.local_aum += amount;

    emit!(DepositEvt {
        caller: ctx.accounts.signer.key(),
        receiver: ctx.accounts.signer.key(),
        amount,
        shares,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct Deposit<'info> {
    #[account(
        mut,
        seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump)]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        seeds=[b"token_vault", deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        token::mint      = deposit_mint,
        token::authority = vault_state
    )]
    pub vault_token_ata: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        token::mint      = deposit_mint,
        token::authority = signer
    )]
    pub sender_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        token::mint      = share_mint,
        token::authority = signer
    )]
    pub sender_share_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"mint", deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        mint::decimals   = deposit_mint.decimals,
        mint::authority  = vault_state
    )]
    pub share_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub signer: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

impl<'info> Deposit<'info> {
    fn transfer_in_ctx(&self, amount: u64) -> Result<()> {
        transfer_checked(
            CpiContext::new(
                self.token_program.to_account_info(),
                TransferChecked {
                    from: self.sender_token_account.to_account_info(),
                    to: self.vault_token_ata.to_account_info(),
                    authority: self.signer.to_account_info(),
                    mint: self.deposit_mint.to_account_info(),
                },
            ),
            amount,
            self.deposit_mint.decimals,
        )?;
        Ok(())
    }

    fn mint_to(&self, shares: u64) -> Result<()> {
        mint_to_checked(
            CpiContext::new_with_signer(
                self.token_program.to_account_info(),
                MintToChecked {
                    mint: self.share_mint.to_account_info(),
                    to: self.sender_share_account.to_account_info(),
                    authority: self.vault_state.to_account_info(),
                },
                &[&self.vault_state.seeds()],
            ),
            shares,
            self.share_mint.decimals,
        )?;
        Ok(())
    }
}

#[event]
pub struct DepositEvt {
    pub caller: Pubkey,
    pub receiver: Pubkey,
    pub amount: u64,
    pub shares: u64,
}
