//! Builds the program bytecode the LiteSVM tests execute, so it cannot be stale.
//!
//! `mainnet_fork_compat.rs` (and the other suites, via `new_svm`) load the program
//! with `include_bytes!("../../target/deploy/august_vault.so")`, resolved at
//! COMPILE time from whatever artifact is on disk. `cargo test` does not rebuild
//! it: this workspace is excluded from the root one, so cargo has no dependency
//! edge from these tests to the program's SBF artifact.
//!
//! The consequence is a silent false pass — edit the program, run `cargo test`
//! without rebuilding, and every test exercises the OLD bytecode and goes green.
//! That is worst in `mainnet_fork_compat.rs`, whose entire job is proving the
//! current program can still operate on live on-chain accounts; a
//! layout-compatibility break is exactly the class of bug a stale artifact hides.
//!
//! CI was safe only by ORDERING: `ci.yml`'s "Build & Test" job runs `anchor build`
//! immediately before the integration tests. That was load-bearing and
//! undocumented — and the same job later rewrites `declare_id!` to a fresh
//! localnet keypair and rebuilds, so reordering those steps would have pointed the
//! fork guard at a localnet-ID binary.
//!
//! ## Why this builds instead of checking
//!
//! This script **creates the missing dependency edge** rather than approximating
//! it: it runs the SBF build, so the artifact the tests embed is by construction
//! the one this source tree produces.
//!
//! Three review rounds killed three successive attempts to *detect* staleness
//! instead, and the failures are recorded here so none is reintroduced:
//!
//! 1. **Hand-picked source paths, compared by mtime.** Cargo fingerprints a
//!    manifest's parsed *contents*, not its mtime, so `touch Cargo.toml` made an
//!    input newer than the artifact while `anchor build` had nothing to relink —
//!    an unclearable failure. A guard that can wedge a build gets deleted.
//! 2. **Hand-picked source paths, compared by content hash, treating changed
//!    artifact bytes as proof of a rebuild.** A rebuild need not change the
//!    output: add a comment to a `#[cfg(test)]` file, run `anchor build`, and the
//!    `.so` is byte-identical — so the guard rejected every run, permanently.
//! 3. **The SBF build's own rustc dep-info.** Correct about sources and immune to
//!    both traps above, but dep-info lists only files rustc read. It cannot see
//!    `[profile.release] overflow-checks`, features, dependency versions or the
//!    lockfile — all of which change SBF codegen. Reproduced: flipping
//!    `overflow-checks` left the guard satisfied against the old artifact.
//!
//! Each attempt guessed at the inputs and was wrong in a new way. Cargo does not
//! have to guess, so the guessing is now cargo's job.
//!
//! ## Consequences worth knowing
//!
//! - **`cargo test` here now requires the SBF toolchain** (`cargo-build-sbf`, from
//!   the Solana CLI). Previously it would run without one — against a possibly
//!   stale artifact. The documented workflow already required `anchor build`
//!   first, so this makes an existing requirement enforceable rather than adding
//!   one.
//! - **`target/deploy/august_vault.so` is build-managed.** A hand-placed artifact
//!   there will be replaced. This is also why the tests do not execute the
//!   reproducible release build: locally and in CI they run `cargo build-sbf`
//!   output, which is a different toolchain from the pinned `solana-verify` Docker
//!   image and hashes differently. Semantically the same program; not the same
//!   bytes. Use `ALLOW_STALE_PROGRAM_SO=1` to test a specific artifact instead.
//! - It is a no-op (~3s) when nothing changed, which is the case in CI, where
//!   `anchor build` has already run.
//!
//! Escape hatch, for deliberately testing a hand-supplied artifact — e.g. a
//! devnet-ID build during an upgrade rehearsal:
//!
//! ```text
//! ALLOW_STALE_PROGRAM_SO=1 cargo test --manifest-path integration-tests/Cargo.toml
//! ```
//!
//! CI must never set it.

use std::path::PathBuf;
use std::process::Command;

const ARTIFACT: &str = "target/deploy/august_vault.so";
const ESCAPE_HATCH: &str = "ALLOW_STALE_PROGRAM_SO";

fn main() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("integration-tests must sit one level below the repository root")
        .to_path_buf();
    let artifact = repo_root.join(ARTIFACT);
    let program_manifest = repo_root.join("programs/august-vault/Cargo.toml");

    println!("cargo:rerun-if-env-changed={ESCAPE_HATCH}");
    // Re-run when anything that feeds the SBF build changes. Cargo walks watched
    // directories recursively, so this covers every program source, and the
    // manifests/lockfile cover profile, feature and dependency changes — the gap
    // that sank the dep-info-only version.
    for path in ["programs", "Cargo.toml", "Cargo.lock", ".cargo/config.toml"] {
        println!("cargo:rerun-if-changed={}", repo_root.join(path).display());
    }

    if std::env::var_os(ESCAPE_HATCH).is_some() {
        println!(
            "cargo:warning={ESCAPE_HATCH} is set — NOT rebuilding {ARTIFACT}. The tests will \
             run whatever bytecode is already there, which may not match this source tree."
        );
        if !artifact.exists() {
            panic!(
                "\n\n{ESCAPE_HATCH} is set but {ARTIFACT} does not exist, so there is no \
                 program to load.\nEither build it (`anchor build`) or unset {ESCAPE_HATCH}.\n\n"
            );
        }
        return;
    }

    let mut cmd = Command::new("cargo");
    cmd.arg("build-sbf")
        .arg("--manifest-path")
        .arg(&program_manifest)
        .current_dir(&repo_root);
    scrub_env(&mut cmd);

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => panic!(
            "\n\n\
             Could not run `cargo build-sbf` ({e}).\n\n\
             These tests embed the program bytecode at compile time, so the SBF toolchain is\n\
             required. It ships with the Solana CLI:\n\n\
             \x20   sh -c \"$(curl -sSfL https://release.anza.xyz/stable/install)\"\n\n\
             To test an existing artifact instead, set {ESCAPE_HATCH}=1.\n\n"
        ),
    };

    if !output.status.success() {
        panic!(
            "\n\n\
             The SBF build of august-vault failed, so the LiteSVM tests have no current\n\
             bytecode to run against. Fix the program, or set {ESCAPE_HATCH}=1 to test the\n\
             existing artifact.\n\n\
             --- cargo build-sbf stderr ---\n{}\n",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    if !artifact.exists() {
        panic!(
            "\n\n\
             `cargo build-sbf` reported success but {ARTIFACT} is missing. The build may be\n\
             writing elsewhere (a CARGO_TARGET_DIR or .cargo/config.toml override).\n\n"
        );
    }
}

/// Strip the variables cargo exports into a build script that would misdirect the
/// nested build — redirecting its output directory, forcing a host target, or
/// leaking this crate's profile and features into the program's build. `CARGO_HOME`
/// and `RUSTUP_HOME` are deliberately kept: the child needs them to resolve
/// toolchains and the registry.
fn scrub_env(cmd: &mut Command) {
    const REMOVE_EXACT: &[&str] = &[
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "RUSTDOCFLAGS",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_TARGET",
        "CARGO_BUILD_TARGET_DIR",
        "CARGO_MAKEFLAGS",
        "CARGO_PRIMARY_PACKAGE",
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_PATH",
        "RUSTC",
        // clippy sets this to clippy-driver, which wraps the HOST stable rustc.
        // Leaking it makes `cargo build-sbf`'s -Z flags fail with "the option `Z`
        // is only accepted on the nightly compiler" — so `cargo clippy` on this
        // crate would break. RUSTC_WRAPPER is deliberately NOT removed: CI sets it
        // to sccache, and dropping it would change the build fingerprint and force
        // a second full SBF rebuild.
        "RUSTC_WORKSPACE_WRAPPER",
        "OUT_DIR",
        "TARGET",
        "HOST",
        "NUM_JOBS",
        "OPT_LEVEL",
        "DEBUG",
        "PROFILE",
    ];
    const REMOVE_PREFIX: &[&str] = &["CARGO_FEATURE_", "CARGO_CFG_", "CARGO_PKG_", "DEP_"];

    for key in REMOVE_EXACT {
        cmd.env_remove(key);
    }
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy().to_string();
        if REMOVE_PREFIX.iter().any(|p| key.starts_with(p)) {
            cmd.env_remove(&key);
        }
    }
}
