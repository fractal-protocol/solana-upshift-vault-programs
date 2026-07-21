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
use anchor_spl::token_2022::burn_checked;
use anchor_spl::token_interface::{
    transfer_checked, BurnChecked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

/// Redeems shares for underlying assets.
///
/// SECURITY: This function follows the CEI (Check-Effects-Interactions) pattern strictly.
/// The liquidity check MUST occur before burn_shares and transfers to prevent wasted
/// compute units on failed transactions and ensure proper error handling.
/// See security finding C-01 in SECURITY_AUDIT_REPORT.md.
pub fn handler(ctx: Context<Redeem>, shares: u64) -> Result<()> {
    require!(shares > 0, ErrorCode::ZeroAmount);
    require!(!ctx.accounts.vault_state.paused, ErrorCode::VaultPaused);

    let supply = ctx.accounts.share_mint.supply;
    let total_assets = ctx.accounts.vault_state.total_assets()?;
    let assets = VaultState::assets_for_redeem(supply, total_assets, shares)?;

    require!(assets > 0, ErrorCode::ZeroAmount);

    let fee_numerator = (assets as u128)
        .checked_mul(ctx.accounts.vault_state.withdrawal_fee as u128)
        .ok_or(ErrorCode::MathError)?;
    let fees: u64 = Redeem::ceil_div(fee_numerator, FEE_RATE_DENOMINATOR_VALUE as u128)
        .ok_or(ErrorCode::MathError)?
        .try_into()
        .map_err(|_| ErrorCode::NumberOverflow)?;

    let final_amount = assets.checked_sub(fees).ok_or(ErrorCode::MathError)?;

    // CEI - Checks: Validate liquidity BEFORE any state changes
    require!(
        assets <= ctx.accounts.vault_state.local_aum,
        ErrorCode::NotEnoughLiquidity
    );

    // CEI - Effects: Update vault state
    ctx.accounts.vault_state.local_aum = ctx
        .accounts
        .vault_state
        .local_aum
        .checked_sub(assets)
        .ok_or(ErrorCode::MathError)?;

    // CEI - Interactions: External CPI calls (burn shares, transfer fees, transfer redemption)
    ctx.accounts.burn_shares(shares)?;
    ctx.accounts
        .transfer_out_ctx(fees, &ctx.accounts.fee_recipient_account.to_account_info())?;

    ctx.accounts.transfer_out_ctx(
        final_amount,
        &ctx.accounts.sender_token_account.to_account_info(),
    )?;

    emit!(WithdrawEvt {
        caller: ctx.accounts.signer.key(),
        receiver: ctx.accounts.signer.key(),
        owner: ctx.accounts.signer.key(),
        assets: final_amount,
        shares,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct Redeem<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Box<Account<'info, VaultState>>,

    #[account(
        mut,
        seeds = [VAULT_TOKEN_SEED, deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        token::mint      = deposit_mint,
        token::authority = vault_state
    )]
    pub vault_deposit_ata: InterfaceAccount<'info, TokenAccount>,

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
        token::mint      = deposit_mint,
        token::authority = vault_state.fee_recipient,
    )]
    pub fee_recipient_account: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [SHARE_MINT_SEED, deposit_mint.key().as_ref(), &vault_state.vault_version],
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

impl<'info> Redeem<'info> {
    fn ceil_div(numerator: u128, denominator: u128) -> Option<u128> {
        numerator
            .checked_add(denominator)?
            .checked_sub(1)?
            .checked_div(denominator)
    }

    fn burn_shares(&self, shares: u64) -> Result<()> {
        burn_checked(
            CpiContext::new(
                self.token_program.to_account_info(),
                BurnChecked {
                    mint: self.share_mint.to_account_info(),
                    from: self.sender_share_account.to_account_info(),
                    authority: self.signer.to_account_info(),
                },
            ),
            shares,
            self.share_mint.decimals,
        )?;
        Ok(())
    }

    fn transfer_out_ctx(&self, amount: u64, to: &AccountInfo<'info>) -> Result<()> {
        transfer_checked(
            CpiContext::new_with_signer(
                self.token_program.to_account_info(),
                TransferChecked {
                    from: self.vault_deposit_ata.to_account_info(),
                    to: to.to_account_info(),
                    authority: self.vault_state.to_account_info(),
                    mint: self.deposit_mint.to_account_info(),
                },
                &[&self.vault_state.seeds()],
            ),
            amount,
            self.deposit_mint.decimals,
        )?;
        Ok(())
    }
}

#[event]
pub struct WithdrawEvt {
    pub caller: Pubkey,
    pub receiver: Pubkey,
    pub owner: Pubkey,
    pub assets: u64,
    pub shares: u64,
}
