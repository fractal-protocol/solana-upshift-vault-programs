// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! The owner adjusts a pending request: the floor, where it pays, and who may
//! finalize it. Shares and timestamps never change here.

use crate::errors::ErrorCode;
use crate::events::WithdrawalRequestUpdated;
use crate::recipient::require_valid_recipient;
use crate::state::*;
use anchor_lang::prelude::*;
use anchor_spl::token_interface::TokenAccount;

/// `expected_sequence` must match the request's stamp; see
/// `WithdrawalRequest::sequence`. An expired request is refused. A field left
/// `None`, or a recipient account left out, is unchanged, and a call that would
/// change nothing is refused rather than confirmed: the generated client sends
/// an omitted account as the program id, so a client that meant to change the
/// recipient and dropped the account would otherwise read a success.
pub fn handler(
    ctx: Context<UpdateRequest>,
    expected_sequence: u64,
    min_assets_out: Option<u64>,
    finalizer: Option<Pubkey>,
) -> Result<()> {
    let request_key = ctx.accounts.request.key();
    require!(
        ctx.accounts.request.sequence == expected_sequence,
        ErrorCode::StaleRequestSequence
    );
    let now = Clock::get()?.unix_timestamp;
    require!(
        !ctx.accounts.request.is_expired(now),
        ErrorCode::RequestExpired
    );
    require!(
        min_assets_out.is_some() || finalizer.is_some() || ctx.accounts.new_recipient.is_some(),
        ErrorCode::NothingToUpdate
    );
    if let Some(recipient) = &ctx.accounts.new_recipient {
        require_valid_recipient(recipient, &ctx.accounts.queue)?;
    }

    let request = &mut ctx.accounts.request;
    if let Some(recipient) = &ctx.accounts.new_recipient {
        request.recipient_token_account = recipient.key();
    }
    if let Some(min) = min_assets_out {
        request.min_assets_out = min;
    }
    if let Some(finalizer) = finalizer {
        request.finalizer = finalizer;
    }

    emit!(WithdrawalRequestUpdated::snapshot(
        request,
        ctx.accounts.queue.vault_state,
        request_key,
    ));
    Ok(())
}

#[derive(Accounts)]
pub struct UpdateRequest<'info> {
    #[account(
        seeds = [WITHDRAWAL_QUEUE_SEED, queue.vault_state.as_ref()],
        bump = queue.bump,
    )]
    pub queue: Box<Account<'info, WithdrawalQueue>>,

    pub owner: Signer<'info>,

    /// The owner's request; the signer must be its owner.
    // Seeds come from the request's own stored fields, so a wrong signer is
    // reported by `has_one = owner` as `NotRequestOwner` rather than as a seeds
    // mismatch (Anchor evaluates seeds before has_one).
    #[account(
        mut,
        seeds = [
            WITHDRAWAL_REQUEST_SEED,
            request.queue.as_ref(),
            request.owner.as_ref(),
            &request.request_id.to_le_bytes(),
        ],
        bump = request.bump,
        has_one = owner @ ErrorCode::NotRequestOwner,
        has_one = queue,
    )]
    pub request: Box<Account<'info, WithdrawalRequest>>,

    /// Present to change where the request pays; validated like the original.
    pub new_recipient: Option<Box<InterfaceAccount<'info, TokenAccount>>>,
}
