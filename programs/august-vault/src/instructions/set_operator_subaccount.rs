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
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

pub fn handler(ctx: Context<SetOperatorSubaccount>, new_subaccount: Pubkey) -> Result<()> {
    let state = &mut ctx.accounts.vault_state;

    match &ctx.accounts.subaccount_ata {
        // No ATA to inspect, so this may only be the rollback to zero.
        None => require!(
            new_subaccount == Pubkey::default(),
            ErrorCode::InvalidOperatorSubaccount
        ),
        Some(ata) => {
            // A delegation is the only on-chain proof the address can return
            // funds, and it needs the owner's signature — so unlike ATA
            // existence, which `create_associated_token_account` lets any third
            // party manufacture, it cannot be arranged on the owner's behalf.
            //
            // It also subsumes judging the address by shape, which misses an
            // uncreated ATA address (System-owned and empty, so it reads as a
            // wallet) and wrongly refuses an SPL multisig that can sign. See
            // README. Program-owned addresses are excluded only because this
            // program issues no `Approve` CPI — if one is ever added, name them
            // explicitly again.
            require!(
                ata.delegate == COption::Some(state.key()) && ata.delegated_amount > 0,
                ErrorCode::SubaccountDelegationMissing
            );

            // Role hygiene, and the one thing the proof cannot see: the
            // operator may perfectly well have delegated its own ATA. Skipped
            // on the rollback path, or a vault whose operator was zeroed could
            // never roll back.
            require!(
                new_subaccount != state.operator,
                ErrorCode::InvalidOperatorSubaccount
            );
        }
    }

    state.operator_subaccount = new_subaccount;
    Ok(())
}

#[derive(Accounts)]
#[instruction(new_subaccount: Pubkey)]
pub struct SetOperatorSubaccount<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    /// The named address's deposit-mint ATA, carrying the delegation that proves
    /// the address can return funds. Anchor derives it from `new_subaccount`, so
    /// the account cannot disagree with the argument. Omitted only for the zero
    /// rollback.
    #[account(
        associated_token::mint = deposit_mint,
        associated_token::authority = new_subaccount,
        associated_token::token_program = token_program,
    )]
    pub subaccount_ata: Option<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,

    /// The admin's signature, not the operator's. Where admin and operator are
    /// the same key this buys nothing; see the note on the field.
    #[account(
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,
}
