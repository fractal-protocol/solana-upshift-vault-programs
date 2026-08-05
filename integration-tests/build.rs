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
//! ## Content, not timestamps
//!
//! The guard compares a recorded manifest of input **content hashes** against the
//! current ones. An earlier timestamp-based version was wrong in both directions
//! and is worth recording so it is not reintroduced:
//!
//! - **It could wedge the build.** Cargo fingerprints a manifest's parsed
//!   *contents*, not its mtime, so `touch Cargo.toml` makes the file newer than
//!   the artifact while `anchor build` has nothing to relink. The guard then failed
//!   with a recovery command that could not clear it. The same applied to a
//!   `git checkout` that rewrote a file byte-identically. A guard that can wedge a
//!   build is a guard someone deletes.
//! - **It could not see a deletion.** Removing an input leaves every surviving
//!   file older than the artifact, so a timestamp comparison saw nothing.
//!
//! Content hashing fixes both: a real edit, addition or deletion changes the
//! manifest, and a content-preserving touch or checkout does not. It also aligns
//! the guard with what actually matters — identical bytes in produce identical
//! bytecode out, whatever the mtimes say.
//!
//! The hash is FNV-1a, not a cryptographic digest. This guards against staleness,
//! not against an adversary crafting a collision in your own working tree.
//!
//! ## Which inputs count
//!
//! The program's Rust sources and manifests, the root manifest (its `[profile]`
//! affects codegen), and the lockfile.
//!
//! Two files are excluded on purpose:
//!
//! - **`Anchor.toml`** — not a bytecode input. Its `[toolchain]` sets only
//!   `package_manager`, `[features]` governs IDL/lint rather than codegen, and
//!   `[programs.*]` are addresses (`declare_id!` lives in source). Its
//!   `[scripts]`/`[provider]`/`[test]` sections change routinely — PR #16 rewrote
//!   `[scripts]` from yarn to pnpm.
//! - **`rust-toolchain.toml`** — its own header states it does not govern the
//!   verifiable on-chain bytecode: the SBF build uses platform-tools' bundled
//!   Rust, fixed by the pinned `solana-verify` image. It pins host tooling only.
//!
//! If a version pin is ever added to `Anchor.toml [toolchain]`, add it here — with
//! content hashing there is no longer a wedge risk in doing so.
//!
//! ## The one gap that remains
//!
//! Comparison needs a baseline, so the first run after adoption (or after
//! `cargo clean`) has nothing to compare against and can only record what it sees.
//! A deletion that happened *before* that first run is therefore invisible. The
//! guard says so via a `cargo:warning` rather than implying coverage it does not
//! have; one `anchor build` closes it permanently.
//!
//! Removing that gap entirely would mean invoking the SBF build from here — making
//! the missing dependency edge real instead of approximating it. That is the
//! sound fix, and deliberately not done in this change: it would make `cargo test`
//! require the SBF toolchain and silently overwrite a hand-placed artifact.
//!
//! Escape hatch for the one legitimate case — deliberately testing a
//! hand-supplied artifact, e.g. a devnet-ID build during an upgrade rehearsal:
//!
//! ```text
//! ALLOW_STALE_PROGRAM_SO=1 cargo test --manifest-path integration-tests/Cargo.toml
//! ```
//!
//! CI must never set it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Directories scanned for program sources, relative to the repo root.
const SOURCE_DIRS: &[&str] = &["programs"];
/// Extensions that can change the bytecode. Anything else under `programs/`
/// (docs, notes) cannot.
const SOURCE_EXTS: &[&str] = &["rs", "toml"];
/// Individual input files, relative to the repo root.
const SOURCE_FILES: &[&str] = &["Cargo.lock", "Cargo.toml"];

const ARTIFACT: &str = "target/deploy/august_vault.so";
/// The recorded baseline: input content hashes as of the last accepted artifact.
/// Lives beside the artifact, so `cargo clean` drops both together.
const RECEIPT: &str = "target/deploy/.august_vault.so.inputs";
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
    // NOT rerun-if-changed on RECEIPT — this script writes it, and watching it
    // would make every build dirty the next one.

    if std::env::var_os(ESCAPE_HATCH).is_some() {
        println!(
            "cargo:warning={ESCAPE_HATCH} is set — NOT checking that {ARTIFACT} is up to \
             date. The tests may be exercising bytecode that does not match this source tree."
        );
        return;
    }

    let artifact_bytes = match std::fs::read(&artifact) {
        Ok(b) => b,
        Err(_) => panic!(
            "\n\n{ARTIFACT} does not exist, so the LiteSVM tests have no program to load.\n\
             Build it first:\n    anchor build\n\n"
        ),
    };

    let mut inputs: BTreeMap<String, u64> = BTreeMap::new();
    for dir in SOURCE_DIRS {
        visit(&repo_root.join(dir), &repo_root, &mut inputs);
    }
    for file in SOURCE_FILES {
        consider(&repo_root.join(file), &repo_root, &mut inputs);
    }

    let manifest = render(fnv1a(&artifact_bytes), &inputs);
    let receipt_path = repo_root.join(RECEIPT);

    match std::fs::read_to_string(&receipt_path) {
        Ok(previous) => {
            let (prev_artifact, prev_inputs) = split(&previous);
            let (this_artifact, this_inputs) = split(&manifest);
            // A different artifact means it was rebuilt, so whatever the inputs are
            // now is the new baseline. Only compare while the bytes are unchanged.
            if prev_artifact == this_artifact && prev_inputs != this_inputs {
                fail_stale(&describe(prev_inputs, this_inputs));
            }
        }
        Err(_) => {
            // No baseline. Say so rather than implying coverage: a deletion made
            // before this run leaves nothing to detect it by.
            println!(
                "cargo:warning=establishing the {ARTIFACT} freshness baseline \
                 ({RECEIPT} was absent). An input DELETED before this run cannot be \
                 detected; run `anchor build` once to close that gap."
            );
        }
    }

    // Best-effort: a receipt that cannot be written costs future detection, not the
    // correctness of this run, so do not fail the build over it.
    let _ = std::fs::write(&receipt_path, manifest);
}

fn fail_stale(detail: &str) -> ! {
    panic!(
        "\n\n\
         {ARTIFACT} is STALE — its inputs have changed since it was built.\n\n\
         \x20 {detail}\n\n\
         The LiteSVM tests `include_bytes!` that artifact at compile time and cargo does\n\
         not rebuild it, so running them now would exercise bytecode that does not match\n\
         this source tree — and pass. Rebuild first:\n\n\
         \x20   anchor build\n\n\
         Only real content changes are reported, so a rebuild always clears this. If you\n\
         are deliberately testing a hand-supplied artifact (e.g. a devnet-ID build during\n\
         an upgrade rehearsal), set {ESCAPE_HATCH}=1.\n\n"
    )
}

/// FNV-1a. Fast, dependency-free, and sufficient to detect edits — see the module
/// docs on why this is not a cryptographic guarantee.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn render(artifact_hash: u64, inputs: &BTreeMap<String, u64>) -> String {
    let mut out = format!("artifact {artifact_hash:016x}\n");
    for (path, hash) in inputs {
        out.push_str(&format!("{path} {hash:016x}\n"));
    }
    out
}

/// Split a rendered manifest into (artifact line, input lines).
fn split(s: &str) -> (&str, Vec<&str>) {
    let mut lines = s.lines();
    let artifact = lines.next().unwrap_or("");
    (artifact, lines.collect())
}

/// Name what changed, so the failure is diagnosable rather than "something".
fn describe(prev: Vec<&str>, now: Vec<&str>) -> String {
    let paths = |v: &Vec<&str>| -> Vec<String> {
        v.iter()
            .filter_map(|l| l.rsplit_once(' ').map(|(p, _)| p.to_string()))
            .collect()
    };
    let (p, n) = (paths(&prev), paths(&now));
    let removed: Vec<String> = p.iter().filter(|x| !n.contains(x)).cloned().collect();
    let added: Vec<String> = n.iter().filter(|x| !p.contains(x)).cloned().collect();
    let changed: Vec<String> = now
        .iter()
        .filter(|l| !prev.contains(l))
        .filter_map(|l| l.rsplit_once(' ').map(|(path, _)| path.to_string()))
        .filter(|path| !added.contains(path))
        .collect();

    let mut parts = Vec::new();
    if !changed.is_empty() {
        parts.push(format!("changed: {}", changed.join(", ")));
    }
    if !removed.is_empty() {
        parts.push(format!("removed: {}", removed.join(", ")));
    }
    if !added.is_empty() {
        parts.push(format!("added: {}", added.join(", ")));
    }
    if parts.is_empty() {
        return "an input changed".to_string();
    }
    parts.join("\n  ")
}

fn consider(path: &Path, repo_root: &Path, inputs: &mut BTreeMap<String, u64>) {
    if let Ok(bytes) = std::fs::read(path) {
        let rel = path.strip_prefix(repo_root).unwrap_or(path);
        inputs.insert(rel.display().to_string(), fnv1a(&bytes));
    }
}

/// Recurse the program sources, skipping build output and dotfiles.
fn visit(dir: &Path, repo_root: &Path, inputs: &mut BTreeMap<String, u64>) {
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
            Ok(t) if t.is_dir() => visit(&path, repo_root, inputs),
            Ok(t) if t.is_file() => {
                let is_input = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| SOURCE_EXTS.contains(&e));
                if is_input {
                    consider(&path, repo_root, inputs);
                }
            }
            _ => {}
        }
    }
}
