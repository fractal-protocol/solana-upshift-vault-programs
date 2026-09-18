// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::auth::require_vault_admin;
use crate::errors::ErrorCode;
use crate::events::QueueConfigUpdated;
use crate::instructions::queue_admin::QueueAdmin;
use anchor_lang::prelude::*;

/// Opening requires the vault's gate to point at this queue (design decision
/// 13). Otherwise, after a release, requests could be escrowed into a cooldown
/// that direct redeemers bypass. Closing is drain mode: no new requests, while
/// finalize and cancel keep working.
pub fn handler(ctx: Context<QueueAdmin>, accepting: bool) -> Result<()> {
    require_vault_admin(
        &ctx.accounts.queue,
        &ctx.accounts.vault_state,
        &ctx.accounts.admin,
    )?;
    let key = ctx.accounts.queue.key();
    if accepting {
        require!(
            ctx.accounts.vault_state.withdrawal_queue() == Some(key),
            ErrorCode::QueueNotActiveOnVault
        );
    }
    let queue = &mut ctx.accounts.queue;
    queue.accepting_requests = accepting;
    emit!(QueueConfigUpdated::snapshot(queue, key));
    Ok(())
}
