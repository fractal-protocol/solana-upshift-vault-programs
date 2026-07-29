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
use anchor_spl::token_interface::Mint;

pub fn handler(ctx: Context<OperatorUpdateAum>, new_aum: u64) -> Result<()> {
    let state = &mut ctx.accounts.vault_state;

    // Calculate limits using configurable values (in basis points)
    let decrease_limit = (BPS_DENOMINATOR - state.aum_decrease_limit) as u128;
    let increase_limit = (BPS_DENOMINATOR + state.aum_increase_limit) as u128;

    // Widen before multiplying. As u64 these products overflow once
    // `deployed_aum` passes `u64::MAX / (10000 + increase_limit)` — about 1.84M
    // whole tokens on a 9-decimal mint with the default 20 bps, and only ~922k
    // if an admin widens the limits to their 10000 bps maximum. Past that point
    // every legitimate AUM report aborts, freezing yield reporting for the vault
    // (the program is built with `overflow-checks`, so the multiplication panics
    // rather than wrapping — see the note on `BPS_DENOMINATOR`). In `u128` the
    // worst case is ~3.7e23 against a ~3.4e38 ceiling, so no u64 input can
    // overflow these comparisons.
    let scaled_new_aum = (new_aum as u128) * (BPS_DENOMINATOR as u128);
    let deployed_aum = state.deployed_aum as u128;

    require!(
        scaled_new_aum >= decrease_limit * deployed_aum,
        ErrorCode::AumDecreaseTooBig
    );
    require!(
        scaled_new_aum <= increase_limit * deployed_aum,
        ErrorCode::AumIncreaseTooBig
    );

    state.deployed_aum = new_aum;
    Ok(())
}

#[derive(Accounts)]
pub struct OperatorUpdateAum<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint = vault_state.operator == operator.key() @ ErrorCode::NotOperator
    )]
    pub operator: Signer<'info>,
}
