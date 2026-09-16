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
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

pub fn handler(ctx: Context<RegisterSubaccount>, address: Pubkey) -> Result<()> {
    let state = &mut ctx.accounts.vault_state;

    // A delegation is the only proof the address can return funds: it needs the
    // owner's signature, unlike ATA existence, which anyone can create.
    //
    // The first registration adopts the vault's outstanding principal, so the
    // allowance must cover it or registration creates principal the operator
    // cannot return — the funds sit at its own ATA, no longer an accepted
    // source — which also blocks deregistration.
    let inherited = if state.subaccount_count == 0 {
        state.deployed_principal
    } else {
        0
    };
    let ata = &ctx.accounts.subaccount_ata;
    require!(
        ata.delegate == COption::Some(state.key())
            && ata.delegated_amount > 0
            && ata.delegated_amount >= inherited,
        ErrorCode::SubaccountDelegationMissing
    );

    // The proof cannot see this: the operator may have delegated its own ATA.
    require!(address != state.operator, ErrorCode::InvalidSubaccount);

    let sub = &mut ctx.accounts.subaccount;
    sub.vault_state = state.key();
    sub.address = address;
    sub.bump = ctx.bumps.subaccount;

    sub.principal = inherited;

    state.subaccount_count = state
        .subaccount_count
        .checked_add(1)
        .ok_or(ErrorCode::NumberOverflow)?;
    Ok(())
}

#[derive(Accounts)]
#[instruction(address: Pubkey)]
pub struct RegisterSubaccount<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    #[account(
        init,
        payer = admin,
        space = Subaccount::LEN,
        seeds = [SUBACCOUNT_SEED, vault_state.key().as_ref(), address.as_ref()],
        bump,
    )]
    pub subaccount: Account<'info, Subaccount>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    /// The address's ATA, carrying the delegation. Derived from `address`, so it
    /// cannot disagree with the argument.
    #[account(
        associated_token::mint = deposit_mint,
        associated_token::authority = address,
        associated_token::token_program = token_program,
    )]
    pub subaccount_ata: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,

    /// Admin, not the operator. Where they are the same key this buys nothing.
    #[account(
        mut,
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}
