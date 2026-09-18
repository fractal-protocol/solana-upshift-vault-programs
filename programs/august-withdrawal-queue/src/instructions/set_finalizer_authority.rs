// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

use crate::auth::require_vault_admin;
use crate::events::QueueConfigUpdated;
use crate::instructions::queue_admin::QueueAdmin;
use anchor_lang::prelude::*;

/// Applies immediately to every request. Zero lifts the restriction; owners may
/// always finalize their own requests whatever this holds.
pub fn handler(ctx: Context<QueueAdmin>, authority: Pubkey) -> Result<()> {
    require_vault_admin(
        &ctx.accounts.queue,
        &ctx.accounts.vault_state,
        &ctx.accounts.admin,
    )?;
    let key = ctx.accounts.queue.key();
    let queue = &mut ctx.accounts.queue;
    queue.finalizer_authority = authority;
    emit!(QueueConfigUpdated::snapshot(queue, key));
    Ok(())
}
