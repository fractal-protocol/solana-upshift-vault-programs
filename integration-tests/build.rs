//! Build-freshness guard for the bytecode the LiteSVM tests execute.
//!
//! `mainnet_fork_compat.rs` (and the other suites, via `new_svm`) load the program
//! with `include_bytes!("../../target/deploy/august_vault.so")`. That is resolved
//! at COMPILE time from whatever artifact happens to be on disk, and
//! `cargo test` does **not** rebuild it — this workspace is excluded from the
//! root one, so cargo has no dependency edge from these tests to the program's
//! SBF artifact.
//!
//! The consequence is a silent false pass: edit the program, run `cargo test`
//! without rebuilding, and every test still exercises the OLD bytecode and goes
//! green. That is worst in `mainnet_fork_compat.rs`, whose entire job is proving
//! the current program can still operate on live on-chain accounts — a
//! layout-compatibility break is exactly the class of bug a stale artifact hides.
//!
//! CI is currently safe only by ORDERING: `ci.yml`'s "Build & Test" job runs
//! `anchor build` immediately before the integration tests. That is load-bearing
//! and was undocumented — and the same job later rewrites `declare_id!` to a
//! fresh localnet keypair and rebuilds, so reordering those steps would silently
//! point the fork guard at a localnet-ID binary. This guard makes the requirement
//! explicit and machine-enforced instead of implied by step order.
//!
//! ## What it checks
//!
//! The artifact must be at least as new as every input that can change it: the
//! program sources, its manifest, the lockfile, and `Anchor.toml`. If anything is
//! newer, the build FAILS with the offending file named, rather than compiling a
//! test binary around stale bytes.
//!
//! Timestamps, not content hashes: there is no record of which source produced
//! the artifact, so a hash comparison has nothing to compare against without also
//! changing how the program is built. mtime is the honest signal available here.
//!
//! It is deliberately strict about one case that looks like a false positive but
//! is not: `git checkout` / `clone` / `stash pop` set source mtimes to now, so the
//! guard fires after a branch switch. Rebuilding at that point is correct — the
//! artifact belongs to the other branch. The cost of over-firing is one rebuild;
//! the cost of under-firing is a green test run that proved nothing.
//!
//! Escape hatch for the one legitimate case — deliberately testing a
//! hand-supplied artifact, e.g. a devnet-ID build during an upgrade rehearsal:
//!
//! ```text
//! ALLOW_STALE_PROGRAM_SO=1 cargo test --manifest-path integration-tests/Cargo.toml
//! ```
//!
//! CI must never set it.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Inputs that can change the compiled artifact. Paths are relative to the
/// repository root (the parent of this crate).
const SOURCE_DIRS: &[&str] = &["programs"];
const SOURCE_FILES: &[&str] = &["Cargo.lock", "Cargo.toml", "Anchor.toml"];

const ARTIFACT: &str = "target/deploy/august_vault.so";
const ESCAPE_HATCH: &str = "ALLOW_STALE_PROGRAM_SO";

fn main() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("integration-tests must sit one level below the repository root")
        .to_path_buf();

    println!("cargo:rerun-if-env-changed={ESCAPE_HATCH}");
    let artifact = repo_root.join(ARTIFACT);
    println!("cargo:rerun-if-changed={}", artifact.display());
    for dir in SOURCE_DIRS {
        println!("cargo:rerun-if-changed={}", repo_root.join(dir).display());
    }
    for file in SOURCE_FILES {
        println!("cargo:rerun-if-changed={}", repo_root.join(file).display());
    }

    if std::env::var_os(ESCAPE_HATCH).is_some() {
        println!(
            "cargo:warning={ESCAPE_HATCH} is set — NOT checking that {ARTIFACT} is up to date. \
             The tests may be exercising bytecode that does not match this source tree."
        );
        return;
    }

    // A missing artifact is already a clear failure at `include_bytes!`, but say
    // so here with the command to fix it rather than as a file-not-found.
    let artifact_time = match modified(&artifact) {
        Some(t) => t,
        None => panic!(
            "\n\n{ARTIFACT} does not exist, so the LiteSVM tests have no program to load.\n\
             Build it first:\n    anchor build\n\n"
        ),
    };

    let mut newest: Option<(PathBuf, SystemTime)> = None;
    for dir in SOURCE_DIRS {
        visit(&repo_root.join(dir), &mut newest);
    }
    for file in SOURCE_FILES {
        consider(&repo_root.join(file), &mut newest);
    }

    if let Some((path, source_time)) = newest {
        if source_time > artifact_time {
            let rel = path.strip_prefix(&repo_root).unwrap_or(&path);
            panic!(
                "\n\n\
                 {ARTIFACT} is STALE — it is older than the program source.\n\n\
                 \x20 newer input : {}\n\
                 \x20 artifact    : {ARTIFACT}\n\n\
                 The LiteSVM tests `include_bytes!` that artifact at compile time and cargo does\n\
                 not rebuild it, so running them now would exercise bytecode that does not match\n\
                 this source tree — and pass. Rebuild first:\n\n\
                 \x20   anchor build\n\n\
                 If you are deliberately testing a hand-supplied artifact (e.g. a devnet-ID build\n\
                 during an upgrade rehearsal), set {ESCAPE_HATCH}=1.\n\n",
                rel.display()
            );
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn consider(path: &Path, newest: &mut Option<(PathBuf, SystemTime)>) {
    if let Some(t) = modified(path) {
        if newest.as_ref().is_none_or(|(_, best)| t > *best) {
            *newest = Some((path.to_path_buf(), t));
        }
    }
}

/// Recurse through the program sources, skipping build output. `target` can hold
/// artifacts far newer than the `.so` and would make the guard fire constantly.
fn visit(dir: &Path, newest: &mut Option<(PathBuf, SystemTime)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "target" || name.to_string_lossy().starts_with('.') {
            continue;
        }
        match entry.file_type() {
            Ok(t) if t.is_dir() => visit(&path, newest),
            Ok(t) if t.is_file() => consider(&path, newest),
            _ => {}
        }
    }
}
