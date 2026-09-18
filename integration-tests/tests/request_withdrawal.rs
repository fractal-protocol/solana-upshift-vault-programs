// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `request_withdrawal` escrows shares and stamps a request with the cooldown
//! and window in force; `update_request` lets the owner change floor, recipient
//! and finalizer, nothing else. Neither writes the vault; `request_withdrawal`
//! reads its gate.

use august_vault::state::vault::VAULT_STATE_SEED;
use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::{WithdrawalRequestUpdated, WithdrawalRequested};
use august_withdrawal_queue::state::{WithdrawalRequest, WITHDRAWAL_REQUEST_SEED};
use integration_tests::harness::{
    assert_anchor_framework_err, events_of, VaultCtx, DEPOSIT_DECIMALS, VAULT_VERSION,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::{pubkey::Pubkey, signer::Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DAY: u64 = 24 * 60 * 60;

fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

fn assert_log_contains(err: &FailedTransactionMetadata, needle: &str) {
    assert!(
        err.meta.logs.iter().any(|l| l.contains(needle)),
        "expected a log line containing {needle:?}; logs:\n{}",
        err.meta.logs.join("\n")
    );
}

/// A vault with a holder, and its queue initialized, attached and accepting.
fn open_vault_with_holder(cooldown: u64) -> (VaultCtx, u64) {
    open_with_holder(VaultCtx::fresh(), cooldown)
}

fn open_with_holder(mut ctx: VaultCtx, cooldown: u64) -> (VaultCtx, u64) {
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(cooldown);
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    assert!(shares > 0);
    (ctx, shares)
}

fn escrow_shares_balance(ctx: &VaultCtx) -> u64 {
    ctx.token_account_amount(&ctx.queue_escrow(&ctx.share_mint))
}

fn assert_full_request_event(
    meta: &litesvm::types::TransactionMetadata,
    ctx: &VaultCtx,
    owner: &Pubkey,
    id: u64,
) {
    let r = ctx.request_state_data(owner, id);
    let events = events_of::<WithdrawalRequested>(meta);
    assert_eq!(events.len(), 1, "exactly one WithdrawalRequested");
    let e = &events[0];
    assert_eq!(e.vault, ctx.vault_state);
    assert_eq!(e.queue, ctx.withdrawal_queue_pda());
    assert_eq!(e.request, ctx.request_pda(owner, id));
    assert_eq!(
        (e.request_id, e.owner, e.sequence),
        (id, *owner, r.sequence)
    );
    assert_eq!((e.shares, e.min_assets_out), (r.shares, r.min_assets_out));
    assert_eq!(e.recipient_token_account, r.recipient_token_account);
    assert_eq!(e.finalizer, r.finalizer);
    assert_eq!((e.eligible_at, e.expires_at), (r.eligible_at, r.expires_at));
}

fn assert_full_update_event(
    meta: &litesvm::types::TransactionMetadata,
    ctx: &VaultCtx,
    owner: &Pubkey,
    id: u64,
) {
    let r = ctx.request_state_data(owner, id);
    let events = events_of::<WithdrawalRequestUpdated>(meta);
    assert_eq!(events.len(), 1, "exactly one WithdrawalRequestUpdated");
    let e = &events[0];
    assert_eq!(e.vault, ctx.vault_state);
    assert_eq!(e.queue, ctx.withdrawal_queue_pda());
    assert_eq!(e.request, ctx.request_pda(owner, id));
    assert_eq!(
        (e.request_id, e.owner, e.sequence),
        (id, *owner, r.sequence)
    );
    assert_eq!(e.min_assets_out, r.min_assets_out);
    assert_eq!(e.recipient_token_account, r.recipient_token_account);
    assert_eq!(e.finalizer, r.finalizer);
}

// ---- request_withdrawal ----

#[test]
fn a_holder_escrows_shares_and_opens_a_request() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let half = shares / 2;
    let now = ctx.now();
    let user = ctx.user.pubkey();

    let meta = ctx
        .request_withdrawal(7, half, 1_000)
        .expect("request_withdrawal");
    eprintln!(
        "request_withdrawal consumed {} CU",
        meta.compute_units_consumed
    );
    assert!(
        meta.compute_units_consumed < 40_000,
        "the design budgets about 40k CU for a request; measured {}",
        meta.compute_units_consumed
    );

    assert_eq!(ctx.token_account_amount(&ctx.user_share_ata), shares - half);
    assert_eq!(
        escrow_shares_balance(&ctx),
        half,
        "exactly `shares` escrowed"
    );

    let r = ctx.request_state_data(&user, 7);
    let pda = ctx.request_pda(&user, 7);
    assert_eq!(r.queue, ctx.withdrawal_queue_pda());
    assert_eq!(r.owner, user);
    assert_eq!(r.recipient_token_account, ctx.user_deposit_ata);
    assert_eq!(r.allowed_finalizer(), None);
    assert_eq!(r.shares, half);
    assert_eq!(r.min_assets_out, 1_000);
    assert_eq!(
        r.request_id, 7,
        "the id is the owner's, distinct from the stamp"
    );
    assert_eq!(r.sequence, 1, "the first stamp is 1");
    assert_eq!(r.requested_at, now);
    assert_eq!(r.scheduled_eligible_at, now + DAY as i64);
    assert_eq!(r.eligible_at, r.scheduled_eligible_at);
    assert_eq!(r.expires_at, 0, "no window configured, so never expires");
    let (_, bump) = Pubkey::find_program_address(
        &[
            WITHDRAWAL_REQUEST_SEED,
            ctx.withdrawal_queue_pda().as_ref(),
            user.as_ref(),
            &7u64.to_le_bytes(),
        ],
        &august_withdrawal_queue::ID,
    );
    assert_eq!(r.bump, bump);

    let q = ctx.queue_state_data();
    assert_eq!(
        (q.sequence, q.pending_requests, q.pending_shares),
        (1, 1, half)
    );
    assert_full_request_event(&meta, &ctx, &user, 7);

    let account = ctx.svm.get_account(&pda).expect("request account");
    assert_eq!(account.data.len(), WithdrawalRequest::LEN);
    assert_eq!(account.owner, august_withdrawal_queue::ID);
}

/// The same path under Token-2022: the escrow transfer goes through the
/// interface into an ATA that carries the `ImmutableOwner` extension.
#[test]
fn a_token_2022_holder_escrows_shares_too() {
    let (mut ctx, shares) = open_with_holder(VaultCtx::fresh_token_2022(), DAY);
    let user = ctx.user.pubkey();
    let meta = ctx
        .request_withdrawal(1, shares / 2, 5)
        .expect("request under Token-2022");
    assert_eq!(escrow_shares_balance(&ctx), shares / 2);
    assert_eq!(ctx.request_state_data(&user, 1).shares, shares / 2);
    assert_full_request_event(&meta, &ctx, &user, 1);

    ctx.update_request(1, 1, Some(9), None, None)
        .expect("update under Token-2022");
    assert_eq!(ctx.request_state_data(&user, 1).min_assets_out, 9);
}

/// The request-level finalizer is the owner's time control (decision 14); it
/// must be stored and reported as given, not defaulted.
#[test]
fn a_request_can_name_its_finalizer() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.insecure_clone();
    let (share_account, recipient) = (ctx.user_share_ata, ctx.user_deposit_ata);
    let ops = Pubkey::new_unique();

    let meta = ctx
        .request_withdrawal_as(&user, share_account, recipient, 1, shares / 2, 0, ops)
        .expect("request with a finalizer");
    assert_eq!(
        ctx.request_state_data(&user.pubkey(), 1)
            .allowed_finalizer(),
        Some(ops)
    );
    assert_eq!(events_of::<WithdrawalRequested>(&meta)[0].finalizer, ops);
}

/// The cooldown and window are stamped at request time; changing them later
/// leaves existing requests alone and applies to the next one.
#[test]
fn timestamps_are_stamped_from_the_settings_at_request_time() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.set_fulfillment_window(7 * DAY).expect("window");
    let user = ctx.user.pubkey();
    let now = ctx.now();

    let meta = ctx.request_withdrawal(1, shares / 4, 0).expect("first");
    let first = ctx.request_state_data(&user, 1);
    assert_eq!(first.scheduled_eligible_at, now + DAY as i64);
    assert_eq!(
        first.expires_at,
        first.scheduled_eligible_at + 7 * DAY as i64
    );
    assert_eq!(
        events_of::<WithdrawalRequested>(&meta)[0].expires_at,
        first.expires_at
    );

    ctx.set_cooldown(2 * DAY).expect("new cooldown");
    ctx.set_fulfillment_window(0).expect("no window");
    ctx.request_withdrawal(2, shares / 4, 0).expect("second");
    let still_first = ctx.request_state_data(&user, 1);
    assert_eq!(
        (still_first.scheduled_eligible_at, still_first.expires_at),
        (first.scheduled_eligible_at, first.expires_at),
        "an existing request keeps its schedule and its deadline"
    );
    let second = ctx.request_state_data(&user, 2);
    assert_eq!(second.scheduled_eligible_at, now + 2 * DAY as i64);
    assert_eq!(second.expires_at, 0);
}

#[test]
fn requests_are_owner_scoped_and_sequenced() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    ctx.request_withdrawal(7, shares / 4, 0).expect("id 7");
    ctx.request_withdrawal(3, shares / 4, 0).expect("id 3");

    let (a, b) = (
        ctx.request_state_data(&user, 7),
        ctx.request_state_data(&user, 3),
    );
    assert_eq!((a.request_id, a.sequence), (7, 1));
    assert_eq!(
        (b.request_id, b.sequence),
        (3, 2),
        "ids are the owner's; order is the queue's"
    );
    assert_ne!(ctx.request_pda(&user, 7), ctx.request_pda(&user, 3));
    let q = ctx.queue_state_data();
    assert_eq!((q.pending_requests, q.pending_shares), (2, shares / 2));

    // An id in use cannot be reused while its request exists: `init` hits the
    // System Program's "account already in use".
    let err = ctx
        .request_withdrawal(7, 1, 0)
        .expect_err("request 7 already exists");
    assert_anchor_framework_err(&err, 0);
    assert_log_contains(&err, "already in use");
    let q = ctx.queue_state_data();
    assert_eq!(
        (q.pending_requests, q.pending_shares),
        (2, shares / 2),
        "nothing changed"
    );
}

/// Two owners may use the same id: identity is per owner (decision 11).
#[test]
fn two_owners_interleave_without_colliding() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let alice = ctx.user.pubkey();
    let bob = ctx.new_depositor(DEPOSIT_AMOUNT);
    ctx.deposit_as(&bob, DEPOSIT_AMOUNT).expect("bob deposits");
    let bob_shares = ctx.token_account_amount(&bob.share_ata);

    ctx.request_withdrawal(1, shares / 2, 0)
        .expect("alice id 1");
    ctx.request_withdrawal_as(
        &bob.keypair,
        bob.share_ata,
        bob.deposit_ata,
        1,
        bob_shares / 2,
        0,
        Pubkey::default(),
    )
    .expect("bob id 1");

    assert_ne!(
        ctx.request_pda(&alice, 1),
        ctx.request_pda(&bob.keypair.pubkey(), 1)
    );
    let (a, b) = (
        ctx.request_state_data(&alice, 1),
        ctx.request_state_data(&bob.keypair.pubkey(), 1),
    );
    assert_eq!((a.owner, a.sequence), (alice, 1));
    assert_eq!((b.owner, b.sequence), (bob.keypair.pubkey(), 2));
    let q = ctx.queue_state_data();
    assert_eq!(
        (q.pending_requests, q.pending_shares),
        (2, shares / 2 + bob_shares / 2)
    );
    assert_eq!(escrow_shares_balance(&ctx), shares / 2 + bob_shares / 2);
}

#[test]
fn zero_shares_is_refused() {
    let (mut ctx, _) = open_vault_with_holder(DAY);
    let err = ctx.request_withdrawal(1, 0, 0).expect_err("zero");
    assert_queue_err(&err, ErrorCode::ZeroShares);
    assert_anchor_framework_err(&err, 6008);
}

/// The escrow transfer is the token program's; asking for more than the owner
/// holds fails there, and the whole transaction, request account included, is
/// rolled back.
#[test]
fn more_shares_than_held_is_refused_by_the_token_program() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    let err = ctx
        .request_withdrawal(1, shares + 1, 0)
        .expect_err("insufficient shares");
    // SPL Token `InsufficientFunds` is code 1; so is a System Program rent
    // shortfall, so pin the token program's log line too.
    assert_anchor_framework_err(&err, 1);
    assert_log_contains(&err, "insufficient funds");
    assert!(ctx.svm.get_account(&ctx.request_pda(&user, 1)).is_none());
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
    assert_eq!(escrow_shares_balance(&ctx), 0);
}

#[test]
fn the_share_account_must_hold_the_share_mint_and_belong_to_the_signer() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.insecure_clone();
    let recipient = ctx.user_deposit_ata;

    // Wrong mint: the user's deposit-token account.
    let err = ctx
        .request_withdrawal_as(
            &user,
            ctx.user_deposit_ata,
            recipient,
            1,
            1,
            0,
            Pubkey::default(),
        )
        .expect_err("not a share account");
    assert_anchor_framework_err(&err, 2014); // ConstraintTokenMint

    // Right mint, someone else's account.
    let other = ctx.new_depositor(DEPOSIT_AMOUNT);
    let err = ctx
        .request_withdrawal_as(
            &user,
            other.share_ata,
            recipient,
            1,
            shares / 2,
            0,
            Pubkey::default(),
        )
        .expect_err("not the signer's account");
    assert_anchor_framework_err(&err, 2015); // ConstraintTokenOwner
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
}

/// The bindings that keep one holder's request off another holder's shares.
/// These are Anchor constraints, not handler guards, so mutation of the
/// handler cannot see them; each is exercised by substituting an account.
#[test]
fn substituted_accounts_are_refused_before_anything_moves() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.insecure_clone();
    let (share_account, recipient) = (ctx.user_share_ata, ctx.user_deposit_ata);
    let genuine = ctx.request_withdrawal_accounts(&user.pubkey(), share_account, recipient, 1);

    // A stranger's share ATA in the escrow slot: the transfer would land there
    // while `pending_shares` grew, and finalize would later redeem out of the
    // real escrow, out of other holders' shares.
    let stranger = ctx.new_funded_keypair(1_000_000_000).pubkey();
    let share_mint = ctx.share_mint;
    let foreign_escrow = ctx.create_ata_for(&stranger, &share_mint);
    let mut accounts = ctx.request_withdrawal_accounts(&user.pubkey(), share_account, recipient, 1);
    accounts.escrow_shares = foreign_escrow;
    let err = ctx
        .send_request_withdrawal(&user, accounts, 1, shares / 2, 0, Pubkey::default())
        .expect_err("escrow substituted");
    assert_anchor_framework_err(&err, 2001); // ConstraintHasOne
    assert_eq!(ctx.token_account_amount(&foreign_escrow), 0);
    assert_eq!(escrow_shares_balance(&ctx), 0);
    assert_eq!(ctx.queue_state_data().pending_shares, 0);

    // Another genuine vault in the vault slot.
    let mint_b = ctx.create_extra_deposit_mint();
    let authority = ctx.protocol_authority.insecure_clone();
    ctx.try_initialize_vault(&authority, mint_b, VAULT_VERSION)
        .expect("vault B");
    let vault_b = Pubkey::find_program_address(
        &[VAULT_STATE_SEED, mint_b.as_ref(), &[VAULT_VERSION]],
        &august_vault::ID,
    )
    .0;
    let mut accounts = ctx.request_withdrawal_accounts(&user.pubkey(), share_account, recipient, 1);
    accounts.vault_state = vault_b;
    let err = ctx
        .send_request_withdrawal(&user, accounts, 1, shares / 2, 0, Pubkey::default())
        .expect_err("vault substituted");
    assert_queue_err(&err, ErrorCode::VaultMismatch);

    // A different mint in the share-mint slot.
    let mut accounts = genuine;
    accounts.share_mint = ctx.deposit_mint;
    let err = ctx
        .send_request_withdrawal(&user, accounts, 1, shares / 2, 0, Pubkey::default())
        .expect_err("share mint substituted");
    assert_anchor_framework_err(&err, 2001);
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
}

#[test]
fn drain_mode_refuses_new_requests() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.set_accepting_requests(false).expect("drain");
    let err = ctx.request_withdrawal(1, shares, 0).expect_err("draining");
    assert_queue_err(&err, ErrorCode::NotAcceptingRequests);
    assert_anchor_framework_err(&err, 6007);
}

/// Decision 13, defence in depth. No instruction sequence reaches this state
/// (detach needs the queue's signature, given only by `release_vault` in drain
/// mode), so the test writes the vault directly.
#[test]
fn a_queue_the_vault_no_longer_points_at_refuses_requests() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let mut vault = ctx.vault_state_data();
    vault.withdrawal_queue_authority = Pubkey::default();
    ctx.force_overwrite_vault_state(vault);
    assert!(
        ctx.queue_state_data().accepting_requests,
        "the queue still believes it is open"
    );

    let err = ctx
        .request_withdrawal(1, shares, 0)
        .expect_err("gate is gone");
    assert_queue_err(&err, ErrorCode::QueueNotActiveOnVault);
}

/// Decision 7: pause guards asset movement on the vault. Requests and updates
/// move only the owner's own shares and keep working; so does an update in
/// drain mode, which is how a draining owner lowers a floor to exit.
#[test]
fn pause_and_drain_do_not_block_the_owner() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.pause().expect("pause");
    ctx.request_withdrawal(1, shares / 2, 0)
        .expect("request while paused");
    ctx.update_request(1, 1, Some(5), None, None)
        .expect("update while paused");
    ctx.unpause().expect("unpause");

    ctx.set_accepting_requests(false).expect("drain");
    ctx.update_request(1, 1, Some(6), None, None)
        .expect("update in drain mode");
    assert_eq!(
        ctx.request_state_data(&ctx.user.pubkey(), 1).min_assets_out,
        6
    );
}

/// Design decision 5: a deposit-mint account, and neither program's.
#[test]
fn the_recipient_must_hold_the_deposit_mint_and_belong_to_neither_program() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.insecure_clone();
    let share_account = ctx.user_share_ata;
    let quarter = shares / 4;

    // A deposit-mint account whose authority is the queue PDA but which is not
    // the escrow: anyone can create one, and only the authority check sees it.
    let (queue_pda, deposit_mint) = (ctx.withdrawal_queue_pda(), ctx.deposit_mint);
    let queue_owned = ctx.create_token_account_for(&queue_pda, &deposit_mint);
    let bad = [
        ("a share-mint account", ctx.user_share_ata),
        (
            "the queue's asset escrow",
            ctx.queue_escrow(&ctx.deposit_mint),
        ),
        ("another account the queue controls", queue_owned),
        ("the vault's reserve", ctx.vault_token_pda),
    ];
    for (i, (what, recipient)) in bad.into_iter().enumerate() {
        let err = ctx
            .request_withdrawal_as(
                &user,
                share_account,
                recipient,
                i as u64,
                quarter,
                0,
                Pubkey::default(),
            )
            .expect_err(what);
        assert_queue_err(&err, ErrorCode::InvalidRecipient);
        assert_anchor_framework_err(&err, 6009);
    }
    assert_eq!(ctx.queue_state_data().pending_requests, 0);

    // Any other deposit-mint account is fine, whoever owns it.
    let elsewhere = ctx.fee_recipient_deposit_ata;
    ctx.request_withdrawal_as(
        &user,
        share_account,
        elsewhere,
        9,
        quarter,
        0,
        Pubkey::default(),
    )
    .expect("a third party's deposit-mint account is a valid recipient");
    assert_eq!(
        ctx.request_state_data(&user.pubkey(), 9)
            .recipient_token_account,
        elsewhere
    );
}

/// The invariant the counters exist for: `pending_shares` equals the escrow's
/// balance and `pending_requests` the number of live requests, after every
/// request. A randomized walk with cancel and finalize belongs to WQ-10.
#[test]
fn counters_track_the_escrow_across_many_requests() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    let mut total = 0u64;
    let amounts = [7u64, 1, 300, 42, 9_999, shares / 10];
    for (i, amount) in amounts.iter().enumerate() {
        ctx.request_withdrawal(i as u64 + 10, *amount, 0)
            .expect("request");
        total += amount;
        let q = ctx.queue_state_data();
        assert_eq!(q.pending_requests, i as u64 + 1);
        assert_eq!(q.pending_shares, total);
        assert_eq!(
            escrow_shares_balance(&ctx),
            total,
            "counters equal the escrow balance"
        );
        assert_eq!(
            ctx.request_state_data(&user, i as u64 + 10).sequence,
            i as u64 + 1
        );
    }
}

// ---- update_request ----

#[test]
fn the_owner_updates_only_what_they_ask_for() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    ctx.request_withdrawal(1, shares / 2, 1_000)
        .expect("request");
    let before = ctx.request_state_data(&user, 1);
    let ops = Pubkey::new_unique();
    let elsewhere = ctx.fee_recipient_deposit_ata;

    let meta = ctx
        .update_request(1, 1, Some(2_000), None, None)
        .expect("floor only");
    let r = ctx.request_state_data(&user, 1);
    assert_eq!(r.min_assets_out, 2_000);
    assert_eq!(r.recipient_token_account, before.recipient_token_account);
    assert_eq!(r.finalizer, before.finalizer);
    assert_eq!(
        (r.shares, r.eligible_at, r.expires_at, r.sequence),
        (
            before.shares,
            before.eligible_at,
            before.expires_at,
            before.sequence
        )
    );
    assert_full_update_event(&meta, &ctx, &user, 1);

    let meta = ctx
        .update_request(1, 1, None, Some(elsewhere), Some(ops))
        .expect("recipient and finalizer");
    let r = ctx.request_state_data(&user, 1);
    assert_eq!(r.recipient_token_account, elsewhere);
    assert_eq!(r.allowed_finalizer(), Some(ops));
    assert_eq!(r.min_assets_out, 2_000, "untouched by the second update");
    assert_full_update_event(&meta, &ctx, &user, 1);

    ctx.update_request(1, 1, None, None, Some(Pubkey::default()))
        .expect("zero lifts the finalizer restriction");
    assert_eq!(ctx.request_state_data(&user, 1).allowed_finalizer(), None);
}

/// A call that changes nothing is refused rather than confirmed. The generated
/// client sends an omitted account as the program id, so a client that meant to
/// change the recipient and dropped the account would otherwise read a success.
#[test]
fn an_update_that_changes_nothing_is_refused() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    ctx.request_withdrawal(1, shares / 2, 1_000)
        .expect("request");
    let before = ctx
        .svm
        .get_account(&ctx.request_pda(&user, 1))
        .expect("request")
        .data;

    let err = ctx
        .update_request(1, 1, None, None, None)
        .expect_err("nothing to update");
    assert_queue_err(&err, ErrorCode::NothingToUpdate);
    assert_anchor_framework_err(&err, 6013);
    assert_eq!(
        ctx.svm
            .get_account(&ctx.request_pda(&user, 1))
            .expect("request")
            .data,
        before
    );

    // Re-stating the current value is a change in intent, and allowed.
    ctx.update_request(1, 1, Some(1_000), None, None)
        .expect("same floor, stated");
}

/// Decision 11: an id may be reused once its account closes, so every
/// instruction targeting a request quotes the stamp it was signed against.
#[test]
fn an_update_needs_the_requests_current_sequence() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
    let err = ctx
        .update_request(1, 2, Some(5), None, None)
        .expect_err("wrong sequence");
    assert_queue_err(&err, ErrorCode::StaleRequestSequence);
    assert_anchor_framework_err(&err, 6012);
    assert_eq!(
        ctx.request_state_data(&ctx.user.pubkey(), 1).min_assets_out,
        0
    );
}

#[test]
fn only_the_owner_updates_a_request() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    let user = ctx.user.pubkey();
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
    let impostor = ctx.new_funded_keypair(1_000_000_000);

    let err = ctx
        .update_request_as(&impostor, &user, 1, 1, Some(5), None, None)
        .expect_err("not the owner");
    assert_queue_err(&err, ErrorCode::NotRequestOwner);
    assert_anchor_framework_err(&err, 6010);
}

/// After the fulfillment window an update is refused: the request can only be
/// cancelled. The deadline instant itself counts as expired.
#[test]
fn an_expired_request_cannot_be_updated() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.set_fulfillment_window(DAY).expect("window");
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
    let r = ctx.request_state_data(&ctx.user.pubkey(), 1);

    ctx.warp_forward_seconds(r.expires_at - ctx.now() - 1);
    ctx.update_request(1, 1, Some(5), None, None)
        .expect("one second before the deadline");

    ctx.warp_forward_seconds(1);
    let err = ctx
        .update_request(1, 1, Some(6), None, None)
        .expect_err("at the deadline");
    assert_queue_err(&err, ErrorCode::RequestExpired);
    assert_anchor_framework_err(&err, 6011);
    assert_eq!(
        ctx.request_state_data(&ctx.user.pubkey(), 1).min_assets_out,
        5
    );
}

#[test]
fn a_new_recipient_follows_the_same_rule() {
    let (mut ctx, shares) = open_vault_with_holder(DAY);
    ctx.request_withdrawal(1, shares / 2, 0).expect("request");
    let (queue_pda, deposit_mint) = (ctx.withdrawal_queue_pda(), ctx.deposit_mint);
    let queue_owned = ctx.create_token_account_for(&queue_pda, &deposit_mint);
    for bad in [
        ctx.queue_escrow(&ctx.deposit_mint),
        queue_owned,
        ctx.vault_token_pda,
        ctx.user_share_ata,
    ] {
        let err = ctx
            .update_request(1, 1, None, Some(bad), None)
            .expect_err("bad recipient");
        assert_queue_err(&err, ErrorCode::InvalidRecipient);
    }
    assert_eq!(
        ctx.request_state_data(&ctx.user.pubkey(), 1)
            .recipient_token_account,
        ctx.user_deposit_ata
    );
}
