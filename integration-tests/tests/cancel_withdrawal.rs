// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `cancel_withdrawal` returns a pending request's escrowed shares to a share
//! account the owner controls and closes the request. It touches no vault
//! account, so it works before eligibility, after expiry, while the vault is
//! paused, and whether or not the queue is attached.

use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::WithdrawalCancelled;
use integration_tests::harness::{
    assert_anchor_framework_err, events_of, VaultCtx, DEPOSIT_DECIMALS,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::{pubkey::Pubkey, signer::Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DAY: u64 = 24 * 60 * 60;
/// Anchor's `AccountNotInitialized`: a closed request, or a missing destination.
const ACCOUNT_NOT_INITIALIZED: u32 = 3012;
/// LiteSVM's fee per signature; the owner signs a cancel, so their lamports move
/// by the rent returned minus this.
const FEE: u64 = 5_000;

fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

/// Everything a refused cancel must leave alone.
#[derive(Debug, PartialEq, Eq)]
struct Untouched {
    request: Option<(Vec<u8>, u64)>,
    escrow_shares: u64,
    destination: Option<u64>,
    counters: (u64, u64),
}

fn untouched(ctx: &VaultCtx, owner: &Pubkey, id: u64, destination: &Pubkey) -> Untouched {
    let q = ctx.queue_state_data();
    Untouched {
        request: ctx
            .svm
            .get_account(&ctx.request_pda(owner, id))
            .filter(|a| a.lamports > 0)
            .map(|a| (a.data, a.lamports)),
        escrow_shares: ctx.token_account_amount(&ctx.queue_escrow(&ctx.share_mint)),
        destination: ctx
            .svm
            .get_account(destination)
            .filter(|a| a.lamports > 0)
            .map(|_| ctx.token_account_amount(destination)),
        counters: (q.pending_requests, q.pending_shares),
    }
}

/// A vault with a holder and its queue attached; the holder has requested half
/// their shares as request 1. Returns the context and that half.
fn holder_with_request(ctx: VaultCtx) -> (VaultCtx, u64) {
    let mut ctx = ctx;
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(DAY);
    let half = ctx.token_account_amount(&ctx.user_share_ata) / 2;
    ctx.request_withdrawal(1, half).expect("request");
    (ctx, half)
}

fn assert_full_cancelled_event(
    meta: &litesvm::types::TransactionMetadata,
    ctx: &VaultCtx,
    owner: &Pubkey,
    id: u64,
    sequence: u64,
    shares: u64,
    destination: &Pubkey,
) {
    let events = events_of::<WithdrawalCancelled>(meta);
    assert_eq!(events.len(), 1, "exactly one WithdrawalCancelled");
    let e = &events[0];
    assert_eq!(e.vault, ctx.vault_state);
    assert_eq!(e.queue, ctx.withdrawal_queue_pda());
    assert_eq!(e.request, ctx.request_pda(owner, id));
    assert_eq!((e.request_id, e.owner, e.sequence), (id, *owner, sequence));
    assert_eq!(
        (e.by, e.shares, e.destination),
        (*owner, shares, *destination)
    );
}

// ---- the happy path ----

#[test]
fn the_owner_cancels_a_pending_request_and_gets_shares_and_rent_back() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh());
    let user = ctx.user.pubkey();
    let ata = ctx.user_share_ata;
    let before = untouched(&ctx, &user, 1, &ata);
    let rent = before.request.as_ref().expect("request exists").1;
    let lamports_before = ctx.svm.get_balance(&user).expect("owner");

    let meta = ctx
        .cancel_withdrawal(1, 1)
        .expect("cancel before eligibility");
    eprintln!(
        "cancel_withdrawal consumed {} CU",
        meta.compute_units_consumed
    );
    assert!(
        meta.compute_units_consumed < 30_000,
        "the design budgets about 30k CU for a cancel; measured {}",
        meta.compute_units_consumed
    );

    let after = untouched(&ctx, &user, 1, &ata);
    assert_eq!(
        after.destination.expect("ata") - before.destination.expect("ata"),
        half,
        "exactly the escrowed shares come back"
    );
    assert_eq!(before.escrow_shares - after.escrow_shares, half);
    assert_eq!(after.request, None, "the request account is closed");
    assert_eq!(
        ctx.svm.get_balance(&user).expect("owner"),
        lamports_before + rent - FEE,
        "rent returns to the owner"
    );
    assert_eq!(after.counters, (0, 0));
    assert_full_cancelled_event(&meta, &ctx, &user, 1, 1, half, &ata);
}

#[test]
fn a_token_2022_request_cancels_too() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh_token_2022());
    let user = ctx.user.pubkey();
    let before = ctx.token_account_amount(&ctx.user_share_ata);
    let meta = ctx
        .cancel_withdrawal(1, 1)
        .expect("cancel under Token-2022");
    assert_eq!(ctx.token_account_amount(&ctx.user_share_ata) - before, half);
    assert_full_cancelled_event(&meta, &ctx, &user, 1, 1, half, &ctx.user_share_ata);
}

/// Decision 7 and decision 13 together: no vault account is read, so neither
/// pause, nor expiry, nor a released vault stops the owner reclaiming shares.
#[test]
fn cancel_never_depends_on_the_vault() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh());
    let quarter = half / 2;
    ctx.set_fulfillment_window(DAY).expect("window");
    ctx.request_withdrawal(2, quarter).expect("windowed");
    ctx.request_withdrawal(3, quarter).expect("third");

    ctx.pause().expect("pause");
    ctx.cancel_withdrawal(1, 1).expect("cancel while paused");
    ctx.unpause().expect("unpause");

    ctx.warp_forward_seconds(3 * DAY as i64);
    let r = ctx.request_state_data(&ctx.user.pubkey(), 2);
    assert!(r.is_expired(ctx.now()));
    ctx.cancel_withdrawal(2, 2).expect("cancel after expiry");

    let mut vault = ctx.vault_state_data();
    vault.withdrawal_queue_authority = Pubkey::default();
    ctx.force_overwrite_vault_state(vault);
    ctx.cancel_withdrawal(3, 3)
        .expect("cancel with the vault released");
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
}

#[test]
fn cancelling_one_request_leaves_the_others_untouched() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh());
    let user = ctx.user.pubkey();
    let quarter = half / 2;
    ctx.request_withdrawal(2, quarter).expect("second");
    ctx.request_withdrawal(3, quarter).expect("third");
    let second = ctx.request_pda(&user, 2);
    let second_before = ctx.svm.get_account(&second).expect("second").data;

    ctx.cancel_withdrawal(3, 3).expect("cancel the third");

    assert_eq!(
        ctx.svm.get_account(&second).expect("second").data,
        second_before
    );
    let q = ctx.queue_state_data();
    assert_eq!((q.pending_requests, q.pending_shares), (2, half + quarter));
    assert_eq!(
        ctx.token_account_amount(&ctx.queue_escrow(&ctx.share_mint)),
        half + quarter,
        "counters equal the escrow balance"
    );
}

// ---- identity ----

#[test]
fn only_the_owner_cancels_and_a_stale_sequence_is_refused() {
    let (mut ctx, _) = holder_with_request(VaultCtx::fresh());
    let user = ctx.user.pubkey();
    let ata = ctx.user_share_ata;
    let before = untouched(&ctx, &user, 1, &ata);

    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let share_mint = ctx.share_mint;
    let impostor_ata = ctx.create_ata_for(&impostor.pubkey(), &share_mint);
    let err = ctx
        .cancel_withdrawal_as(&impostor, &user, 1, 1, impostor_ata)
        .expect_err("not the owner");
    assert_queue_err(&err, ErrorCode::NotRequestOwner);
    assert_anchor_framework_err(&err, 6009);

    let err = ctx.cancel_withdrawal(1, 2).expect_err("wrong stamp");
    assert_queue_err(&err, ErrorCode::StaleRequestSequence);
    assert_anchor_framework_err(&err, 6011);

    assert_eq!(untouched(&ctx, &user, 1, &ata), before, "nothing moved");
    assert_eq!(ctx.token_account_amount(&impostor_ata), 0);
}

/// Cancel and finalize each close the account, so whichever lands first makes
/// the other find nothing. Both account sets are built while the request exists.
#[test]
fn cancel_and_finalize_are_mutually_exclusive() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh());
    let signer = ctx.user.insecure_clone();
    let user = signer.pubkey();
    ctx.request_withdrawal(2, half / 2).expect("second");
    ctx.warp_forward_seconds(DAY as i64);
    let finalize_1 = ctx.finalize_withdrawal_accounts(&user, &user, 1);
    let cancel_2 = ctx.cancel_withdrawal_accounts(&user, &user, 2, ctx.user_share_ata);

    ctx.cancel_withdrawal(1, 1).expect("cancel first");
    let err = ctx
        .send_finalize_withdrawal(&signer, finalize_1, 1)
        .expect_err("finalize a cancelled request");
    assert_anchor_framework_err(&err, ACCOUNT_NOT_INITIALIZED);

    ctx.finalize_withdrawal(2, 2).expect("finalize second");
    let err = ctx
        .send_cancel_withdrawal(&signer, cancel_2, 2)
        .expect_err("cancel a finalized request");
    assert_anchor_framework_err(&err, ACCOUNT_NOT_INITIALIZED);
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
}

/// Decision 11's stale-instruction case, end to end: cancel, reuse the id, then
/// land a second cancel signed against the old request. It fails on the stamp
/// and the new request is untouched.
#[test]
fn a_cancel_signed_against_a_cancelled_request_cannot_hit_its_successor() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh());
    let user = ctx.user.pubkey();
    ctx.cancel_withdrawal(1, 1).expect("cancel");
    ctx.request_withdrawal(1, half / 2).expect("reuse the id");
    let successor = ctx.request_state_data(&user, 1);
    assert_eq!(successor.sequence, 2, "a fresh stamp");

    let err = ctx
        .cancel_withdrawal(1, 1)
        .expect_err("signed against the old request");
    assert_queue_err(&err, ErrorCode::StaleRequestSequence);
    let after = ctx.request_state_data(&user, 1);
    assert_eq!(
        (after.sequence, after.shares, after.requested_at),
        (successor.sequence, successor.shares, successor.requested_at)
    );
}

// ---- the destination ----

/// Any share account whose authority is the owner, and it must exist: the SDK
/// creates the ATA in the same transaction when it is missing. The reassigned
/// ATA case is why "any": an owner who moved their ATA's authority elsewhere
/// pays a fresh account instead, and the account they gave away gets nothing.
#[test]
fn the_destination_is_any_existing_share_account_the_owner_controls() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh());
    let user = ctx.user.insecure_clone();
    let (ata, share_mint) = (ctx.user_share_ata, ctx.share_mint);
    let quarter = half / 2;
    ctx.request_withdrawal(2, quarter).expect("second");
    let before = untouched(&ctx, &user.pubkey(), 1, &ata);

    // Wrong mint, someone else's share account, an account that does not exist.
    let other = ctx.new_depositor(0);
    let cases = [
        ("a deposit-mint account", ctx.user_deposit_ata, 2014),
        ("another holder's share account", other.share_ata, 2015),
        (
            "a missing account",
            Pubkey::new_unique(),
            ACCOUNT_NOT_INITIALIZED,
        ),
    ];
    for (what, destination, code) in cases {
        let err = ctx
            .cancel_withdrawal_as(&user, &user.pubkey(), 1, 1, destination)
            .err()
            .unwrap_or_else(|| panic!("{what} was accepted"));
        assert_anchor_framework_err(&err, code);
        assert_eq!(untouched(&ctx, &user.pubkey(), 1, &ata), before, "{what}");
    }

    // A plain (non-ATA) account with the owner as authority is fine.
    let plain = ctx.create_token_account_for(&user.pubkey(), &share_mint);
    ctx.cancel_withdrawal_as(&user, &user.pubkey(), 1, 1, plain)
        .expect("a non-ATA share account");
    assert_eq!(ctx.token_account_amount(&plain), half);

    // The ATA given away to a stranger is refused; a fresh account is paid.
    let stranger = ctx.new_funded_keypair(1_000_000_000).pubkey();
    ctx.set_token_account_authority_as(&user, &ata, &stranger);
    let err = ctx
        .cancel_withdrawal_as(&user, &user.pubkey(), 2, 2, ata)
        .expect_err("the ATA now belongs to the stranger");
    assert_anchor_framework_err(&err, 2015);
    let fresh = ctx.create_token_account_for(&user.pubkey(), &share_mint);
    ctx.cancel_withdrawal_as(&user, &user.pubkey(), 2, 2, fresh)
        .expect("a fresh owner-controlled account");
    assert_eq!(ctx.token_account_amount(&fresh), quarter);
    assert_eq!(
        ctx.token_account_amount(&ata),
        quarter,
        "the stranger's account only holds what the owner had left there"
    );
}

/// The escrow and mint slots are bound to the queue's stored keys; a
/// substituted escrow would pay the owner out of a stranger's account while
/// the real escrow kept the shares.
#[test]
fn substituted_accounts_are_refused() {
    let (mut ctx, _) = holder_with_request(VaultCtx::fresh());
    let user = ctx.user.insecure_clone();
    let ata = ctx.user_share_ata;
    let before = untouched(&ctx, &user.pubkey(), 1, &ata);
    let stranger = ctx.new_funded_keypair(1_000_000_000).pubkey();
    let (share_mint, deposit_mint) = (ctx.share_mint, ctx.deposit_mint);
    let foreign_escrow = ctx.create_ata_for(&stranger, &share_mint);

    let mut accounts = ctx.cancel_withdrawal_accounts(&user.pubkey(), &user.pubkey(), 1, ata);
    accounts.escrow_shares = foreign_escrow;
    let err = ctx
        .send_cancel_withdrawal(&user, accounts, 1)
        .expect_err("escrow substituted");
    assert_anchor_framework_err(&err, 2001);

    let mut accounts = ctx.cancel_withdrawal_accounts(&user.pubkey(), &user.pubkey(), 1, ata);
    accounts.share_mint = deposit_mint;
    let err = ctx
        .send_cancel_withdrawal(&user, accounts, 1)
        .expect_err("mint substituted");
    assert_anchor_framework_err(&err, 2001);

    assert_eq!(untouched(&ctx, &user.pubkey(), 1, &ata), before);
    assert_eq!(ctx.token_account_amount(&foreign_escrow), 0);
}

/// A request belongs to one queue. Presenting another vault's queue, escrow and
/// share mint with it would pay the owner out of that vault's escrow; the
/// request's `has_one = queue` refuses the set before anything moves.
#[test]
fn a_request_cannot_be_cancelled_through_another_vaults_queue() {
    let (mut ctx, _) = holder_with_request(VaultCtx::fresh());
    let user = ctx.user.insecure_clone();
    let other = ctx.new_vault_with_queue();
    let destination = ctx.create_token_account_for(&user.pubkey(), &other.share_mint);

    let mut accounts =
        ctx.cancel_withdrawal_accounts(&user.pubkey(), &user.pubkey(), 1, destination);
    accounts.queue = other.queue;
    accounts.escrow_shares = other.escrow_shares;
    accounts.share_mint = other.share_mint;
    let err = ctx
        .send_cancel_withdrawal(&user, accounts, 1)
        .expect_err("another vault's queue");
    assert_anchor_framework_err(&err, 2001);
    assert_eq!(ctx.queue_state_data().pending_requests, 1);
    assert_eq!(ctx.token_account_amount(&destination), 0);
}
