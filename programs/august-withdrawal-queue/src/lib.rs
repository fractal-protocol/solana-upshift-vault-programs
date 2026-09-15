// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Delayed redemption for `august_vault`.
//!
//! A vault whose `withdrawal_queue_authority` names this program's per-vault PDA
//! accepts redemptions from that PDA only. Holders request a withdrawal here,
//! wait out a cooldown, and the request is finalized by CPI into the vault's
//! `redeem_checked` with the PDA signing.
//!
//! **This crate is a scaffold.** It carries the program identity, the error ABI
//! pin and the build wiring; the state accounts and instructions arrive with
//! their own changes. An empty `#[program]` module still produces a deployable
//! artifact, which is what lets the two-program build, CI and test harness be
//! stood up and reviewed before any queue logic exists.

pub mod errors;

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
