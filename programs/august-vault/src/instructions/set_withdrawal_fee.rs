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
use anchor_spl::token_interface::Mint;

pub fn handler(ctx: Context<SetWithdrawalFee>, new_fee: u32) -> Result<()> {
    let state = &mut ctx.accounts.vault_state;
    require!(
        new_fee < u32::try_from(10 * FEE_RATE_DENOMINATOR_VALUE as u64 / 100 as u64)?,
        ErrorCode::WithdrawalFeeTooHigh
    );
    state.withdrawal_fee = new_fee;
    Ok(())
}

#[derive(Accounts)]
pub struct SetWithdrawalFee<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,
}
