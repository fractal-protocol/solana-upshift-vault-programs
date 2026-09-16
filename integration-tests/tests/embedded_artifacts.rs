//! Guards the two-program build wiring itself.
//!
//! Every other suite depends on the embedded artifacts being real programs, but
//! none of them *says so*: when the build produces a stunted artifact, the whole
//! suite fails on unrelated assertions and the cause has to be reverse-engineered.
//! These checks fail by name instead.
//!
//! The specific hazard is feature unification — see `build.rs`. Building the
//! workspace in one go compiles the vault under the queue's `cpi` feature, which
//! implies `no-entrypoint`, yielding a ~900-byte `august_vault.so` with no
//! entrypoint that LiteSVM rejects at first invocation rather than at load.

use litesvm::LiteSVM;

/// The stunted `no-entrypoint` artifact is ~900 bytes; a real program is
/// hundreds of KB. Anywhere in between means something is wrong.
const MIN_PROGRAM_SIZE: usize = 50 * 1024;

const ARTIFACTS: &[(&str, &[u8])] = &[
    (
        "august_vault",
        include_bytes!(concat!(env!("OUT_DIR"), "/august_vault.so")),
    ),
    (
        "august_withdrawal_queue",
        include_bytes!(concat!(env!("OUT_DIR"), "/august_withdrawal_queue.so")),
    ),
];

/// A `no-entrypoint` build drops the symbol the SBF loader dispatches through,
/// which is the difference the size check only approximates.
#[test]
fn embedded_artifacts_declare_an_entrypoint() {
    for (name, bytes) in ARTIFACTS {
        assert!(
            bytes.len() >= MIN_PROGRAM_SIZE,
            "{name}.so is {} bytes — a `no-entrypoint` build (see build.rs: never \
             build the workspace in one go)",
            bytes.len(),
        );
        assert!(
            bytes.windows(10).any(|w| w == b"entrypoint"),
            "{name}.so has no `entrypoint` symbol — built with `no-entrypoint`, \
             so the loader has nothing to dispatch to (see build.rs)",
        );
    }
}

#[test]
fn embedded_artifacts_load_into_litesvm() {
    let mut svm = LiteSVM::new();
    svm.add_program(august_vault::ID, ARTIFACTS[0].1)
        .expect("LiteSVM rejected august_vault.so");
    svm.add_program(august_withdrawal_queue::ID, ARTIFACTS[1].1)
        .expect("LiteSVM rejected august_withdrawal_queue.so");
}
