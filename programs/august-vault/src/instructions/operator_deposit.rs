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
use anchor_lang::solana_program::program_option::COption;
use anchor_spl::token_interface::{
    transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked,
};

pub fn handler(ctx: Context<OperatorDeposit>, amount: u64) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);

    let state = &mut ctx.accounts.vault_state;

    // Who signs the source transfer follows from who owns the source: an
    // unconfigured vault pulls from the operator's own ATA, so the operator
    // signs; with a subaccount the source is custody the operator cannot sign
    // for, so the vault PDA pulls against a delegation granted up front.
    //
    // One `match`, deliberately: "the vault PDA signs" and "the delegation was
    // verified" are the same decision, and splitting them across two readers of
    // a bool is how they would come apart.
    let seeds = state.seeds();
    let vault_signer: [&[&[u8]]; 1] = [&seeds];
    let (authority, signer_seeds): (_, &[&[&[u8]]]) = match state.operator_subaccount() {
        Some(_) => {
            // SPL would report a missing or wrong delegate as `OwnerMismatch`,
            // and a short allowance as `InsufficientFunds` — indistinguishable
            // from a short balance. A short balance and a frozen source still
            // surface as SPL's own errors.
            let source = &ctx.accounts.operator_token_account;
            require!(
                source.delegate == COption::Some(state.key()) && source.delegated_amount >= amount,
                ErrorCode::SubaccountDelegationMissing
            );
            (state.to_account_info(), vault_signer.as_slice())
        }
        // Empty seeds make `new_with_signer` equivalent to `new`: the vault PDA
        // grants no signature on the legacy path.
        None => (ctx.accounts.operator.to_account_info(), &[]),
    };

    transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            TransferChecked {
                from: ctx.accounts.operator_token_account.to_account_info(),
                to: ctx.accounts.vault_deposit_ata.to_account_info(),
                authority,
                mint: ctx.accounts.deposit_mint.to_account_info(),
            },
            signer_seeds,
        ),
        amount,
        ctx.accounts.deposit_mint.decimals,
    )?;

    if amount > state.deployed_aum {
        state.deployed_aum = 0;
    } else {
        state.deployed_aum -= amount;
    };

    state.local_aum += amount;

    // Mirrors the `deployed_aum` clamp above: a return larger than the
    // outstanding principal is a recapitalisation, not a repayment.
    state.deployed_principal = state.deployed_principal.saturating_sub(amount);

    Ok(())
}

#[derive(Accounts)]
pub struct OperatorDeposit<'info> {
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

    /// Source: the vault's `operator_subaccount` if set, else the operator's
    /// own ATA.
    #[account(
        mut,
        associated_token::mint = deposit_mint,
        associated_token::authority = vault_state.destination_for_signer(operator.key()),
        associated_token::token_program = token_program,
    )]
    pub operator_token_account: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub deposit_mint: InterfaceAccount<'info, Mint>,

    /// Still the operator: the destination changed, not who may move funds.
    #[account(
        constraint = vault_state.operator == operator.key() @ ErrorCode::NotOperator
    )]
    pub operator: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}
