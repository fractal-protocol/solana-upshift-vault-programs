//! Shared LiteSVM test harness for the August vault and withdrawal-queue programs.
//!
//! Each test owns its own `LiteSVM` instance (no shared state), so tests are
//! parallel-safe. The program binaries are loaded from `$OUT_DIR`, which build.rs
//! compiles from source on every `cargo test` — see build.rs for why they are not
//! taken from `target/deploy`.

pub mod harness;
