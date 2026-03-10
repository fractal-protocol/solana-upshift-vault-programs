// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE.BSL file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! The vault state account is the main account of the vault.
//! It stores the operator pubkey and the paused state

use anchor_lang::prelude::*;

pub const VAULT_STATE_SEED: &[u8] = b"VAULT_STATE";

pub const FEE_RATE_DENOMINATOR_VALUE: u32 = 1_000_000;
pub const EXTRA_SHARES: u128 = 1;

#[account]
#[derive(Default, InitSpace)]
pub struct VaultState {
    pub operator: Pubkey,
    pub admin: Pubkey,
    pub share_mint: Pubkey,
    pub deposit_mint: Pubkey,
    pub fee_recipient: Pubkey,
    pub withdrawal_fee: u32,
    pub local_aum: u64,
    pub deployed_aum: u64,
    pub aum_increase_limit: u32, // Basis points (e.g., 20 = 0.2%)
    pub aum_decrease_limit: u32, // Basis points (e.g., 20 = 0.2%)
    pub pda_bump: [u8; 1],
    pub vault_version: [u8; 1], // Version number for vault PDAs (allows multiple vaults per deposit mint)
    pub paused: bool,
    pub padding: [u64; 32],
}

impl VaultState {
    pub const LEN: usize = 8 + Self::INIT_SPACE;
    /// Initialize the vault state
    pub fn init(
        &mut self,
        operator: Pubkey,
        admin: Pubkey,
        share_mint: Pubkey,
        deposit_mint: Pubkey,
        fee_recipient: Pubkey,
        withdrawal_fee: u32,
        pda_bump: [u8; 1],
        vault_version: [u8; 1],
    ) {
        self.operator = operator;
        self.admin = admin;
        self.share_mint = share_mint;
        self.deposit_mint = deposit_mint;
        self.fee_recipient = fee_recipient;
        self.withdrawal_fee = withdrawal_fee;
        self.deployed_aum = 0;
        self.aum_increase_limit = 20; // Default: 0.2% (20 basis points)
        self.aum_decrease_limit = 20; // Default: 0.2% (20 basis points)
        self.paused = false;
        self.pda_bump = pda_bump;
        self.vault_version = vault_version;
    }

    /// Get the seed for the vault state PDA
    pub fn seeds(&self) -> [&[u8]; 4] {
        [
            VAULT_STATE_SEED.as_ref(),
            self.deposit_mint.as_ref(),
            &self.vault_version,
            &self.pda_bump,
        ]
    }
}
