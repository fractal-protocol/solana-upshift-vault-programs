// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use crate::state::subaccount::*;
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_option::COption;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

pub fn handler(ctx: Context<OperatorWithdraw>, amount: u64) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);

    let state = &mut ctx.accounts.vault_state;

    // Pairing rule: without it the operator omits the account and pays itself.
    require!(
        state.requires_subaccount() == ctx.accounts.subaccount.is_some(),
        ErrorCode::InvalidSubaccount
    );

    // Keep what this destination owes covered by a live delegation, so it stays
    // recallable. Measured against its own principal, not reported AUM (a report
    // would reopen capacity) nor the ATA balance (anyone can donate to it).
    if let Some(sub) = &ctx.accounts.subaccount {
        let dest = &ctx.accounts.operator_token_account;
        require!(
            dest.delegate == COption::Some(state.key())
                && dest.delegated_amount >= sub.principal.saturating_add(amount),
            ErrorCode::SubaccountDelegationMissing
        );
    }

    let signer_seeds: &[&[&[u8]]] = &[&state.seeds()];

    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.vault_deposit_ata.to_account_info(),
                to: ctx.accounts.operator_token_account.to_account_info(),
                authority: state.to_account_info(),
                mint: ctx.accounts.deposit_mint.to_account_info(),
            },
            signer_seeds,
        ),
        amount,
        ctx.accounts.deposit_mint.decimals,
    )?;

    //This can happen whenever someone transfers X deposit_mint
    //directly to the Vault, without going through Deposit
    //operator should be able to compound X
    if amount > state.local_aum {
        state.local_aum = 0;
    } else {
        state.local_aum -= amount;
    };

    state.deployed_aum += amount;
    // Never marked, unlike `deployed_aum`, so a report cannot reopen capacity.
    state.deployed_principal = state
        .deployed_principal
        .checked_add(amount)
        .ok_or(ErrorCode::NumberOverflow)?;
    if let Some(sub) = &mut ctx.accounts.subaccount {
        sub.principal = sub
            .principal
            .checked_add(amount)
            .ok_or(ErrorCode::NumberOverflow)?;
    }
    Ok(())
}

#[derive(Accounts)]
pub struct OperatorWithdraw<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        mut,
        seeds = [VAULT_TOKEN_SEED, deposit_mint.key().as_ref(), &vault_state.vault_version],
        bump,
        token::mint      = deposit_mint,
        token::authority = vault_state
    )]
    pub vault_deposit_ata: InterfaceAccount<'info, TokenAccount>,

    /// Destination: a registered subaccount's ATA, else the operator's own.
    /// Keeps the old name for wire compatibility.
    #[account(
        mut,
        associated_token::mint = deposit_mint,
        associated_token::authority = subaccount.as_ref().map(|s| s.address).unwrap_or_else(|| operator.key()),
        associated_token::token_program = token_program,
    )]
    pub operator_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint = vault_state.operator == operator.key() @ ErrorCode::NotOperator
    )]
    pub operator: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,

    /// The destination's registry entry, required exactly when the vault has
    /// registrations. Its PDA binds it to this vault.
    ///
    /// **Last, and omittable.** Inserted mid-struct it would shift
    /// `deposit_mint`, breaking every pre-registry operator call on upgrade.
    #[account(
        mut,
        seeds = [SUBACCOUNT_SEED, vault_state.key().as_ref(), subaccount.address.as_ref()],
        bump = subaccount.bump,
    )]
    pub subaccount: Option<Account<'info, Subaccount>>,
}
