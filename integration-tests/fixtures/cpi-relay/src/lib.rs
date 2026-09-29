// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Test fixture: forwards one instruction by CPI. Account 0 is the program to
//! call; the rest are its accounts, passed on with their writable and signer
//! flags. Data is the bump of this program's `["relay"]` PDA, then the inner
//! instruction's data. That PDA is marked as a signer wherever it appears and
//! signed for with `invoke_signed`, which is the one power a calling program
//! has that a client does not.

use solana_program::{
    account_info::AccountInfo,
    entrypoint,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
};

/// Seed of the PDA this program signs with.
pub const RELAY_SEED: &[u8] = b"relay";

entrypoint!(process);

pub fn process(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let (bump, inner_data) = data
        .split_first()
        .ok_or(ProgramError::InvalidInstructionData)?;
    let (target, inner_accounts) = accounts
        .split_first()
        .ok_or(ProgramError::NotEnoughAccountKeys)?;
    let seeds: &[&[u8]] = &[RELAY_SEED, core::slice::from_ref(bump)];
    let pda = Pubkey::create_program_address(seeds, program_id)?;
    let metas = inner_accounts
        .iter()
        .map(|a| AccountMeta {
            pubkey: *a.key,
            is_signer: a.is_signer || *a.key == pda,
            is_writable: a.is_writable,
        })
        .collect();
    let ix = Instruction {
        program_id: *target.key,
        accounts: metas,
        data: inner_data.to_vec(),
    };
    invoke_signed(&ix, accounts, &[seeds])
}
