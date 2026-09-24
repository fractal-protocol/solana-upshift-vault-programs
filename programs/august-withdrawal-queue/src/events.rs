// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Queue-level and request-level events. Every request event carries the
//! request's full identity: vault, queue, request PDA, owner-scoped id, owner
//! and sequence stamp, so history is keyed by (request, sequence); one
//! transaction can finalize several requests, so a signature is not a key.

use crate::state::{WithdrawalQueue, WithdrawalRequest};
use anchor_lang::prelude::*;

/// Emitted once per vault, when its queue is created.
#[event]
pub struct QueueInitialized {
    pub vault: Pubkey,
    pub queue: Pubkey,
    pub deposit_mint: Pubkey,
    pub share_mint: Pubkey,
    pub escrow_shares: Pubkey,
    pub escrow_assets: Pubkey,
    pub cooldown_seconds: u64,
}

/// Emitted by every admin setter with the whole configuration after the change,
/// so an indexer needs no earlier state to know the current one.
#[event]
pub struct QueueConfigUpdated {
    pub vault: Pubkey,
    pub queue: Pubkey,
    pub cooldown_seconds: u64,
    pub fulfillment_window_seconds: u64,
}

/// A request was created and its shares escrowed.
#[event]
pub struct WithdrawalRequested {
    pub vault: Pubkey,
    pub queue: Pubkey,
    pub request: Pubkey,
    pub request_id: u64,
    pub owner: Pubkey,
    pub sequence: u64,
    pub shares: u64,
    pub recipient_token_account: Pubkey,
    pub finalizer: Pubkey,
    pub eligible_at: i64,
    pub expires_at: i64,
}

impl WithdrawalRequested {
    pub fn snapshot(request: &WithdrawalRequest, vault: Pubkey, key: Pubkey) -> Self {
        Self {
            vault,
            queue: request.queue,
            request: key,
            request_id: request.request_id,
            owner: request.owner,
            sequence: request.sequence,
            shares: request.shares,
            recipient_token_account: request.recipient_token_account,
            finalizer: request.finalizer,
            eligible_at: request.eligible_at,
            expires_at: request.expires_at,
        }
    }
}

/// A request was paid and closed. `assets` is what left the escrow for the
/// recipient: the vault's net payout, never a balance that already sat there.
#[event]
pub struct WithdrawalFinalized {
    pub vault: Pubkey,
    pub queue: Pubkey,
    pub request: Pubkey,
    pub request_id: u64,
    pub owner: Pubkey,
    pub sequence: u64,
    pub recipient: Pubkey,
    pub finalizer: Pubkey,
    pub shares: u64,
    pub assets: u64,
}

impl WithdrawalFinalized {
    pub fn snapshot(
        request: &WithdrawalRequest,
        vault: Pubkey,
        key: Pubkey,
        finalizer: Pubkey,
        assets: u64,
    ) -> Self {
        Self {
            vault,
            queue: request.queue,
            request: key,
            request_id: request.request_id,
            owner: request.owner,
            sequence: request.sequence,
            recipient: request.recipient_token_account,
            finalizer,
            shares: request.shares,
            assets,
        }
    }
}

/// A request was cancelled and its shares returned. `by` is the signer: the
/// owner, or the admin through `admin_cancel_withdrawal`.
#[event]
pub struct WithdrawalCancelled {
    pub vault: Pubkey,
    pub queue: Pubkey,
    pub request: Pubkey,
    pub request_id: u64,
    pub owner: Pubkey,
    pub sequence: u64,
    pub by: Pubkey,
    pub shares: u64,
    pub destination: Pubkey,
}

impl WithdrawalCancelled {
    pub fn snapshot(
        request: &WithdrawalRequest,
        vault: Pubkey,
        key: Pubkey,
        by: Pubkey,
        destination: Pubkey,
    ) -> Self {
        Self {
            vault,
            queue: request.queue,
            request: key,
            request_id: request.request_id,
            owner: request.owner,
            sequence: request.sequence,
            by,
            shares: request.shares,
            destination,
        }
    }
}

/// The admin or operator moved a request's eligibility to now (decision 15).
#[event]
pub struct WithdrawalExpedited {
    pub vault: Pubkey,
    pub queue: Pubkey,
    pub request: Pubkey,
    pub request_id: u64,
    pub owner: Pubkey,
    pub sequence: u64,
    pub by: Pubkey,
    pub previous_eligible_at: i64,
    pub new_eligible_at: i64,
}

impl WithdrawalExpedited {
    pub fn snapshot(
        request: &WithdrawalRequest,
        vault: Pubkey,
        key: Pubkey,
        by: Pubkey,
        previous_eligible_at: i64,
    ) -> Self {
        Self {
            vault,
            queue: request.queue,
            request: key,
            request_id: request.request_id,
            owner: request.owner,
            sequence: request.sequence,
            by,
            previous_eligible_at,
            new_eligible_at: request.eligible_at,
        }
    }
}

/// The vault was returned to instant redemption with this much still pending.
#[event]
pub struct VaultReleased {
    pub vault: Pubkey,
    pub queue: Pubkey,
    pub pending_requests: u64,
    pub pending_shares: u64,
}

impl QueueConfigUpdated {
    pub fn snapshot(queue: &WithdrawalQueue, key: Pubkey) -> Self {
        Self {
            vault: queue.vault_state,
            queue: key,
            cooldown_seconds: queue.cooldown_seconds,
            fulfillment_window_seconds: queue.fulfillment_window_seconds,
        }
    }
}
