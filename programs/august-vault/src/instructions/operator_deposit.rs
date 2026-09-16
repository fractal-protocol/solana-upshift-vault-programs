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

pub fn handler(ctx: Context<OperatorDeposit>, amount: u64) -> Result<()> {
    require!(amount > 0, ErrorCode::ZeroAmount);

    let state = &mut ctx.accounts.vault_state;

    require!(
        state.requires_subaccount() == ctx.accounts.subaccount.is_some(),
        ErrorCode::InvalidSubaccount
    );

    // An unconfigured vault pulls from the operator's own ATA, which the
    // operator signs for. With a subaccount the source is custody the operator
    // cannot sign for, so the vault PDA pulls against a delegation. One branch
    // so the PDA cannot sign without the delegation having been checked.
    let seeds = state.seeds();
    let vault_signer: [&[&[u8]]; 1] = [&seeds];
    let (authority, signer_seeds): (_, &[&[&[u8]]]) = if ctx.accounts.subaccount.is_some() {
        // Covers the delegation only. A short balance or frozen source still
        // surface as SPL's own errors.
        let source = &ctx.accounts.operator_token_account;
        require!(
            source.delegate == COption::Some(state.key()) && source.delegated_amount >= amount,
            ErrorCode::SubaccountDelegationMissing
        );
        (state.to_account_info(), vault_signer.as_slice())
    } else {
        // Empty seeds make `new_with_signer` equivalent to `new`: the vault PDA
        // grants no signature on the unconfigured path.
        (ctx.accounts.operator.to_account_info(), &[])
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

    // A return beyond the outstanding principal is recapitalisation, not
    // repayment — so the total drops only by what this destination actually
    // owed. Subtracting the full amount would erase other destinations'
    // exposure: return 200 from A when A and B each owe 100, and the total
    // reads zero while B still owes 100.
    let repaid = match &ctx.accounts.subaccount {
        Some(sub) => amount.min(sub.principal),
        None => amount,
    };
    state.deployed_principal = state.deployed_principal.saturating_sub(repaid);
    if let Some(sub) = &mut ctx.accounts.subaccount {
        sub.principal = sub.principal.saturating_sub(amount);
    }

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

    /// Source: the named subaccount's ATA if the vault has any registered, else
    /// the operator's own. Keeps the old name for wire compatibility.
    #[account(
        mut,
        associated_token::mint = deposit_mint,
        associated_token::authority = subaccount.as_ref().map(|s| s.address).unwrap_or_else(|| operator.key()),
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

    /// The source's registry entry, required exactly when the vault has
    /// registrations. Its PDA binds it to this vault.
    ///
    /// **Last, and omittable.** Inserting it mid-struct would shift every
    /// account after it, so a caller sending the pre-registry account list would
    /// have its `deposit_mint` deserialized as a `Subaccount` — breaking every
    /// existing operator integration on upgrade, before any admin opted in.
    /// Appended plus `allow-missing-optionals`, the old six-account call still
    /// works and resolves to the operator's own ATA.
    #[account(
        mut,
        seeds = [SUBACCOUNT_SEED, vault_state.key().as_ref(), subaccount.address.as_ref()],
        bump = subaccount.bump,
    )]
    pub subaccount: Option<Account<'info, Subaccount>>,
}
