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
use anchor_spl::token_interface::{
    mint_to_checked, transfer_checked, Mint, MintToChecked, TokenAccount, TokenInterface,
    TransferChecked,
};
/// Deposit without a slippage bound. Equivalent to `handler_checked` with
/// `min_shares_out = 0`.
pub fn handler(ctx: Context<Deposit>, amount: u64) -> Result<()> {
    handler_checked(ctx, amount, 0)
}

/// Deposit, refusing to mint fewer than `min_shares_out` shares.
///
/// Share count is `floor(amount * (supply + EXTRA_SHARES) / (total + VIRTUAL_ASSETS))`,
/// so a depositor always forfeits the fractional remainder — normally dust. The
/// price can also move between quoting and execution, since share supply is read
/// from the SPL mint and any holder may burn their own tokens (see
/// `EXTRA_SHARES`). This lets a caller state the worst rate it will accept
/// instead of trusting the rate it is given. Pass 0 to opt out.
pub fn handler_checked(ctx: Context<Deposit>, amount: u64, min_shares_out: u64) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);
    require!(!ctx.accounts.vault_state.paused, ErrorCode::VaultPaused);

    // Enforce minimum deposit only on the very first deposit.
    if ctx.accounts.share_mint.supply == 0 {
        let min_deposit = VaultState::min_first_deposit(ctx.accounts.deposit_mint.decimals);
        require!(amount >= min_deposit, ErrorCode::InsufficientAmount);
    }

    ctx.accounts.transfer_in_ctx(amount)?;

    let supply = ctx.accounts.share_mint.supply;
    let total_assets = ctx.accounts.vault_state.total_assets()?;
    let shares = VaultState::shares_for_deposit(supply, total_assets, amount)?;

    require!(shares > 0, ErrorCode::ZeroAmount);
    require!(shares >= min_shares_out, ErrorCode::SlippageExceeded);

    ctx.accounts.mint_to(shares)?;
    ctx.accounts.vault_state.local_aum = ctx
        .accounts
        .vault_state
        .local_aum
        .checked_add(amount)
        .ok_or(ErrorCode::NumberOverflow)?;

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
        seeds = [VAULT_TOKEN_SEED, deposit_mint.key().as_ref(), &vault_state.vault_version],
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
