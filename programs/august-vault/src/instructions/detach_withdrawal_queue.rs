// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Detach this vault's withdrawal queue, restoring direct redemption.
//!
//! When queue mode may end is the queue's decision, not the vault's: the vault
//! never reads queue state, so that decision is expressed as a signature. The
//! attached queue must co-sign, and its `release_vault` is the only instruction
//! that provides one, after its own precondition on the pending set. Pending
//! requests survive a detach and finalize or cancel afterwards.

use crate::errors::ErrorCode;
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

/// Clear `withdrawal_queue_authority`, with the attached queue's signature.
///
/// The `queue` signer must equal the stored key, else
/// `WrongWithdrawalQueueSigner`. A vault with no queue attached is refused with
/// `WithdrawalQueueNotAttached`, judged first among the handler's checks, so a
/// caller who detaches twice learns the vault is already open rather than that
/// the signer is wrong.
///
/// Only the signature is checked, and no account. Requiring the queue's account
/// to still exist would let a queue that closed its own PDA strand the vault
/// permanently. Anchor's account phase runs before both checks: a non-admin
/// caller receives `NotAdmin`, and an unsigned `queue` receives 3010.
pub fn handler(ctx: Context<DetachWithdrawalQueue>) -> Result<()> {
    let vault = ctx.accounts.vault_state.key();
    let Some(current) = ctx.accounts.vault_state.withdrawal_queue() else {
        return err!(ErrorCode::WithdrawalQueueNotAttached);
    };
    require_keys_eq!(
        ctx.accounts.queue.key(),
        current,
        ErrorCode::WrongWithdrawalQueueSigner
    );

    ctx.accounts.vault_state.withdrawal_queue_authority = Pubkey::default();

    emit!(WithdrawalQueueDetached {
        vault,
        queue: current,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct DetachWithdrawalQueue<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,

    /// The attached queue, which must sign. The handler checks its key against
    /// the stored authority.
    // `Signer` rather than `UncheckedAccount` plus a manual `is_signer` check,
    // because the field's type is what generates the account meta: Anchor's CPI
    // and client structs emit a literal `true` for a `Signer` field and a literal
    // `false` for an unchecked one, without consulting the runtime `AccountInfo`.
    // `release_vault` can therefore sign through `CpiContext::new_with_signer`
    // as-is.
    pub queue: Signer<'info>,
}

/// `queue` is the key that was stored, and has just been cleared.
#[event]
pub struct WithdrawalQueueDetached {
    pub vault: Pubkey,
    pub queue: Pubkey,
}
