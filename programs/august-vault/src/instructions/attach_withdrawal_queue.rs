// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Attach this vault's withdrawal queue.
//!
//! The field this writes decides who may redeem, so every check below exists to
//! keep out a key the queue program has never acted for. The full rationale is
//! on `VaultState::withdrawal_queue_authority`.

use crate::errors::ErrorCode;
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

/// Store this vault's queue PDA as its `withdrawal_queue_authority`.
///
/// `WithdrawalQueueAlreadyAttached` is judged before the account checks, so a
/// gated vault reports its state rather than a bad account. The `queue` account
/// must then sit at [`withdrawal_queue_pda`], be owned by
/// [`WITHDRAWAL_QUEUE_PROGRAM_ID`], and hold data; everything else raises
/// `InvalidWithdrawalQueueAuthority`. Anchor's account phase runs first, so a
/// non-admin caller receives `NotAdmin`.
pub fn handler(ctx: Context<AttachWithdrawalQueue>) -> Result<()> {
    let vault = ctx.accounts.vault_state.key();
    let queue = &ctx.accounts.queue;

    require!(
        ctx.accounts.vault_state.withdrawal_queue().is_none(),
        ErrorCode::WithdrawalQueueAlreadyAttached
    );
    require_keys_eq!(
        queue.key(),
        withdrawal_queue_pda(&vault),
        ErrorCode::InvalidWithdrawalQueueAuthority
    );
    // Together, the owner and the data length mean that the queue program has
    // initialized this account. An address that merely derives correctly is
    // still owned by the system program and holds no data until
    // `initialize_queue` runs, and storing it then would prove nothing about
    // whether the queue program has ever acted for this vault.
    require_keys_eq!(
        *queue.owner,
        WITHDRAWAL_QUEUE_PROGRAM_ID,
        ErrorCode::InvalidWithdrawalQueueAuthority
    );
    require!(
        !queue.data_is_empty(),
        ErrorCode::InvalidWithdrawalQueueAuthority
    );

    ctx.accounts.vault_state.withdrawal_queue_authority = queue.key();

    emit!(WithdrawalQueueAttached {
        vault,
        queue: queue.key(),
    });
    Ok(())
}

#[derive(Accounts)]
pub struct AttachWithdrawalQueue<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,

    /// This vault's queue PDA, `["withdrawal_queue", vault_state]` under the
    /// hardcoded queue program, already initialized by that program.
    ///
    /// CHECK: derivation, owner and non-empty data are validated in the handler.
    // Not typed as the queue's state account; see `WITHDRAWAL_QUEUE_PROGRAM_ID`
    // for the dependency cycle. Keep the CHECK marker on a single line: Anchor
    // strips only the line carrying the marker, so a multi-line CHECK ships its
    // remainder to the IDL, and to every generated client, as a sentence fragment.
    pub queue: UncheckedAccount<'info>,
}

/// `queue` is the key now stored in `withdrawal_queue_authority`.
#[event]
pub struct WithdrawalQueueAttached {
    pub vault: Pubkey,
    pub queue: Pubkey,
}
