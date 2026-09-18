// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Design decision 5: where a request may be paid out.

use crate::errors::ErrorCode;
use crate::state::WithdrawalQueue;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::TokenAccount;
use august_vault::state::vault::VaultState;

/// The recipient must hold the deposit mint and must not belong to the queue or
/// the vault. Paying an escrow would make the payout an SPL self-transfer that
/// succeeds without moving anything, shares burned and assets stranded; paying
/// the vault's reserve would hand the payout straight back. Authority is what
/// identifies both: every token account the queue or the vault controls has one
/// of them as owner, whatever its address.
pub fn require_valid_recipient(
    recipient: &InterfaceAccount<'_, TokenAccount>,
    queue: &Account<'_, WithdrawalQueue>,
    vault_state: &Account<'_, VaultState>,
) -> Result<()> {
    require_keys_eq!(
        recipient.mint,
        queue.deposit_mint,
        ErrorCode::InvalidRecipient
    );
    require_keys_neq!(recipient.owner, queue.key(), ErrorCode::InvalidRecipient);
    require_keys_neq!(
        recipient.owner,
        vault_state.key(),
        ErrorCode::InvalidRecipient
    );
    Ok(())
}
