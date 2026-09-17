// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `attach_withdrawal_queue` and `detach_withdrawal_queue`, the two writers of
//! the field the redeem gate reads.
//!
//! Two rules have to hold. No key that fails derivation can ever be stored,
//! because a stored key nobody can sign for is a permanent exit freeze. And once
//! a key is stored, nothing but that key's own signature can clear it. Attach
//! carries the first rule and detach the second, and this suite is organised
//! around them and the transitions between them.
//!
//! The queue program has no instructions yet, so two conditions are staged rather
//! than produced. "Initialized by the queue" is staged by planting an account at
//! the PDA with `install_queue_account`, and "the queue signs" is exercised with
//! an ordinary keypair written straight into the field. Both are faithful to the
//! rules under test, because the vault checks a signature against a stored key
//! and never asks whether that key is a PDA. The real CPI path, in which the queue
//! PDA signs through `invoke_signed`, belongs to `release_vault`'s tests.

use anchor_lang::{AnchorDeserialize, Discriminator};
use august_vault::errors::ErrorCode;
use august_vault::instructions::attach_withdrawal_queue::WithdrawalQueueAttached;
use august_vault::instructions::detach_withdrawal_queue::WithdrawalQueueDetached;
use august_vault::state::vault::{
    withdrawal_queue_pda, VAULT_STATE_SEED, WITHDRAWAL_QUEUE_PROGRAM_ID, WITHDRAWAL_QUEUE_SEED,
};
use base64::Engine;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, QueueCoSigner, VaultCtx, DEPOSIT_DECIMALS,
    VAULT_VERSION,
};
use litesvm::types::TransactionMetadata;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);

/// Returns a funded vault whose user holds redeemable shares.
fn vault_with_a_holder() -> VaultCtx {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("initial deposit");
    ctx
}

/// Returns the vault's own queue PDA, with an account planted there in the state
/// `initialize_queue` will leave it in.
fn ready_queue(ctx: &mut VaultCtx) -> Pubkey {
    let pda = ctx.withdrawal_queue_pda();
    ctx.install_queue_account(pda, august_withdrawal_queue::ID);
    pda
}

/// Returns a second vault's queue PDA, initialized and owned by the queue program.
fn another_vaults_ready_queue(ctx: &mut VaultCtx) -> Pubkey {
    let mint2 = ctx.create_extra_deposit_mint();
    let authority = ctx.protocol_authority.insecure_clone();
    ctx.try_initialize_vault(&authority, mint2, VAULT_VERSION)
        .expect("second vault");
    let vault2 = Pubkey::find_program_address(
        &[VAULT_STATE_SEED, mint2.as_ref(), &[VAULT_VERSION]],
        &august_vault::ID,
    )
    .0;
    let pda = withdrawal_queue_pda(&vault2);
    assert_ne!(pda, ctx.withdrawal_queue_pda());
    ctx.install_queue_account(pda, august_withdrawal_queue::ID);
    pda
}

/// Gates the vault on an ordinary keypair, bypassing the instruction, so that a
/// test can produce the attached queue's signature.
///
/// The resulting state is synthetic: attach refuses to store a keypair, so no
/// vault can reach it through the public API. The real exit path stays unproven
/// until `release_vault` exists. See the module documentation above.
fn gate_on_keypair(ctx: &mut VaultCtx) -> Keypair {
    let queue = Keypair::new();
    let mut state = ctx.vault_state_data();
    state.withdrawal_queue_authority = queue.pubkey();
    ctx.force_overwrite_vault_state(state);
    queue
}

/// Returns every event of type `E` the transaction emitted, in order.
fn emitted<E: AnchorDeserialize + Discriminator>(meta: &TransactionMetadata) -> Vec<E> {
    let disc = E::DISCRIMINATOR;
    meta.logs
        .iter()
        .filter_map(|line| line.strip_prefix("Program data: "))
        .map(|b64| {
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .expect("Program data line is base64")
        })
        .filter(|bytes| bytes.starts_with(disc))
        .map(|bytes| E::try_from_slice(&bytes[disc.len()..]).expect("event body decodes"))
        .collect()
}

fn assert_single_attached(meta: &TransactionMetadata, vault: Pubkey, queue: Pubkey) {
    let events = emitted::<WithdrawalQueueAttached>(meta);
    assert_eq!(events.len(), 1, "exactly one WithdrawalQueueAttached");
    assert_eq!(events[0].vault, vault);
    assert_eq!(events[0].queue, queue);
    assert!(
        emitted::<WithdrawalQueueDetached>(meta).is_empty(),
        "attach must not emit WithdrawalQueueDetached"
    );
}

fn assert_single_detached(meta: &TransactionMetadata, vault: Pubkey, queue: Pubkey) {
    let events = emitted::<WithdrawalQueueDetached>(meta);
    assert_eq!(events.len(), 1, "exactly one WithdrawalQueueDetached");
    assert_eq!(events[0].vault, vault);
    assert_eq!(events[0].queue, queue);
    assert!(
        emitted::<WithdrawalQueueAttached>(meta).is_empty(),
        "detach must not emit WithdrawalQueueAttached"
    );
}

// ---- the pin between the two crates ----

/// The vault cannot import the queue's id (the queue depends on the vault, so
/// the reverse edge would be a cycle) and hardcodes it. This crate depends on
/// both, so it is where the two are held equal.
#[test]
fn the_hardcoded_queue_program_id_is_the_queue_crates_id() {
    assert_eq!(
        WITHDRAWAL_QUEUE_PROGRAM_ID,
        august_withdrawal_queue::ID,
        "WITHDRAWAL_QUEUE_PROGRAM_ID in the vault has drifted from the queue's declare_id!"
    );
}

// ---- attach ----

#[test]
fn the_admin_attaches_the_vaults_own_queue_pda() {
    let mut ctx = VaultCtx::fresh();
    let pda = ready_queue(&mut ctx);

    let meta = ctx
        .attach_withdrawal_queue(pda)
        .expect("the vault's own initialized queue PDA must be accepted");

    assert_eq!(ctx.vault_state_data().withdrawal_queue(), Some(pda));
    assert_single_attached(&meta, ctx.vault_state, pda);
}

/// Attaching closes direct redemption, which is the whole point of the field.
#[test]
fn attaching_gates_direct_redemption() {
    let mut ctx = vault_with_a_holder();
    let pda = ready_queue(&mut ctx);
    ctx.attach_withdrawal_queue(pda).expect("attach");

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let err = ctx
        .redeem(shares)
        .expect_err("a holder must not redeem directly once gated");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueRequired);
}

/// Admin settings are not pause-gated; this one is no exception.
#[test]
fn attaching_works_while_paused() {
    let mut ctx = VaultCtx::fresh();
    let pda = ready_queue(&mut ctx);
    ctx.pause().expect("pause");

    ctx.attach_withdrawal_queue(pda)
        .expect("pause guards asset movement, not configuration");
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), Some(pda));
}

#[test]
fn a_non_admin_cannot_attach_even_the_right_pda() {
    let mut ctx = VaultCtx::fresh();
    let pda = ready_queue(&mut ctx);
    let impostor = ctx.new_funded_keypair(1_000_000_000);

    let err = ctx
        .attach_withdrawal_queue_as(&impostor, pda)
        .expect_err("only the admin attaches a queue");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

// ---- attach: every wrong account is refused, and nothing is stored ----

#[test]
fn an_arbitrary_wallet_is_refused() {
    let mut ctx = VaultCtx::fresh();
    // This is a real, funded, system-owned account. It exists, and it is not a
    // PDA of any program.
    let wallet = ctx.new_funded_keypair(1_000_000_000).pubkey();

    let err = ctx
        .attach_withdrawal_queue(wallet)
        .expect_err("a wallet nobody but its owner can sign for must never be stored");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    // Consumers match on the number, so pin the number (see the gate suite).
    assert_anchor_framework_err(&err, 6022);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// This is a genuine, initialized queue PDA, but it belongs to a different vault.
/// Derivation binds the key to *this* vault's state address.
#[test]
fn another_vaults_queue_pda_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let other_pda = another_vaults_ready_queue(&mut ctx);

    let err = ctx
        .attach_withdrawal_queue(other_pda)
        .expect_err("another vault's queue must not be attachable here");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// The seeds and the owner are correct, but the derivation uses the wrong
/// program. This isolates the program-id check from the owner check.
#[test]
fn the_right_seeds_under_the_wrong_program_are_refused() {
    let mut ctx = VaultCtx::fresh();
    let wrong = Pubkey::find_program_address(
        &[WITHDRAWAL_QUEUE_SEED, ctx.vault_state.as_ref()],
        &august_vault::ID,
    )
    .0;
    ctx.install_queue_account(wrong, august_withdrawal_queue::ID);

    let err = ctx
        .attach_withdrawal_queue(wrong)
        .expect_err("a PDA of any other program must be refused");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// This is the right address, but taken before `initialize_queue` has run there.
/// Storing it would gate the vault on a key the queue program has never acted for.
#[test]
fn an_uninitialized_pda_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let pda = ctx.withdrawal_queue_pda();
    assert!(
        ctx.svm.get_account(&pda).is_none(),
        "fixture: nothing planted"
    );

    let err = ctx
        .attach_withdrawal_queue(pda)
        .expect_err("the correct PDA is still refused until the queue has initialized it");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// This is the right address and it holds data, but the queue program does not
/// own it.
#[test]
fn a_pda_the_queue_program_does_not_own_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let pda = ctx.withdrawal_queue_pda();
    ctx.install_queue_account(pda, anchor_lang::system_program::ID);

    let err = ctx
        .attach_withdrawal_queue(pda)
        .expect_err("ownership by the queue program is what makes the account its own");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// The seeds and program are the same, but a lower bump also yields a valid
/// address. The queue program must sign only with the canonical bump, so this is
/// an address it must never use.
#[test]
fn a_non_canonical_bump_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let seeds: &[&[u8]] = &[WITHDRAWAL_QUEUE_SEED, ctx.vault_state.as_ref()];
    let (canonical, bump) = Pubkey::find_program_address(seeds, &WITHDRAWAL_QUEUE_PROGRAM_ID);
    let non_canonical = (0..bump)
        .rev()
        .find_map(|b| {
            Pubkey::create_program_address(
                &[WITHDRAWAL_QUEUE_SEED, ctx.vault_state.as_ref(), &[b]],
                &WITHDRAWAL_QUEUE_PROGRAM_ID,
            )
            .ok()
        })
        .expect("some lower bump yields a valid address (probability ~1 - 2^-bump)");
    assert_ne!(non_canonical, canonical);
    ctx.install_queue_account(non_canonical, august_withdrawal_queue::ID);

    let err = ctx
        .attach_withdrawal_queue(non_canonical)
        .expect_err("only the canonical bump is accepted");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// This is the only state `!data_is_empty()` can catch: an account that is
/// queue-owned but carries nothing. No standard Anchor path produces it today,
/// because `close` reassigns the account as well as reallocating it, so the check
/// earns its keep only once `initialize_queue` allocates and initializes in two
/// separate steps.
#[test]
fn a_queue_owned_but_empty_account_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let pda = ctx.withdrawal_queue_pda();
    ctx.install_queue_account_of_len(pda, august_withdrawal_queue::ID, 0);

    let err = ctx
        .attach_withdrawal_queue(pda)
        .expect_err("an allocated but empty queue account is not an initialized one");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

// ---- attach: never over an attached queue ----

/// Attach is not a path around detach. If it could overwrite a stored key, the
/// admin could move a vault off the queue its holders are queued in without that
/// queue's signature, which is the run the detach rule exists to prevent. The
/// stored key here is one attach would itself refuse, and the replacement is the
/// vault's own valid PDA, so nothing but this check stands in the way.
#[test]
fn attaching_over_an_attached_queue_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);
    let pda = ready_queue(&mut ctx);

    let err = ctx
        .attach_withdrawal_queue(pda)
        .expect_err("a valid PDA does not excuse the queue already attached");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueAlreadyAttached);
    assert_anchor_framework_err(&err, 6024);
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(queue.pubkey()),
        "a refused attach must leave the original queue attached"
    );
}

/// The same key twice is still refused. There is no idempotent re-set: a second
/// attach is a caller who has lost track of the vault's state, and a green
/// transaction would confirm the wrong belief.
#[test]
fn attaching_twice_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let pda = ready_queue(&mut ctx);
    ctx.attach_withdrawal_queue(pda).expect("first attach");

    let err = ctx
        .attach_withdrawal_queue(pda)
        .expect_err("already attached");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueAlreadyAttached);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), Some(pda));
}

/// On a gated vault the state is judged before the account, so a call that is
/// wrong in both ways reports "already attached" rather than "bad account", and
/// the caller goes looking for the right problem.
#[test]
fn a_gated_vault_reports_already_attached_before_judging_the_account() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);

    let err = ctx
        .attach_withdrawal_queue(Pubkey::new_unique())
        .expect_err("wrong on both counts");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueAlreadyAttached);
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(queue.pubkey())
    );
}

// ---- detach: only with the attached queue's signature ----

#[test]
fn detaching_without_the_queues_signature_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);

    // The queue is present but does not sign. The slot is typed `Signer`, so
    // Anchor's own check fires before the handler runs and reports
    // `AccountNotSigner` as 3010.
    let err = ctx
        .detach_withdrawal_queue(QueueCoSigner::Unsigned(queue.pubkey()))
        .expect_err("unsigned");
    assert_anchor_framework_err(&err, 3010);

    // Some other key signs in its place.
    let impostor = Keypair::new();
    let err = ctx
        .detach_withdrawal_queue(QueueCoSigner::Signing(&impostor))
        .expect_err("wrong signer");
    assert_anchor_err(&err, ErrorCode::WrongWithdrawalQueueSigner);
    assert_anchor_framework_err(&err, 6023);

    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(queue.pubkey()),
        "the vault must still be gated"
    );
}

/// This is the release path at the end of a drain. The stored key signs, the
/// field clears, and holders redeem directly again.
///
/// The signing key has no account on chain, and the call still succeeds. That is
/// deliberate: detach checks a signature and no account, so a queue that closed
/// its own PDA cannot strand the vault.
#[test]
fn detaching_with_the_queues_signature_restores_direct_redemption() {
    let mut ctx = vault_with_a_holder();
    let queue = gate_on_keypair(&mut ctx);
    assert!(
        ctx.svm.get_account(&queue.pubkey()).is_none(),
        "fixture: the queue key must have no account, so that this test proves detach reads none"
    );
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert_anchor_err(
        &ctx.redeem(shares).expect_err("gated"),
        ErrorCode::WithdrawalQueueRequired,
    );

    let meta = ctx
        .detach_withdrawal_queue(QueueCoSigner::Signing(&queue))
        .expect("the attached queue's signature releases the vault");

    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
    assert_single_detached(&meta, ctx.vault_state, queue.pubkey());
    ctx.redeem(shares)
        .expect("direct redemption must be open again");
}

/// Pause guards asset movement, not configuration. Detach is the exit-restoring
/// path, so pause-gating it would make a paused, gated vault unreleasable.
#[test]
fn detaching_works_while_paused() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);
    ctx.pause().expect("pause");

    ctx.detach_withdrawal_queue(QueueCoSigner::Signing(&queue))
        .expect("a paused vault can still be released");
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// The queue's signature authorises the change, but it does not stand in for the
/// admin's.
#[test]
fn a_non_admin_cannot_detach_even_with_the_queues_signature() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);
    let impostor = ctx.new_funded_keypair(1_000_000_000);

    let err = ctx
        .detach_withdrawal_queue_as(&impostor, QueueCoSigner::Signing(&queue))
        .expect_err("admin signature is still required");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(queue.pubkey())
    );
}

/// Nothing is attached, so there is no key whose signature could authorise this,
/// and the call reports that rather than a missing signature. A caller who
/// detaches twice learns the vault is already open.
#[test]
fn detaching_an_open_vault_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let anyone = Keypair::new();

    let err = ctx
        .detach_withdrawal_queue(QueueCoSigner::Signing(&anyone))
        .expect_err("no queue to detach");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueNotAttached);
    assert_anchor_framework_err(&err, 6025);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

// ---- transitions and side effects ----

/// Releases and then re-attaches, which is the sequence a vault goes through when
/// a queue is re-enabled after a drain. It shows that the two rules compose.
#[test]
fn release_then_reattach_round_trip() {
    let mut ctx = vault_with_a_holder();
    let queue = gate_on_keypair(&mut ctx);
    ctx.detach_withdrawal_queue(QueueCoSigner::Signing(&queue))
        .expect("release");
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);

    let pda = ready_queue(&mut ctx);
    ctx.attach_withdrawal_queue(pda)
        .expect("re-attach needs no co-signer once released");
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), Some(pda));

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert_anchor_err(
        &ctx.redeem(shares).expect_err("gated again"),
        ErrorCode::WithdrawalQueueRequired,
    );
}

/// A refused attach is a pure Check, so not one byte of the vault account changes.
#[test]
fn a_refused_attach_leaves_the_vault_untouched() {
    let mut ctx = vault_with_a_holder();
    let before = ctx
        .svm
        .get_account(&ctx.vault_state)
        .expect("vault state")
        .data;
    let before_balances = ctx.snapshot();

    let err = ctx
        .attach_withdrawal_queue(Pubkey::new_unique())
        .expect_err("refused");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);

    let after = ctx
        .svm
        .get_account(&ctx.vault_state)
        .expect("vault state")
        .data;
    assert_eq!(
        after, before,
        "vault_state bytes changed on a refused attach"
    );
    assert_eq!(ctx.snapshot(), before_balances);
}

/// The same holds for a refused detach on a gated vault.
#[test]
fn a_refused_detach_leaves_the_vault_untouched() {
    let mut ctx = vault_with_a_holder();
    gate_on_keypair(&mut ctx);
    let before = ctx
        .svm
        .get_account(&ctx.vault_state)
        .expect("vault state")
        .data;
    let before_balances = ctx.snapshot();

    let impostor = Keypair::new();
    let err = ctx
        .detach_withdrawal_queue(QueueCoSigner::Signing(&impostor))
        .expect_err("refused");
    assert_anchor_err(&err, ErrorCode::WrongWithdrawalQueueSigner);

    let after = ctx
        .svm
        .get_account(&ctx.vault_state)
        .expect("vault state")
        .data;
    assert_eq!(
        after, before,
        "vault_state bytes changed on a refused detach"
    );
    assert_eq!(ctx.snapshot(), before_balances);
}
