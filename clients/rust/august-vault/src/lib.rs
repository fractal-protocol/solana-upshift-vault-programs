//! Generated Rust client for the august-vault program.
//!
//! Source of truth: `target/idl/august_vault.json`. Regenerate via
//! `pnpm run generate-clients`. Do not edit `src/generated/` by hand.

mod generated;

pub use generated::{accounts::*, errors::*, instructions::*, programs::*};
pub use solana_pubkey::Pubkey;
