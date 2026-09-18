// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Queue-level and request-level events. Every request event carries the
//! request's full identity: vault, queue, request PDA, owner-scoped id, owner
//! and sequence stamp, so history is keyed by (request, sequence) and never by
//! transaction signature.

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
    pub min_assets_out: u64,
    pub recipient_token_account: Pubkey,
    pub finalizer: Pubkey,
    pub eligible_at: i64,
    pub expires_at: i64,
}

/// The owner changed a pending request. Carries the three updatable fields as
/// they now stand, whether or not each changed.
#[event]
pub struct WithdrawalRequestUpdated {
    pub vault: Pubkey,
    pub queue: Pubkey,
    pub request: Pubkey,
    pub request_id: u64,
    pub owner: Pubkey,
    pub sequence: u64,
    pub min_assets_out: u64,
    pub recipient_token_account: Pubkey,
    pub finalizer: Pubkey,
}

impl WithdrawalRequestUpdated {
    pub fn snapshot(request: &WithdrawalRequest, vault: Pubkey, key: Pubkey) -> Self {
        Self {
            vault,
            queue: request.queue,
            request: key,
            request_id: request.request_id,
            owner: request.owner,
            sequence: request.sequence,
            min_assets_out: request.min_assets_out,
            recipient_token_account: request.recipient_token_account,
            finalizer: request.finalizer,
        }
    }
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
