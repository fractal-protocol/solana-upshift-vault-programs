// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! Generated Rust client for `august_withdrawal_queue`.
//!
//! Everything under `generated/` comes from the program's IDL via Codama —
//! regenerate with `pnpm run generate-clients` rather than editing it.
//!
//! Hand-written helpers belong here, outside `generated/`, the way the vault
//! client carries `resolved_share_offset` and `min_first_deposit`. There are
//! none yet because the program has no state accounts yet.

// Public, unlike the vault client's, because nothing here would catch a missed
// re-export: this crate sets `unused = "allow"` for Codama's output, so an
// unreachable `pub mod accounts;` raises no `dead_code` warning and `cargo check
// -D warnings` stays green while the client exposes nothing. Keeping the module
// public makes the re-export below a convenience rather than the only way in.
pub mod generated;

// Only `errors` and `programs` exist while the program is a scaffold. Codama emits
// `accounts` and `instructions` modules as soon as there are any; widen this then.
pub use generated::{errors::*, programs::*};
pub use solana_pubkey::Pubkey;
