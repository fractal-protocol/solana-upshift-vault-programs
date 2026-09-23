// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! The account shape every admin setter shares.

use crate::errors::ErrorCode;
use crate::state::*;
use anchor_lang::prelude::*;
use august_vault::state::vault::VaultState;

/// Admin setters on an existing queue. The handler calls
/// `crate::auth::require_vault_admin`, which binds the vault to the queue and
/// checks the admin key; `has_one` catches the same binding one phase earlier
/// and reports it as `VaultMismatch`. The seeds are taken from the queue's own
/// stored vault, not the passed account, so that check speaks only to the stored
/// bump: a queue created with a wrong bump fails here, on its first admin call,
/// rather than at the first PDA-signed transfer.
#[event_cpi]
#[derive(Accounts)]
pub struct QueueAdmin<'info> {
    #[account(
        mut,
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
        has_one = vault_state @ ErrorCode::VaultMismatch,
    )]
    pub queue: Account<'info, WithdrawalQueue>,

    pub vault_state: Account<'info, VaultState>,

    pub admin: Signer<'info>,
}
