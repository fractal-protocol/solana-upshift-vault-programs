// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Admin authorization, borrowed from the vault rather than stored here.

use crate::errors::ErrorCode;
use crate::state::WithdrawalQueue;
use anchor_lang::prelude::*;
use august_vault::state::vault::VaultState;

/// Requires `admin` to be the current admin of the vault this queue serves.
///
/// The queue keeps no admin key of its own, so a vault admin rotation carries
/// over with nothing to sync. The three facts an admin instruction needs are all
/// checked or typed here: `vault_state` is a genuine vault account (the
/// `Account` wrapper enforces the vault program as owner), it is *this* queue's
/// vault (`VaultMismatch` otherwise, so the admin of another vault cannot act on
/// this queue), and `admin` signed (`Signer`). In `initialize_queue`, write
/// `queue.vault_state` first; the PDA seeds already bind it.
pub fn require_vault_admin(
    queue: &WithdrawalQueue,
    vault_state: &Account<'_, VaultState>,
    admin: &Signer<'_>,
) -> Result<()> {
    require_keys_eq!(
        vault_state.key(),
        queue.vault_state,
        ErrorCode::VaultMismatch
    );
    require_keys_eq!(vault_state.admin, admin.key(), ErrorCode::NotVaultAdmin);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ANCHOR_USER_ERROR_OFFSET;
    use anchor_lang::AccountSerialize;

    fn code_of(err: Error) -> u32 {
        match err {
            Error::AnchorError(e) => e.error_code_number,
            other => panic!("expected an AnchorError, got {other:?}"),
        }
    }

    fn expected(code: ErrorCode) -> u32 {
        code as u32 + ANCHOR_USER_ERROR_OFFSET
    }

    /// Serializes a vault with `admin` and runs the check against it as the
    /// `Account` and `Signer` wrappers a handler would hold.
    fn check(admin: Pubkey, vault_key: Pubkey, queue_vault: Pubkey, signer: Pubkey) -> Result<()> {
        let vault = VaultState {
            admin,
            ..Default::default()
        };
        let mut data = Vec::new();
        vault.try_serialize(&mut data).expect("serialize");
        let owner = august_vault::ID;
        let mut lamports = 0u64;
        let vault_info = AccountInfo::new(
            &vault_key,
            false,
            false,
            &mut lamports,
            &mut data,
            &owner,
            false,
            0,
        );
        let vault_state = Account::<VaultState>::try_from(&vault_info).expect("a genuine vault");

        let system = anchor_lang::system_program::ID;
        let mut signer_lamports = 0u64;
        let mut signer_data: Vec<u8> = Vec::new();
        let signer_info = AccountInfo::new(
            &signer,
            true,
            false,
            &mut signer_lamports,
            &mut signer_data,
            &system,
            false,
            0,
        );
        let admin_signer = Signer::try_from(&signer_info).expect("signed");

        let queue = WithdrawalQueue {
            vault_state: queue_vault,
            ..Default::default()
        };
        require_vault_admin(&queue, &vault_state, &admin_signer)
    }

    #[test]
    fn the_admin_of_this_queues_vault_passes() {
        let admin = Pubkey::new_unique();
        let vault = Pubkey::new_unique();
        check(admin, vault, vault, admin).expect("the admin passes");
    }

    #[test]
    fn anyone_else_is_not_the_admin() {
        let vault = Pubkey::new_unique();
        let err =
            check(Pubkey::new_unique(), vault, vault, Pubkey::new_unique()).expect_err("fails");
        assert_eq!(code_of(err), expected(ErrorCode::NotVaultAdmin));
    }

    /// The admin of another vault is a real admin, just not of this queue's
    /// vault. The binding is judged before the admin key, so this reports the
    /// mismatch rather than the admin.
    #[test]
    fn another_vaults_admin_is_refused_as_a_mismatch() {
        let admin = Pubkey::new_unique();
        let err =
            check(admin, Pubkey::new_unique(), Pubkey::new_unique(), admin).expect_err("fails");
        assert_eq!(code_of(err), expected(ErrorCode::VaultMismatch));
    }
}
