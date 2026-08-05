//! Shared LiteSVM test harness for the August vault program.
//!
//! Each test owns its own `LiteSVM` instance (no shared state), so tests are
//! parallel-safe. The program binary is loaded from
//! `$OUT_DIR/august_vault.so`, which build.rs compiles from source on every
//! `cargo test` — see build.rs for why it is not `target/deploy`.

pub mod harness;
