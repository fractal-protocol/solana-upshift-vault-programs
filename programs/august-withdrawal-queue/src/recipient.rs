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

/// Must hold the deposit mint and be owned by neither PDA. Paying `escrow_assets`
/// would turn finalize's payout into an SPL self-transfer, a successful no-op with
/// shares burned and assets stranded; paying the reserve would return it to the
/// vault. The authority check covers every account either PDA controls, whatever
/// its address; the address check names the one account the hazard is about. A
/// classic-SPL recipient can be reassigned to a PDA afterwards, which strands
/// only the owner's own payout; finalize re-validates before paying.
pub fn require_valid_recipient(
    recipient: &InterfaceAccount<'_, TokenAccount>,
    queue: &Account<'_, WithdrawalQueue>,
) -> Result<()> {
    require_keys_eq!(
        recipient.mint,
        queue.deposit_mint,
        ErrorCode::InvalidRecipient
    );
    require_keys_neq!(
        recipient.key(),
        queue.escrow_assets,
        ErrorCode::InvalidRecipient
    );
    require_keys_neq!(recipient.owner, queue.key(), ErrorCode::InvalidRecipient);
    require_keys_neq!(
        recipient.owner,
        queue.vault_state,
        ErrorCode::InvalidRecipient
    );
    Ok(())
}
