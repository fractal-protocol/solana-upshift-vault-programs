// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Queue-level events. Request events arrive with the request instructions.

use crate::state::WithdrawalQueue;
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
    pub accepting_requests: bool,
}

impl QueueConfigUpdated {
    pub fn snapshot(queue: &WithdrawalQueue, key: Pubkey) -> Self {
        Self {
            vault: queue.vault_state,
            queue: key,
            cooldown_seconds: queue.cooldown_seconds,
            fulfillment_window_seconds: queue.fulfillment_window_seconds,
            accepting_requests: queue.accepting_requests,
        }
    }
}
