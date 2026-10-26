//! Guards the two-program build wiring itself.
//!
//! Every other suite depends on the embedded artifacts being real programs, but
//! none of them *says so*: when the build produces a stunted artifact, the whole
//! suite fails on unrelated assertions and the cause has to be reverse-engineered.
//! These checks fail by name instead.
//!
//! The specific hazard is feature unification — see `build.rs`. Building the
//! workspace in one go compiles the vault under the queue's `cpi` feature, which
//! implies `no-entrypoint`, yielding a 544-byte `august_vault.so` stub that
//! LiteSVM refuses to load.

use anchor_lang::error::ErrorCode as AnchorError;
use integration_tests::harness::assert_anchor_framework_err;
use integration_tests::svm::Svm;
use solana_sdk::{
    instruction::Instruction,
    signature::{Keypair, Signer},
    transaction::Transaction,
};

/// The stunted `no-entrypoint` artifact is under 1 KB; a real program is
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

/// ELF64 `e_flags`, which carries the SBPF version.
const E_FLAGS_OFFSET: usize = 0x30;
/// Must match `SBF_ARCH` in build.rs and the `--arch` of the verifiable build.
const SBPF_V3: u32 = 3;

#[test]
fn embedded_artifacts_are_sbpf_v3() {
    for (name, bytes) in ARTIFACTS {
        assert!(
            bytes.len() >= MIN_PROGRAM_SIZE,
            "{name}.so is {} bytes — a `no-entrypoint` build (see build.rs: never \
             build the workspace in one go)",
            bytes.len(),
        );
        assert_eq!(&bytes[..4], b"\x7fELF", "{name}.so is not an ELF");
        let flags = u32::from_le_bytes(
            bytes[E_FLAGS_OFFSET..E_FLAGS_OFFSET + 4]
                .try_into()
                .unwrap(),
        );
        assert_eq!(
            flags, SBPF_V3,
            "{name}.so is SBPF v{flags}, not v3 (see build.rs)"
        );
    }
}

/// A v3 ELF names no symbols, so the entrypoint cannot be found by grepping.
/// Prove it behaviourally instead: an unknown discriminator only reaches
/// Anchor's `InstructionFallbackNotFound` through a working entrypoint.
#[test]
fn embedded_artifacts_dispatch_through_their_entrypoint() {
    let mut svm = Svm::new();
    svm.add_program(august_vault::ID, ARTIFACTS[0].1).unwrap();
    svm.add_program(august_withdrawal_queue::ID, ARTIFACTS[1].1)
        .unwrap();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();

    for program_id in [august_vault::ID, august_withdrawal_queue::ID] {
        let ix = Instruction {
            program_id,
            accounts: vec![],
            data: vec![0xff; 8],
        };
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[&payer],
            svm.latest_blockhash(),
        );
        let err = svm
            .send_transaction(tx)
            .expect_err("unknown instruction must fail");
        assert_anchor_framework_err(&err, AnchorError::InstructionFallbackNotFound as u32);
    }
}

#[test]
fn embedded_artifacts_load_into_litesvm() {
    let mut svm = Svm::new();
    svm.add_program(august_vault::ID, ARTIFACTS[0].1)
        .expect("LiteSVM rejected august_vault.so");
    svm.add_program(august_withdrawal_queue::ID, ARTIFACTS[1].1)
        .expect("LiteSVM rejected august_withdrawal_queue.so");
}
