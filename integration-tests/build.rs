//! Builds the program bytecode the LiteSVM tests execute, so it cannot be stale.
//!
//! The suites load each program with `include_bytes!`, resolved at COMPILE time
//! from whatever artifact is on disk. `cargo test` does not rebuild them: this
//! workspace is excluded from the root one, so cargo has no dependency edge from
//! these tests to the programs' SBF artifacts.
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
//! - **The tests embed the artifacts under `$OUT_DIR`, not `target/deploy`.** The build
//!   writes into a fresh `--sbf-out-dir` under `OUT_DIR` and that file is what gets
//!   embedded, so the bytes provably come from this invocation. Checking that
//!   an artifact in `target/deploy` merely *exists* was not enough: `cargo
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

/// Every program the tests embed, as (manifest, artifact).
///
/// **Built one manifest at a time, never the workspace in one go.** The queue
/// depends on the vault with `features = ["cpi"]`, and `cpi` implies
/// `no-entrypoint`. Cargo unifies features per dependency across a single build,
/// so a workspace-wide `cargo build-sbf` compiles the vault ONCE under
/// `default ∪ cpi ∪ no-entrypoint` — cfg'ing `entrypoint!` out of the only
/// `august_vault.so` that exists. The result is a ~900-byte artifact that LiteSVM
/// rejects with `InvalidAccountData`. Confirm with:
///
/// ```text
/// cargo tree -e features -i august-vault --workspace
/// ```
///
/// A per-manifest build resolves features from that manifest alone, so the vault
/// built by itself never sees `cpi`. The invariant to preserve is therefore "never
/// let the vault and a `cpi` consumer be feature-unified in one cargo invocation" —
/// separate out-dirs alone would NOT save you.
const PROGRAMS: &[(&str, &str)] = &[
    ("programs/august-vault/Cargo.toml", "august_vault.so"),
    (
        "programs/august-withdrawal-queue/Cargo.toml",
        "august_withdrawal_queue.so",
    ),
];
const ESCAPE_HATCH: &str = "ALLOW_STALE_PROGRAM_SO";

/// Where a parked artifact lives when the escape hatch is used.
fn parked_path(repo_root: &Path, artifact: &str) -> PathBuf {
    repo_root.join("target/deploy").join(artifact)
}

fn main() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("integration-tests must sit one level below the repository root")
        .to_path_buf();
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    // Only used for the lockfile pre-check below, which is workspace-wide.
    let program_manifest = repo_root.join("Cargo.toml");

    println!("cargo:rerun-if-env-changed={ESCAPE_HATCH}");
    // Re-run when anything that feeds the SBF build changes. Cargo walks watched
    // directories recursively, so `programs` covers every program source, and the
    // manifests/lockfile cover profile, feature and dependency changes. `.cargo` is
    // watched as a DIRECTORY so a config added later is still noticed.
    //
    // Only paths that EXIST are registered. A `rerun-if-changed` on a missing path
    // is permanently "stale: missing", which re-runs this script on every single
    // invocation — and since the script rewrites the embedded artifact, every
    // integration-test target then relinks each time. This repo has
    // `.cargo/audit.toml` but no `.cargo/config.toml`, so naming that file directly
    // cost ~5s on every no-op `cargo test`.
    for path in ["programs", "Cargo.toml", "Cargo.lock", ".cargo"] {
        let watched = repo_root.join(path);
        if watched.exists() {
            println!("cargo:rerun-if-changed={}", watched.display());
        }
    }

    if std::env::var_os(ESCAPE_HATCH).is_some() {
        // Declare the parked artifact an input, or replacing it would keep
        // embedding the previous OUT_DIR copy and a rehearsal would silently test
        // the wrong bytes. Registered only on this path, where it is actually read:
        // doing it unconditionally would re-introduce the missing-path staleness
        // above for every checkout that has not built the program yet.
        for (_, artifact) in PROGRAMS {
            let parked = parked_path(&repo_root, artifact);
            if parked.exists() {
                println!("cargo:rerun-if-changed={}", parked.display());
            }
            println!(
                "cargo:warning={ESCAPE_HATCH} is set — NOT building. Embedding {} as-is, \
                 which may not match this source tree.",
                parked.display()
            );
            if let Err(e) = install(&parked, &out_dir.join(artifact)) {
                panic!(
                    "\n\n{ESCAPE_HATCH} is set, but {} could not be used ({e}).\n\
                     Put the artifact you want to test there, or unset {ESCAPE_HATCH}.\n\n",
                    parked.display()
                );
            }
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
    // Not `.ok()`: a `None` here would silently disable the post-build comparison
    // below, which is the only check that observes an actual re-resolve. The
    // lockfile is committed, so failing to read it is a real problem.
    let lock_before = std::fs::read(&lock_path)
        .unwrap_or_else(|e| panic!("read the committed {}: {e}", lock_path.display()));
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
    //
    // Cleared wholesale rather than per program: one call covers every program
    // plus anything an earlier layout left in here.
    let staging_root = out_dir.join("sbf-out");
    match std::fs::remove_dir_all(&staging_root) {
        Ok(()) => {}
        // Absent on the first run, which is the normal case.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        // Anything else leaves artifacts behind that the per-program existence
        // check below would happily accept as this invocation's output.
        Err(e) => panic!(
            "clear the SBF staging directory {}: {e}",
            staging_root.display()
        ),
    }

    for (manifest, artifact) in PROGRAMS {
        // One staging directory per program, so the artifact found after a build
        // is attributable to THAT build: nothing another program's invocation
        // wrote can satisfy this one's existence check below.
        let staging = staging_root.join(artifact.trim_end_matches(".so"));
        std::fs::create_dir_all(&staging).expect("create the SBF staging directory");

        let mut cmd = Command::new("cargo");
        cmd.arg("build-sbf")
            .arg("--manifest-path")
            .arg(repo_root.join(manifest))
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
             The SBF build failed, so the LiteSVM tests have no current bytecode to run\n\
             against. Fix the program, or set {ESCAPE_HATCH}=1 to embed the existing\n\
             target/deploy artifacts.\n\n\
             --- cargo build-sbf stderr ---\n{}\n",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Belt and braces: prove the build did not re-resolve. Cheap, and the only
        // check that actually observes the outcome rather than the intent.
        let after = std::fs::read(&lock_path)
            .unwrap_or_else(|e| panic!("re-read {} after the build: {e}", lock_path.display()));
        if lock_before != after {
            panic!(
                "\n\n\
             The SBF build modified the root Cargo.lock, so the bytecode under test came\n\
             from a dependency resolution that is not the committed one.\n\n\
             Review `git diff Cargo.lock` and commit it if the change is intended.\n\n"
            );
        }

        let built = staging.join(artifact);
        if let Err(e) = install(&built, &out_dir.join(artifact)) {
            panic!(
                "\n\n\
             `cargo build-sbf` reported success but {artifact} was not produced in the\n\
             staging directory ({e}).\n\n\
             The build may be writing elsewhere — check for a CARGO_TARGET_DIR or\n\
             .cargo/config.toml override.\n\n"
            );
        }
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
