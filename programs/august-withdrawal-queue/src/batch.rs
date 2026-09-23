// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Design decision 16: batched instructions take their requests as trailing
//! accounts, `stride` accounts per request with the request itself first, and a
//! parallel `Vec<u64>` of expected sequences. All-or-nothing: the transaction
//! is atomic anyway, so a failing request simply aborts it. Each request is
//! announced in the log before it is touched, so whatever fails afterwards, a
//! CPI included, is attributed to the last announcement.
//!
//! Each batch instruction has a bound, enforced here with `BatchTooLarge`. The
//! binding constraint is heap, not compute or packet size: the program's bump
//! allocator never frees, so every request's CPI and event buffers stay
//! allocated until the instruction ends, and its 32 KB length is fixed at
//! build time, so a larger heap frame requested by the transaction changes
//! nothing (honouring one would take a custom allocator). Past the heap, the
//! runtime's 64-entry instruction trace would cap a finalize batch near ten.
//! Without the bound a caller would see an allocation panic instead of an
//! error code.

use crate::errors::ErrorCode;
use crate::state::WithdrawalRequest;
use anchor_lang::prelude::*;

/// The most requests `expedite_requests` accepts. Each takes about a kilobyte
/// of heap, so the bound sits well inside the 32 KB; a legacy transaction has
/// room for 22, so it is the program that limits an expedite batch.
pub const MAX_EXPEDITE_BATCH: usize = 20;

/// The most requests `finalize_withdrawals` accepts: what the heap holds,
/// measured, the sixth runs out. A legacy transaction with distinct owners
/// holds six, so the heap binds before the wire. The test at the bound is what
/// keeps this number honest against allocation changes.
pub const MAX_FINALIZE_BATCH: usize = 5;

/// Checks the shape of a batch against its bound and returns the request
/// groups in order. Request keys must be strictly ascending, which makes a
/// duplicate unsubmittable: one request could otherwise be expedited twice, or
/// closed twice.
pub fn groups<'info>(
    remaining: &'info [AccountInfo<'info>],
    sequences: &[u64],
    stride: usize,
    max: usize,
) -> Result<Vec<&'info [AccountInfo<'info>]>> {
    require!(!sequences.is_empty(), ErrorCode::EmptyBatch);
    require!(sequences.len() <= max, ErrorCode::BatchTooLarge);
    require!(
        remaining.len() == sequences.len() * stride,
        ErrorCode::BatchLengthMismatch
    );
    let groups: Vec<&[AccountInfo]> = remaining.chunks(stride).collect();
    for pair in groups.windows(2) {
        require!(
            pair[0][0].key() < pair[1][0].key(),
            ErrorCode::RequestsNotSorted
        );
    }
    Ok(groups)
}

/// Loads a request from a trailing account: program-owned, right discriminator,
/// writable, and bound to `queue`. What the `Accounts` derive checks for a
/// declared request field, done by hand for an undeclared one, with the same
/// framework codes so a caller sees no difference between the two forms.
pub fn load_request<'info>(
    info: &'info AccountInfo<'info>,
    queue: &Pubkey,
) -> Result<Account<'info, WithdrawalRequest>> {
    require!(
        info.is_writable,
        anchor_lang::error::ErrorCode::ConstraintMut
    );
    let request = Account::<WithdrawalRequest>::try_from(info)?;
    require_keys_eq!(
        request.queue,
        *queue,
        anchor_lang::error::ErrorCode::ConstraintHasOne
    );
    Ok(request)
}

/// Names the request about to be processed: `request`, then its key, as two
/// log lines. A failing CPI never returns control to this program, so a
/// request cannot be named after the fact; `Pubkey::log` is a syscall and
/// costs a fraction of formatting the key.
pub fn announce(request: &Pubkey) {
    msg!("request");
    request.log();
}
