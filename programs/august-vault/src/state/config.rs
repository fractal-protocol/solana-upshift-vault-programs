// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use anchor_lang::prelude::*;

/// Seed for the singleton program-config PDA. There is exactly one config
/// account per program deployment, shared by every vault.
pub const PROGRAM_CONFIG_SEED: &[u8] = b"program_config";

/// Program-wide configuration.
///
/// Exists so vault creation is authenticated rather than open to any paying
/// signer. See the namespace note in `initialize.rs` for why a
/// `(deposit_mint, vault_version)` pair cannot be reused once retired.
///
/// The authority is stored here rather than hardcoded so it can be rotated with
/// `set_config_authority` instead of a program upgrade (upgrades require a
/// Fordefi-signed verified deploy).
#[account]
#[derive(Default, InitSpace)]
pub struct ProgramConfig {
    /// The only key permitted to call `initialize`.
    pub authority: Pubkey,
    /// Bump of this PDA, stored so callers need not recompute it.
    pub bump: [u8; 1],
    /// Reserved. Lets a future change add policy fields (for example an
    /// allowlist of approved vault creators) without resizing the account.
    pub padding: [u64; 16],
}

impl ProgramConfig {
    pub const LEN: usize = 8 + Self::INIT_SPACE;

    pub fn init(&mut self, authority: Pubkey, bump: [u8; 1]) {
        self.authority = authority;
        self.bump = bump;
        self.padding = [0; 16];
    }
}
