// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Delayed redemption for `august_vault`.
//!
//! The vault half is live: a vault whose `withdrawal_queue_authority` is set
//! accepts redemptions from that key alone, and one left unset redeems instantly
//! as before. Holders request a withdrawal here and wait out a cooldown; then
//! anyone the request permits finalizes it, and this program redeems the
//! escrowed shares by CPI into the vault's `redeem_checked`, signing as the
//! per-vault PDA the vault was pointed at.
//!
//! This crate carries the program identity, the error ABI pin, the build wiring,
//! the two state accounts (`state`), the admin check (`auth`), the queue's
//! admin instructions, the owner's request and update instructions, and
//! finalization. Cancellation and release arrive with their own changes.
//!
//! **Deployment blocker.** `initialize_queue` is what first makes a queue
//! attachable on the vault side, and detaching needs `release_vault`, which does
//! not exist yet. Deploying this program before it lands would make attaching a
//! one-way door.

pub mod auth;
pub mod errors;
pub mod events;
pub mod instructions;
pub mod mint_policy;
pub mod recipient;
pub mod state;

use instructions::finalize_withdrawal::*;
use instructions::initialize_queue::*;
use instructions::queue_admin::*;
use instructions::request_withdrawal::*;
use instructions::update_request::*;

use anchor_lang::prelude::*;

declare_id!("NmJ9CGaiPJSfAGSdhNeVDkMPGi7ZQMABYYwWUC4GHyf");

// On-chain security contact + source provenance, queryable from the deployed
// program. Gated out of CPI/library builds via `no-entrypoint`, so it ships only
// in the deployable program — the vault does the same.
#[cfg(not(feature = "no-entrypoint"))]
use solana_security_txt::security_txt;

#[cfg(not(feature = "no-entrypoint"))]
security_txt! {
    name: "Upshift Withdrawal Queue (august_withdrawal_queue)",
    project_url: "https://github.com/fractal-protocol/solana-upshift-vault-programs",
    contacts: "email:alex@augustdigital.io,link:https://github.com/fractal-protocol/solana-upshift-vault-programs/security/advisories/new",
    policy: "https://github.com/fractal-protocol/solana-upshift-vault-programs/security/policy",
    source_code: "https://github.com/fractal-protocol/solana-upshift-vault-programs",
    preferred_languages: "en"
}

#[program]
pub mod august_withdrawal_queue {
    use super::*;

    /// Admin creates the vault's withdrawal queue: the queue PDA and its two
    /// escrow token accounts. The deposit mint must be classic
    /// SPL, or Token-2022 carrying at most the two metadata extensions.
    /// Attaching the queue to the vault is a separate, vault-side step, and is
    /// what makes it live.
    ///
    /// ### Parameters
    /// - `cooldown_seconds` - Wait between request and eligibility, at most 30 days
    pub fn initialize_queue(ctx: Context<InitializeQueue>, cooldown_seconds: u64) -> Result<()> {
        return instructions::initialize_queue::handler(ctx, cooldown_seconds);
    }

    /// Admin sets the cooldown for new requests, at most 30 days.
    pub fn set_cooldown(ctx: Context<QueueAdmin>, seconds: u64) -> Result<()> {
        return instructions::set_cooldown::handler(ctx, seconds);
    }

    /// Admin sets how long after scheduled eligibility a new request may still
    /// be finalized: zero disables expiry, otherwise at most 90 days.
    pub fn set_fulfillment_window(ctx: Context<QueueAdmin>, seconds: u64) -> Result<()> {
        return instructions::set_fulfillment_window::handler(ctx, seconds);
    }

    /// A holder escrows `shares` and opens a request that becomes finalizable
    /// after the queue's cooldown, paying `recipient_token_account` the shares'
    /// value at the moment of finalization. Requires the vault's gate to point
    /// at this queue.
    ///
    /// ### Parameters
    /// - `request_id` - Owner-chosen id, unique per owner while the request exists
    /// - `shares` - Shares to escrow, nonzero
    /// - `finalizer` - Who may finalize besides the owner; zero for anyone
    pub fn request_withdrawal(
        ctx: Context<RequestWithdrawal>,
        request_id: u64,
        shares: u64,
        finalizer: Pubkey,
    ) -> Result<()> {
        return instructions::request_withdrawal::handler(ctx, request_id, shares, finalizer);
    }

    /// The owner changes a pending, unexpired request's floor, finalizer, or
    /// recipient (passed as the optional `new_recipient` account). A field left
    /// `None`, or a recipient left out, is unchanged; a call that would change
    /// nothing is refused. `Some(Pubkey::default())` clears the finalizer.
    ///
    /// ### Parameters
    /// - `expected_sequence` - The request's stamp, so a delayed call cannot land
    ///   on a recreated request with the same id
    /// - `min_assets_out` - New floor on the net payout, if changing
    /// - `finalizer` - New request-level finalizer, if changing; zero clears it
    pub fn update_request(
        ctx: Context<UpdateRequest>,
        expected_sequence: u64,
        min_assets_out: Option<u64>,
        finalizer: Option<Pubkey>,
    ) -> Result<()> {
        return instructions::update_request::handler(
            ctx,
            expected_sequence,
            min_assets_out,
            finalizer,
        );
    }

    /// Pays out a request whose cooldown has run and whose window is open: the
    /// queue redeems the escrowed shares by CPI into the vault as its PDA,
    /// forwards the net payout to the request's recipient, and closes the
    /// request with its rent to the owner. Anyone the request's `finalizer` and
    /// the queue's `finalizer_authority` permit may call it, the owner always.
    /// The vault's `VaultPaused`, `NotEnoughLiquidity` and `SlippageExceeded`
    /// propagate unchanged and leave the request pending.
    ///
    /// ### Parameters
    /// - `expected_sequence` - The request's stamp, as for `update_request`
    pub fn finalize_withdrawal(
        ctx: Context<FinalizeWithdrawal>,
        expected_sequence: u64,
    ) -> Result<()> {
        return instructions::finalize_withdrawal::handler(ctx, expected_sequence);
    }
}
