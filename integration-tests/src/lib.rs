//! Shared LiteSVM test harness for the August vault program.
//!
//! Each test owns its own `LiteSVM` instance (no shared state), so tests are
//! parallel-safe. The program binary is loaded from
//! `target/deploy/august_vault.so` — run `anchor build` before `cargo test`.

pub mod harness;
