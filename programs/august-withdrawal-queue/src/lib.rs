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
//! **No instructions yet.** The crate carries the program identity, the error ABI
//! pin, the build wiring, the two state accounts (`state`) and the admin check
//! (`auth`); the instructions arrive with their own changes. An empty
//! `#[program]` module still produces a deployable artifact.

pub mod auth;
pub mod errors;
pub mod state;

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
pub mod august_withdrawal_queue {}
