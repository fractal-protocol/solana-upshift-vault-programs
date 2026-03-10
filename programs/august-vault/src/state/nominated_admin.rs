// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE.BSL file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::errors::ErrorCode;
use anchor_lang::prelude::*;

#[account(zero_copy)]
#[repr(C)]
#[derive(Default, InitSpace)]

pub struct NominatedAdmin {
    nominated_admin: Pubkey,
    valid_until: i64,
}

pub const NOMINATED_ADMIN_PDA_SEED: &[u8] = b"nominated_admin";

impl NominatedAdmin {
    pub const LEN: usize = 8 + Self::INIT_SPACE;

    pub fn initialize(&mut self, nominated_admin: Pubkey) {
        const ONE_DAY_IN_SECONDS: i64 = 24 * 60 * 60;
        let now = Clock::get().unwrap().unix_timestamp;
        self.nominated_admin = nominated_admin;
        self.valid_until = now + ONE_DAY_IN_SECONDS;
    }

    pub fn valid_accept_nomination(&self, signer: &Pubkey) -> Result<()> {
        if !self.is_nominated_admin(signer) {
            return Err(ErrorCode::InvalidNominatedAdmin.into());
        }
        if !self.is_still_valid() {
            return Err(ErrorCode::NominationExpired.into());
        }
        Ok(())
    }

    pub fn is_nominated_admin(&self, key: &Pubkey) -> bool {
        self.nominated_admin.eq(key)
    }

    pub fn is_still_valid(&self) -> bool {
        let now = Clock::get().unwrap().unix_timestamp;
        self.valid_until > now
    }

    pub fn valid_until(&self) -> i64 {
        self.valid_until
    }

    pub fn nominated_admin(&self) -> Pubkey {
        self.nominated_admin
    }
}
