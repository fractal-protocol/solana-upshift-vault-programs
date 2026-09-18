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
//! as before. What is missing is this side — holders will request a withdrawal
//! here, wait out a cooldown, and the request will be finalized by CPI into the
//! vault's `redeem_checked`, with this program's per-vault PDA signing as the
//! authority the vault was pointed at.
//!
//! This crate carries the program identity, the error ABI pin, the build wiring,
//! the two state accounts (`state`), the admin check (`auth`), the queue's
//! admin instructions, and the owner's request and update instructions.
//! Finalization, cancellation and release arrive with their own changes.
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
    /// after the queue's cooldown, paying `recipient_token_account` at least
    /// `min_assets_out`. Requires the queue to be accepting and the vault's gate
    /// to point at it.
    ///
    /// ### Parameters
    /// - `request_id` - Owner-chosen id, unique per owner while the request exists
    /// - `shares` - Shares to escrow, nonzero
    /// - `min_assets_out` - Floor on the net payout
    /// - `finalizer` - Who may finalize besides the owner; zero for anyone
    pub fn request_withdrawal(
        ctx: Context<RequestWithdrawal>,
        request_id: u64,
        shares: u64,
        min_assets_out: u64,
        finalizer: Pubkey,
    ) -> Result<()> {
        return instructions::request_withdrawal::handler(
            ctx,
            request_id,
            shares,
            min_assets_out,
            finalizer,
        );
    }

    /// The owner changes a pending request's floor, finalizer, or recipient
    /// (passed as the optional `new_recipient` account). `expected_sequence`
    /// must match the request's stamp.
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
}
