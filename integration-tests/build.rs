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
//! - **The tests embed `$OUT_DIR/august_vault.so`, not `target/deploy`.** The build
//!   writes into a fresh `--sbf-out-dir` under `OUT_DIR` and that file is what gets
//!   embedded, so the bytes provably come from this invocation. Checking that
//!   `target/deploy/august_vault.so` merely *exists* was not enough: `cargo
//!   build-sbf` skips its copy step when compilation is cached, so a stale or
//!   hand-placed artifact there survived and was embedded. `target/deploy` is now
//!   left alone entirely — a release artifact parked there is no longer clobbered.
//! - **These tests do not execute the reproducible release build.** They run
//!   `cargo build-sbf` output, a different toolchain from the pinned
//!   `solana-verify` Docker image, so it hashes differently. Same program, not the
//!   same bytes. `ALLOW_STALE_PROGRAM_SO=1` embeds `target/deploy` as-is instead.
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

use std::path::{Path, PathBuf};
use std::process::Command;

const ARTIFACT_NAME: &str = "august_vault.so";
const PARKED_ARTIFACT: &str = "target/deploy/august_vault.so";
const ESCAPE_HATCH: &str = "ALLOW_STALE_PROGRAM_SO";

fn main() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("integration-tests must sit one level below the repository root")
        .to_path_buf();
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    // What the tests `include_bytes!`.
    let embedded = out_dir.join(ARTIFACT_NAME);
    let program_manifest = repo_root.join("programs/august-vault/Cargo.toml");

    println!("cargo:rerun-if-env-changed={ESCAPE_HATCH}");
    // Re-run when anything that feeds the SBF build changes. Cargo walks watched
    // directories recursively, so this covers every program source, and the
    // manifests/lockfile cover profile, feature and dependency changes.
    for path in ["programs", "Cargo.toml", "Cargo.lock", ".cargo/config.toml"] {
        println!("cargo:rerun-if-changed={}", repo_root.join(path).display());
    }

    if std::env::var_os(ESCAPE_HATCH).is_some() {
        let parked = repo_root.join(PARKED_ARTIFACT);
        println!(
            "cargo:warning={ESCAPE_HATCH} is set — NOT building. Embedding {PARKED_ARTIFACT} \
             as-is, which may not match this source tree."
        );
        if let Err(e) = install(&parked, &embedded) {
            panic!(
                "\n\n{ESCAPE_HATCH} is set, but {PARKED_ARTIFACT} could not be used ({e}).\n\
                 Put the artifact you want to test there, or unset {ESCAPE_HATCH}.\n\n"
            );
        }
        return;
    }

    // Assert the checked-in lockfile actually resolves BEFORE building. The outer
    // --locked covers only the integration-tests workspace, and `cargo build-sbf`
    // does NOT honour `-- --locked` (verified: it updated the root Cargo.lock
    // anyway), so without this the nested build could silently re-resolve and the
    // bytecode under test would come from an uncommitted dependency set.
    // `cargo metadata` is real cargo, so it does honour it, and it touches nothing.
    let lock_path = repo_root.join("Cargo.lock");
    let lock_before = std::fs::read(&lock_path).ok();
    let mut meta = Command::new("cargo");
    meta.arg("metadata")
        .arg("--locked")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(&program_manifest)
        .current_dir(&repo_root);
    scrub_env(&mut meta);
    match meta.output() {
        Ok(o) if !o.status.success() => panic!(
            "\n\n\
             The root Cargo.lock does not resolve the program's dependencies, so the bytecode\n\
             would be built from an uncommitted dependency set.\n\n\
             Update and commit the lockfile (`cargo update -p <crate>` or `cargo check`), then\n\
             re-run.\n\n\
             --- cargo metadata --locked ---\n{}\n",
            String::from_utf8_lossy(&o.stderr)
        ),
        Ok(_) => {}
        // Do not fail the build if metadata itself could not run; the post-build
        // lockfile comparison below still catches a re-resolve.
        Err(e) => println!("cargo:warning=could not pre-check the lockfile ({e})"),
    }

    // Build into a directory that is emptied first, so the artifact found there
    // afterwards cannot be a leftover from an earlier run. `cargo build-sbf` copies
    // into --sbf-out-dir unconditionally, including when compilation is cached, so
    // its presence is proof this invocation produced it.
    let staging = out_dir.join("sbf-out");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).expect("create the SBF staging directory");

    let mut cmd = Command::new("cargo");
    cmd.arg("build-sbf")
        .arg("--manifest-path")
        .arg(&program_manifest)
        .arg("--sbf-out-dir")
        .arg(&staging)
        // Forwarded in case a future cargo-build-sbf honours it; today it does
        // not, which is why the lockfile is checked separately above and below.
        .arg("--")
        .arg("--locked")
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
             To embed an existing artifact instead, set {ESCAPE_HATCH}=1.\n\n"
        ),
    };

    if !output.status.success() {
        panic!(
            "\n\n\
             The SBF build of august-vault failed, so the LiteSVM tests have no current\n\
             bytecode to run against. Fix the program, or set {ESCAPE_HATCH}=1 to embed the\n\
             existing {PARKED_ARTIFACT}.\n\n\
             --- cargo build-sbf stderr ---\n{}\n",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Belt and braces: prove the build did not re-resolve. Cheap, and the only
    // check that actually observes the outcome rather than the intent.
    if let (Some(before), Ok(after)) = (&lock_before, std::fs::read(&lock_path)) {
        if *before != after {
            panic!(
                "\n\n\
                 The SBF build modified the root Cargo.lock, so the bytecode under test came\n\
                 from a dependency resolution that is not the committed one.\n\n\
                 Review `git diff Cargo.lock` and commit it if the change is intended.\n\n"
            );
        }
    }

    let built = staging.join(ARTIFACT_NAME);
    if let Err(e) = install(&built, &embedded) {
        panic!(
            "\n\n\
             `cargo build-sbf` reported success but {ARTIFACT_NAME} was not produced in the\n\
             staging directory ({e}).\n\n\
             The build may be writing elsewhere — check for a CARGO_TARGET_DIR or\n\
             .cargo/config.toml override.\n\n"
        );
    }
}

/// Copy `from` over `to` via a temporary in the destination directory, so a reader
/// never observes a half-written program. Fails if `from` is absent, which is how
/// "the build did not produce an artifact" is detected.
fn install(from: &Path, to: &Path) -> std::io::Result<()> {
    let tmp = to.with_extension("so.tmp");
    std::fs::copy(from, &tmp)?;
    std::fs::rename(&tmp, to)
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
