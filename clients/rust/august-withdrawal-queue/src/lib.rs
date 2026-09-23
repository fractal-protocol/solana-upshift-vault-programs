// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Generated Rust client for `august_withdrawal_queue`.
//!
//! Everything under `generated/` comes from the program's IDL via Codama —
//! regenerate with `pnpm run generate-clients` rather than editing it.
//!
//! Hand-written helpers belong here, outside `generated/`, the way the vault
//! client carries `resolved_share_offset` and `min_first_deposit`. There are
//! none yet.

pub mod generated;

pub use generated::{accounts::*, errors::*, instructions::*, programs::*};
pub use solana_pubkey::Pubkey;

#[cfg(test)]
mod tests {
    use super::*;

    /// The generated struct derives no `Default`, so build it field-by-field:
    /// adding a field to `WithdrawalQueue` then fails to compile here rather than
    /// silently skipping the new field.
    #[test]
    fn a_fresh_queue_literal_has_empty_counters() {
        let zero = Pubkey::default();
        let queue = WithdrawalQueue {
            discriminator: [0; 8],
            vault_state: zero,
            deposit_mint: zero,
            share_mint: zero,
            escrow_shares: zero,
            escrow_assets: zero,
            cooldown_seconds: 0,
            fulfillment_window_seconds: 0,
            sequence: 0,
            pending_requests: 0,
            pending_shares: 0,
            bump: 0,
            padding: [0; 20],
        };
        assert_eq!(
            (queue.sequence, queue.pending_requests, queue.pending_shares),
            (0, 0, 0)
        );
    }

    /// Same guard for the request account.
    #[test]
    fn a_zero_request_literal_never_expires() {
        let zero = Pubkey::default();
        let request = WithdrawalRequest {
            discriminator: [0; 8],
            queue: zero,
            owner: zero,
            recipient_token_account: zero,
            finalizer: zero,
            shares: 0,
            request_id: 0,
            sequence: 0,
            requested_at: 0,
            scheduled_eligible_at: 0,
            eligible_at: 0,
            expires_at: 0,
            bump: 0,
            padding: [0; 9],
        };
        assert_eq!(request.expires_at, 0, "zero means no deadline");
    }
}
