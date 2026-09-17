// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Attach or detach this vault's withdrawal queue.
//!
//! The field this writes decides who may redeem, so a wrong value is an exit
//! freeze rather than a misconfiguration. `redeem` accepts only the stored key,
//! so a key nobody can sign for strands every holder while deposits keep
//! working. No admin instruction can undo that, because clearing the field
//! requires the stored key's own signature. The two rules below exist to make
//! the state unreachable.

use crate::errors::ErrorCode;
use crate::state::vault::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::Mint;

/// Store `new_authority` as the vault's `withdrawal_queue_authority`, or clear
/// it with the zero key.
///
/// [`withdrawal_queue_pda`] is a pure function of the vault, so exactly one
/// non-zero value is ever storable. The reachable transitions are therefore
/// attach, detach, and an idempotent re-set of the same key. There is no swap to
/// a different queue.
///
/// * **Attach.** The key must be that PDA, and the account there must already
///   exist and be owned by [`WITHDRAWAL_QUEUE_PROGRAM_ID`]. The only storable key
///   is therefore one the queue can sign for, and only once `initialize_queue`
///   has run. Everything else raises `InvalidWithdrawalQueueAuthority`, including
///   a `new_queue` that is missing or does not match the argument.
///
/// * **Detach.** The attached queue must co-sign. The vault never reads queue
///   state, so a signature is how "drained" is expressed, and `release_vault`
///   (WQ-09) will be the only instruction that provides one. Checking the
///   signature and no account is deliberate: requiring the queue's account to
///   still exist would let a queue that closed its own PDA strand the vault
///   permanently.
///
/// The co-sign rule is judged before the new key, so a gated vault reports the
/// rule that protects its queued holders. Anchor's account phase runs before both
/// of them: a non-admin caller receives `NotAdmin`, and an unsigned
/// `current_queue` receives 3010.
pub fn handler(ctx: Context<SetWithdrawalQueueAuthority>, new_authority: Pubkey) -> Result<()> {
    let vault = ctx.accounts.vault_state.key();
    let previous = ctx.accounts.vault_state.withdrawal_queue_authority;

    if let Some(current) = ctx.accounts.vault_state.withdrawal_queue() {
        let co_signer = ctx
            .accounts
            .current_queue
            .as_ref()
            .ok_or(ErrorCode::WithdrawalQueueNotDrained)?;
        require_keys_eq!(
            co_signer.key(),
            current,
            ErrorCode::WithdrawalQueueNotDrained
        );
    }

    if new_authority == Pubkey::default() {
        // This account is rejected rather than ignored. Ignoring it would turn a
        // client that left `new_authority` at its default into a successful
        // no-op, and the operator would read a green transaction as a gated vault
        // that is not in fact gated.
        require!(
            ctx.accounts.new_queue.is_none(),
            ErrorCode::InvalidWithdrawalQueueAuthority
        );
    } else {
        let new_queue = ctx
            .accounts
            .new_queue
            .as_ref()
            .ok_or(ErrorCode::InvalidWithdrawalQueueAuthority)?;
        require_keys_eq!(
            new_queue.key(),
            new_authority,
            ErrorCode::InvalidWithdrawalQueueAuthority
        );
        require_keys_eq!(
            new_authority,
            withdrawal_queue_pda(&vault),
            ErrorCode::InvalidWithdrawalQueueAuthority
        );
        // Together, the owner and the data length mean that the queue program has
        // initialized this account. An address that merely derives correctly is
        // still owned by the system program and holds no data until
        // `initialize_queue` runs, and a key stored before that point could not
        // have been signed for.
        require_keys_eq!(
            *new_queue.owner,
            WITHDRAWAL_QUEUE_PROGRAM_ID,
            ErrorCode::InvalidWithdrawalQueueAuthority
        );
        require!(
            !new_queue.data_is_empty(),
            ErrorCode::InvalidWithdrawalQueueAuthority
        );
    }

    ctx.accounts.vault_state.withdrawal_queue_authority = new_authority;

    emit!(WithdrawalQueueAuthorityUpdated {
        vault,
        previous,
        current: new_authority,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct SetWithdrawalQueueAuthority<'info> {
    #[account(mut, seeds=[VAULT_STATE_SEED.as_ref(), deposit_mint.key().as_ref(), &vault_state.vault_version], bump)]
    pub vault_state: Account<'info, VaultState>,

    pub deposit_mint: InterfaceAccount<'info, Mint>,

    #[account(
        constraint = vault_state.admin == admin.key() @ ErrorCode::NotAdmin
    )]
    pub admin: Signer<'info>,

    /// The account at `new_authority`. It is required when that argument is
    /// non-zero, and rejected when it is zero.
    ///
    /// The handler requires its key to equal `new_authority`, that key to derive
    /// as this vault's queue PDA under `WITHDRAWAL_QUEUE_PROGRAM_ID`, and the
    /// account to be initialized and owned by that program. It is not typed as
    /// the queue's state account because the queue depends on the vault for CPI,
    /// so the reverse edge would be a dependency cycle.
    ///
    /// CHECK: this account is validated in the handler, as described above.
    // Keep that CHECK marker on a single line. Anchor strips only the line
    // carrying the marker, so a multi-line CHECK ships its remainder to the IDL,
    // and to every generated client, as a sentence fragment.
    pub new_queue: Option<UncheckedAccount<'info>>,

    /// The currently attached queue. It is required, and must sign, whenever a
    /// queue is attached. The handler checks its key against the stored authority.
    ///
    /// The type is `Signer` rather than `UncheckedAccount` plus a manual
    /// `is_signer` check, because the field's type is what generates the account
    /// meta. Anchor's CPI and client structs match on that type at codegen,
    /// emitting a literal `true` for a `Signer` field and a literal `false` for an
    /// unchecked one, and neither consults the runtime `AccountInfo`.
    /// `release_vault` can therefore sign through `CpiContext::new_with_signer`
    /// as-is. Had this field been unchecked, the meta would be hardcoded unsigned
    /// and the CPI could not co-sign without patching it by hand.
    pub current_queue: Option<Signer<'info>>,
}

/// This event is emitted on every successful call, including a no-op re-set.
/// Both keys are the raw stored values, so zero reads as "no queue" on either
/// side.
#[event]
pub struct WithdrawalQueueAuthorityUpdated {
    pub vault: Pubkey,
    pub previous: Pubkey,
    pub current: Pubkey,
}
