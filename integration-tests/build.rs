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
//! ## Ask the build, don't second-guess it
//!
//! The guard reads the **dep-info the SBF build itself writes**
//! (`target/<sbf-triple>/release/august_vault.d`): a Makefile-style list of
//! exactly the files rustc read to produce the bytecode, plus its own mtime as the
//! moment that build ran. Staleness is then a two-line question — is any of those
//! files newer than that build, or has one been deleted?
//!
//! Two earlier designs were wrong, and the reasons are worth keeping so neither is
//! reintroduced:
//!
//! 1. **Comparing hand-picked source paths by mtime.** Cargo fingerprints a
//!    manifest's parsed *contents*, not its mtime, so `touch Cargo.toml` made the
//!    file newer than the artifact while `anchor build` had nothing to relink —
//!    a failure the prescribed recovery command could not clear.
//! 2. **Comparing hand-picked source paths by content hash, treating changed
//!    artifact bytes as proof of a rebuild.** A rebuild need not change the output:
//!    add a comment to a `#[cfg(test)]` file, run `anchor build`, and the `.so`
//!    is byte-identical — so the guard rejected every run forever.
//!
//! Both failed the same way: they guessed at the input set. Cargo already knows it.
//! `#[cfg(test)]` files are absent from the dep-info because rustc never reads them
//! for a release build, so editing them cannot fire the guard — correctly, since
//! they cannot change the bytecode. And because any real change makes cargo
//! recompile and rewrite both the `.d` and the `.so`, **a rebuild always clears a
//! failure**. That is the property both earlier versions lacked.
//!
//! ## Known limitation
//!
//! Dep-info covers source files, not manifests. Editing `Cargo.lock` or a
//! `Cargo.toml` without rebuilding is therefore not detected. Including them was
//! tried and is what caused failure mode 1 above: cargo will not relink on a
//! content-preserving manifest touch, so the guard became unclearable. A real
//! manifest edit does force a recompile, so the window is "edited a manifest and
//! ran the tests without building" — narrower than the wedge it replaces.
//!
//! Closing it properly means invoking the SBF build from here, making the missing
//! dependency edge real rather than approximating it. Deliberately not done: it
//! would make `cargo test` require the SBF toolchain and would silently overwrite
//! a hand-placed artifact.
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
    // Watch the program sources so an edit re-runs this check. Watching the
    // directory (rather than the files the dep-info names) also catches a file
    // being added or removed.
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("programs").display()
    );

    if std::env::var_os(ESCAPE_HATCH).is_some() {
        println!(
            "cargo:warning={ESCAPE_HATCH} is set — NOT checking that {ARTIFACT} is up to \
             date. The tests may be exercising bytecode that does not match this source tree."
        );
        return;
    }

    if !artifact.exists() {
        panic!(
            "\n\n{ARTIFACT} does not exist, so the LiteSVM tests have no program to load.\n\
             Build it first:\n    anchor build\n\n"
        );
    }

    // The SBF build's own record of what it read. Its directory is the target
    // triple, which has been renamed across toolchains (sbf-… -> sbpf-…), so find
    // it rather than hardcoding.
    let dep_info = match find_dep_info(&repo_root) {
        Some(p) => p,
        None => panic!(
            "\n\n\
             No SBF dep-info found under target/, so there is no record of which sources\n\
             produced {ARTIFACT} and its freshness cannot be checked.\n\n\
             Build the program with the SBF toolchain:\n\n\
             \x20   anchor build\n\n\
             (If you are deliberately testing a hand-supplied artifact, set {ESCAPE_HATCH}=1.)\n\n"
        ),
    };

    let built_at = modified(&dep_info).expect("dep-info mtime is readable");

    let mut newer: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for input in parse_dep_info(&dep_info) {
        let rel = input
            .strip_prefix(&repo_root)
            .unwrap_or(&input)
            .display()
            .to_string();
        match modified(&input) {
            None => missing.push(rel),
            Some(t) if t > built_at => newer.push(rel),
            Some(_) => {}
        }
    }

    if newer.is_empty() && missing.is_empty() {
        return;
    }

    let mut detail = Vec::new();
    if !newer.is_empty() {
        detail.push(format!("modified since that build: {}", newer.join(", ")));
    }
    if !missing.is_empty() {
        detail.push(format!("deleted since that build: {}", missing.join(", ")));
    }
    panic!(
        "\n\n\
         {ARTIFACT} is STALE — the program was compiled before these changes.\n\n\
         \x20 {}\n\n\
         The LiteSVM tests `include_bytes!` that artifact at compile time and cargo does\n\
         not rebuild it, so running them now would exercise bytecode that does not match\n\
         this source tree — and pass. Rebuild first:\n\n\
         \x20   anchor build\n\n\
         The file list comes from the SBF build's own dep-info, so only files that really\n\
         affect the bytecode are reported, and a rebuild always clears this.\n\n\
         If you are deliberately testing a hand-supplied artifact (e.g. a devnet-ID build\n\
         during an upgrade rehearsal), set {ESCAPE_HATCH}=1.\n\n",
        detail.join("\n  ")
    )
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Locate `august_vault.d` under an SBF target directory, preferring the most
/// recently written one if a toolchain rename left more than one behind.
fn find_dep_info(repo_root: &Path) -> Option<PathBuf> {
    let target = repo_root.join("target");
    let mut best: Option<(PathBuf, SystemTime)> = None;
    for entry in std::fs::read_dir(&target).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // sbf-solana-solana / sbpf-solana-solana / bpfel-unknown-unknown
        if !(name.contains("sbf") || name.contains("sbpf") || name.contains("bpf")) {
            continue;
        }
        let candidate = entry.path().join("release").join("august_vault.d");
        if let Some(t) = modified(&candidate) {
            if best.as_ref().is_none_or(|(_, bt)| t > *bt) {
                best = Some((candidate, t));
            }
        }
    }
    best.map(|(p, _)| p)
}

/// Parse rustc's Makefile-style dep-info: the first line is
/// `<output>: <dep> <dep> …`, followed by one empty rule per dep.
///
/// Paths containing spaces are backslash-escaped by rustc; unescape them so a
/// checkout under such a path does not read as a set of missing files.
fn parse_dep_info(path: &Path) -> Vec<PathBuf> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let first = text.lines().next().unwrap_or_default();
    let rhs = match first.split_once(": ") {
        Some((_, r)) => r,
        None => return Vec::new(),
    };

    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = rhs.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&' ') => {
                current.push(' ');
                chars.next();
            }
            ' ' => {
                if !current.is_empty() {
                    out.push(PathBuf::from(std::mem::take(&mut current)));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(PathBuf::from(current));
    }
    out
}
