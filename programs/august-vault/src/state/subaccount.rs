// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! One permitted operator destination. Its existence at the derived address is
//! the registry, so membership needs no list and no cap.

use anchor_lang::prelude::*;

pub const SUBACCOUNT_SEED: &[u8] = b"SUBACCOUNT";

#[account]
#[derive(Default, InitSpace)]
pub struct Subaccount {
    /// In the seeds, so it cannot be used against another vault; stored for
    /// `getProgramAccounts` indexing.
    pub vault_state: Pubkey,
    /// The receiving address. Its ATA is where funds go.
    pub address: Pubkey,
    /// Principal sent here and not returned. Read by the coverage rule;
    /// deregistration requires zero.
    pub principal: u64,
    pub bump: u8,
    pub padding: [u64; 8],
}

impl Subaccount {
    pub const LEN: usize = 8 + Self::INIT_SPACE;

    pub fn seeds(&self) -> [&[u8]; 4] {
        [
            SUBACCOUNT_SEED,
            self.vault_state.as_ref(),
            self.address.as_ref(),
            std::slice::from_ref(&self.bump),
        ]
    }
}
