// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `set_withdrawal_queue_authority`, the only writer of the field the redeem
//! gate reads.
//!
//! Two rules have to hold. No key that fails derivation can ever be stored,
//! because a stored key nobody can sign for is a permanent exit freeze. And once
//! a key is stored, nothing but that key's own signature can change it. This
//! suite is organised around those two rules and the transitions between them.
//!
//! The queue program has no instructions yet, so two conditions are staged rather
//! than produced. "Initialized by the queue" is staged by planting an account at
//! the PDA with `install_queue_account`, and "the queue co-signs" is exercised
//! with an ordinary keypair written straight into the field. Both are faithful to
//! the rules under test, because the vault checks a signature against a stored key
//! and never asks whether that key is a PDA. The real CPI path, in which the queue
//! PDA signs through `invoke_signed`, belongs to `release_vault`'s tests.

use anchor_lang::{AnchorDeserialize, Discriminator};
use august_vault::errors::ErrorCode;
use august_vault::instructions::set_withdrawal_queue_authority::WithdrawalQueueAuthorityUpdated;
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
/// The resulting state is synthetic: the instruction refuses to store a keypair,
/// so no vault can reach it through the public API. It is faithful for the rule
/// under test, because the vault compares a signature against a stored key and
/// never asks whether that key is a PDA, but the real exit path stays unproven
/// until `release_vault` exists. See the module documentation above.
fn gate_on_keypair(ctx: &mut VaultCtx) -> Keypair {
    let queue = Keypair::new();
    let mut state = ctx.vault_state_data();
    state.withdrawal_queue_authority = queue.pubkey();
    ctx.force_overwrite_vault_state(state);
    queue
}

/// Returns every `WithdrawalQueueAuthorityUpdated` the transaction emitted, in order.
fn emitted_updates(meta: &TransactionMetadata) -> Vec<WithdrawalQueueAuthorityUpdated> {
    let disc = WithdrawalQueueAuthorityUpdated::DISCRIMINATOR;
    meta.logs
        .iter()
        .filter_map(|line| line.strip_prefix("Program data: "))
        .filter_map(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .filter(|bytes| bytes.starts_with(disc))
        .map(|bytes| {
            WithdrawalQueueAuthorityUpdated::try_from_slice(&bytes[disc.len()..])
                .expect("event body decodes")
        })
        .collect()
}

fn assert_single_update(
    meta: &TransactionMetadata,
    vault: Pubkey,
    previous: Pubkey,
    current: Pubkey,
) {
    let events = emitted_updates(meta);
    assert_eq!(
        events.len(),
        1,
        "exactly one WithdrawalQueueAuthorityUpdated"
    );
    assert_eq!(events[0].vault, vault);
    assert_eq!(events[0].previous, previous);
    assert_eq!(events[0].current, current);
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
        .set_withdrawal_queue_authority(pda, Some(pda), None)
        .expect("the vault's own initialized queue PDA must be accepted");

    assert_eq!(ctx.vault_state_data().withdrawal_queue(), Some(pda));
    assert_single_update(&meta, ctx.vault_state, Pubkey::default(), pda);
}

/// Attaching closes direct redemption, which is the whole point of the field.
#[test]
fn attaching_gates_direct_redemption() {
    let mut ctx = vault_with_a_holder();
    let pda = ready_queue(&mut ctx);
    ctx.set_withdrawal_queue_authority(pda, Some(pda), None)
        .expect("attach");

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

    ctx.set_withdrawal_queue_authority(pda, Some(pda), None)
        .expect("pause guards asset movement, not configuration");
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), Some(pda));
}

#[test]
fn a_non_admin_cannot_attach_even_the_right_pda() {
    let mut ctx = VaultCtx::fresh();
    let pda = ready_queue(&mut ctx);
    let impostor = ctx.new_funded_keypair(1_000_000_000);

    let err = ctx
        .set_withdrawal_queue_authority_as(&impostor, pda, Some(pda), None)
        .expect_err("only the admin attaches a queue");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

// ---- attach: every wrong key is refused, and nothing is stored ----

#[test]
fn an_arbitrary_wallet_is_refused() {
    let mut ctx = VaultCtx::fresh();
    // This is a real, funded, system-owned account. It exists, and it is not a
    // PDA of any program.
    let wallet = ctx.new_funded_keypair(1_000_000_000).pubkey();

    let err = ctx
        .set_withdrawal_queue_authority(wallet, Some(wallet), None)
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
        .set_withdrawal_queue_authority(other_pda, Some(other_pda), None)
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
        .set_withdrawal_queue_authority(wrong, Some(wrong), None)
        .expect_err("a PDA of any other program must be refused");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// This is the right address, but taken before `initialize_queue` has run there.
/// Storing it would gate the vault on a key the queue could not yet sign for.
#[test]
fn an_uninitialized_pda_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let pda = ctx.withdrawal_queue_pda();
    assert!(
        ctx.svm.get_account(&pda).is_none(),
        "fixture: nothing planted"
    );

    let err = ctx
        .set_withdrawal_queue_authority(pda, Some(pda), None)
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
        .set_withdrawal_queue_authority(pda, Some(pda), None)
        .expect_err("ownership by the queue program is what makes the account its own");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// The seeds and program are the same, but a lower bump also yields a valid
/// address. The queue program will sign only with the canonical bump, so this is
/// an address it could never use.
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
        .set_withdrawal_queue_authority(non_canonical, Some(non_canonical), None)
        .expect_err("only the canonical bump is accepted");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

/// The argument names the key; the account proves it exists. They must agree.
///
/// The decoy is a valid-looking queue account, namely another vault's,
/// initialized and queue-owned, and this vault's own PDA is deliberately left
/// uninstalled. Without both of those the owner check would refuse the call on the
/// same error code, and deleting the key-match check would go unnoticed. The
/// attack this forecloses is to borrow any initialized queue account as evidence
/// and attach before this vault's own `initialize_queue` has ever run.
#[test]
fn the_account_passed_must_be_the_key_named() {
    let mut ctx = VaultCtx::fresh();
    let own_pda = ctx.withdrawal_queue_pda();
    let decoy = another_vaults_ready_queue(&mut ctx);
    assert!(
        ctx.svm.get_account(&own_pda).is_none(),
        "fixture: this vault's own queue must NOT be initialized"
    );

    let err = ctx
        .set_withdrawal_queue_authority(own_pda, Some(decoy), None)
        .expect_err("another vault's queue account is not evidence for this one");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);

    let err = ctx
        .set_withdrawal_queue_authority(own_pda, None, None)
        .expect_err("right key, no account to check");
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
        .set_withdrawal_queue_authority(pda, Some(pda), None)
        .expect_err("an allocated but empty queue account is not an initialized one");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
}

// ---- detach and replace: only with the attached queue's signature ----

#[test]
fn detaching_without_the_queues_signature_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);

    // The co-signer is omitted entirely.
    let err = ctx
        .set_withdrawal_queue_authority(Pubkey::default(), None, None)
        .expect_err("omitted");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueNotDrained);
    assert_anchor_framework_err(&err, 6023);

    // The co-signer is present but does not sign. The slot is typed `Signer`, so
    // Anchor's own check fires before the handler runs and reports
    // `AccountNotSigner` as 3010. That typing is also what lets the queue sign the
    // slot by CPI without patching the meta.
    let err = ctx
        .set_withdrawal_queue_authority(
            Pubkey::default(),
            None,
            Some(QueueCoSigner::Unsigned(queue.pubkey())),
        )
        .expect_err("unsigned");
    assert_anchor_framework_err(&err, 3010);

    // Some other key signs in its place.
    let impostor = Keypair::new();
    let err = ctx
        .set_withdrawal_queue_authority(
            Pubkey::default(),
            None,
            Some(QueueCoSigner::Signing(&impostor)),
        )
        .expect_err("wrong signer");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueNotDrained);

    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(queue.pubkey()),
        "the vault must still be gated"
    );
}

/// This is the release path at the end of a drain. The stored key signs, the
/// field clears, and holders redeem directly again.
#[test]
fn detaching_with_the_queues_signature_restores_direct_redemption() {
    let mut ctx = vault_with_a_holder();
    let queue = gate_on_keypair(&mut ctx);
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert_anchor_err(
        &ctx.redeem(shares).expect_err("gated"),
        ErrorCode::WithdrawalQueueRequired,
    );

    let meta = ctx
        .set_withdrawal_queue_authority(
            Pubkey::default(),
            None,
            Some(QueueCoSigner::Signing(&queue)),
        )
        .expect("the attached queue's signature releases the vault");

    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
    assert_single_update(&meta, ctx.vault_state, queue.pubkey(), Pubkey::default());
    ctx.redeem(shares)
        .expect("direct redemption must be open again");
}

#[test]
fn replacing_requires_the_current_queues_signature() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);
    let pda = ready_queue(&mut ctx);

    let err = ctx
        .set_withdrawal_queue_authority(pda, Some(pda), None)
        .expect_err("a valid new key does not excuse the missing co-signature");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueNotDrained);
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(queue.pubkey())
    );

    let meta = ctx
        .set_withdrawal_queue_authority(pda, Some(pda), Some(QueueCoSigner::Signing(&queue)))
        .expect("replace with the current queue co-signing");
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), Some(pda));
    assert_single_update(&meta, ctx.vault_state, queue.pubkey(), pda);
}

/// The queue's signature authorises the change, but it does not stand in for the
/// admin's.
#[test]
fn a_non_admin_cannot_detach_even_with_the_queues_signature() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);
    let impostor = ctx.new_funded_keypair(1_000_000_000);

    let err = ctx
        .set_withdrawal_queue_authority_as(
            &impostor,
            Pubkey::default(),
            None,
            Some(QueueCoSigner::Signing(&queue)),
        )
        .expect_err("admin signature is still required");
    assert_anchor_err(&err, ErrorCode::NotAdmin);
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(queue.pubkey())
    );
}

/// A co-signature authorises the change, but it does not excuse the new key.
/// Without this test the attach checks are never exercised with a queue already
/// attached, and that is the unrecoverable direction: clearing a bogus stored key
/// would require that key's own signature.
#[test]
fn a_gated_vault_still_validates_the_replacement_key() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);

    for bad in [Pubkey::new_unique(), ctx.withdrawal_queue_pda()] {
        let err = ctx
            .set_withdrawal_queue_authority(bad, Some(bad), Some(QueueCoSigner::Signing(&queue)))
            .expect_err("co-signed, but the new key is still invalid");
        assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
        assert_eq!(
            ctx.vault_state_data().withdrawal_queue(),
            Some(queue.pubkey()),
            "a refused replacement must leave the original queue attached"
        );
    }
}

/// On a gated vault the co-sign rule is judged first, so a call that is wrong
/// in both ways reports the rule protecting the queued holders.
#[test]
fn the_co_sign_rule_is_reported_before_the_new_key_is_judged() {
    let mut ctx = VaultCtx::fresh();
    gate_on_keypair(&mut ctx);
    let junk = Pubkey::new_unique();

    let err = ctx
        .set_withdrawal_queue_authority(junk, Some(junk), None)
        .expect_err("wrong on both counts");
    assert_anchor_err(&err, ErrorCode::WithdrawalQueueNotDrained);
}

// ---- detaching consults no account ----

/// This is the vacuous-success path, where a client fills the account slot but
/// leaves the argument at its default. Ignoring `new_queue` here would return
/// success on a vault that is still ungated.
#[test]
fn an_attach_whose_argument_was_left_default_is_refused() {
    let mut ctx = vault_with_a_holder();
    let pda = ready_queue(&mut ctx);

    let err = ctx
        .set_withdrawal_queue_authority(Pubkey::default(), Some(pda), None)
        .expect_err("naming the zero key while pointing at a queue is a contradiction");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);

    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    ctx.redeem(shares)
        .expect("the vault is still ungated, whatever the caller believed");
}

/// The same rule applies on a genuine detach.
#[test]
fn detaching_must_not_carry_a_new_queue_account() {
    let mut ctx = VaultCtx::fresh();
    let queue = gate_on_keypair(&mut ctx);
    let pda = ready_queue(&mut ctx);

    let err = ctx
        .set_withdrawal_queue_authority(
            Pubkey::default(),
            Some(pda),
            Some(QueueCoSigner::Signing(&queue)),
        )
        .expect_err("detach takes no new_queue");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);
    assert_eq!(
        ctx.vault_state_data().withdrawal_queue(),
        Some(queue.pubkey())
    );
}

/// Clearing an already-clear field is permitted, and it still emits an event.
/// This pins the event's claim that a no-op re-set is included.
#[test]
fn detaching_an_already_detached_vault_is_an_idempotent_no_op() {
    let mut ctx = VaultCtx::fresh();

    let meta = ctx
        .set_withdrawal_queue_authority(Pubkey::default(), None, None)
        .expect("a redundant detach is not an error");

    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);
    assert_single_update(&meta, ctx.vault_state, Pubkey::default(), Pubkey::default());
}

// ---- transitions and side effects ----

/// Releases and then re-attaches, which is the sequence a vault goes through when
/// a queue is re-enabled after a drain. It shows that the two rules compose.
#[test]
fn release_then_reattach_round_trip() {
    let mut ctx = vault_with_a_holder();
    let queue = gate_on_keypair(&mut ctx);
    ctx.set_withdrawal_queue_authority(
        Pubkey::default(),
        None,
        Some(QueueCoSigner::Signing(&queue)),
    )
    .expect("release");
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), None);

    let pda = ready_queue(&mut ctx);
    ctx.set_withdrawal_queue_authority(pda, Some(pda), None)
        .expect("re-attach needs no co-signer once released");
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), Some(pda));

    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert_anchor_err(
        &ctx.redeem(shares).expect_err("gated again"),
        ErrorCode::WithdrawalQueueRequired,
    );
}

/// A refused call is a pure Check, so not one byte of the vault account changes.
#[test]
fn a_refused_call_leaves_the_vault_untouched() {
    let mut ctx = vault_with_a_holder();
    let before = ctx
        .svm
        .get_account(&ctx.vault_state)
        .expect("vault state")
        .data;
    let before_balances = ctx.snapshot();

    let junk = Pubkey::new_unique();
    let err = ctx
        .set_withdrawal_queue_authority(junk, Some(junk), None)
        .expect_err("refused");
    assert_anchor_err(&err, ErrorCode::InvalidWithdrawalQueueAuthority);

    let after = ctx
        .svm
        .get_account(&ctx.vault_state)
        .expect("vault state")
        .data;
    assert_eq!(after, before, "vault_state bytes changed on a refused call");
    assert_eq!(ctx.snapshot(), before_balances);
}
