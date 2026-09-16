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
    // Program-owned addresses are excluded only because this program issues no
    // `Approve` CPI. See README.
    // The first registration adopts the vault's outstanding principal, so the
    // allowance has to cover it — otherwise registration immediately creates
    // uncovered principal the operator cannot return (the funds are at its own
    // ATA, no longer an accepted source) and that blocks deregistration too.
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

    // The proof cannot see this: the operator may have delegated its own ATA,
    // and naming it would leave a vault reading as configured while paying the
    // operator exactly as before.
    require!(address != state.operator, ErrorCode::InvalidSubaccount);

    let sub = &mut ctx.accounts.subaccount;
    sub.vault_state = state.key();
    sub.address = address;
    sub.bump = ctx.bumps.subaccount;

    // The first registration adopts the vault's existing outstanding principal,
    // so a vault that already has funds out keeps them covered rather than
    // starting from a clean slate that under-states what is owed.
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

    /// The address's deposit-mint ATA, carrying the delegation. Anchor derives
    /// it from `address`, so the account cannot disagree with the argument.
    #[account(
        associated_token::mint = deposit_mint,
        associated_token::authority = address,
        associated_token::token_program = token_program,
    )]
    pub subaccount_ata: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,

    /// The admin's signature, not the operator's. Where admin and operator are
    /// the same key this buys nothing; see the note on `subaccount_count`.
    #[account(
        mut,
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,

    pub system_program: Program<'info, System>,
}
