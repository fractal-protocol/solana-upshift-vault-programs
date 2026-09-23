// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `finalize_withdrawals` is `finalize_withdrawal` over a set: three trailing
//! accounts per request, all or nothing, one event per request. The single
//! form's suite covers the payout itself; this one covers the batching.

use august_vault::errors::ErrorCode as VaultError;
use august_withdrawal_queue::batch::MAX_FINALIZE_BATCH;
use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::WithdrawalFinalized;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, events_of, sorted_by_request,
    with_batch_budget, Depositor, RequestGroup, VaultCtx, DEPOSIT_DECIMALS,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::{pubkey::Pubkey, signer::Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DAY: u64 = 24 * 60 * 60;
/// The per-transaction compute ceiling the design measures batches against.
const CU_CEILING: u64 = 1_400_000;
/// A legacy transaction's wire limit.
const PACKET_DATA_SIZE: usize = 1232;

fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

/// The last request announced before the failure is the offending one.
fn assert_blamed(err: &FailedTransactionMetadata, request: &Pubkey) {
    let logs = &err.meta.logs;
    let last = logs
        .iter()
        .rposition(|l| l == "Program log: request")
        .expect("a request was announced");
    assert_eq!(
        logs.get(last + 1).map(String::as_str),
        Some(format!("Program log: {request}").as_str()),
        "logs:
{}",
        logs.join(
            "
"
        )
    );
}

/// A vault with the built-in holder and `extra` more depositors, queue attached
/// under a one-day cooldown.
fn vault_with_holders(extra: usize) -> (VaultCtx, Vec<Depositor>) {
    let mut ctx = VaultCtx::fresh();
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(DAY);
    let others: Vec<Depositor> = (0..extra)
        .map(|_| {
            let d = ctx.new_depositor(DEPOSIT_AMOUNT);
            ctx.deposit_as(&d, DEPOSIT_AMOUNT).expect("deposit");
            d
        })
        .collect();
    (ctx, others)
}

/// Opens one request of `shares` for `owner` with `id`, from their share ATA
/// into their deposit ATA, and returns its group and sequence.
fn open(ctx: &mut VaultCtx, owner: &Depositor, id: u64, shares: u64) -> (RequestGroup, u64) {
    ctx.request_withdrawal_as(
        &owner.keypair,
        owner.share_ata,
        owner.deposit_ata,
        id,
        shares,
        Pubkey::default(),
    )
    .expect("request");
    let key = owner.keypair.pubkey();
    (
        ctx.request_group(&key, id),
        ctx.request_state_data(&key, id).sequence,
    )
}

/// Sequences in the order of `groups`.
fn sequences_of(ctx: &VaultCtx, groups: &[RequestGroup], ids: &[(Pubkey, u64)]) -> Vec<u64> {
    groups
        .iter()
        .map(|g| {
            let (owner, id) = ids
                .iter()
                .find(|(o, id)| ctx.request_pda(o, *id) == g.request)
                .expect("known request");
            ctx.request_state_data(owner, *id).sequence
        })
        .collect()
}

#[test]
fn a_batch_pays_every_request_and_closes_each_with_rent_to_its_owner() {
    let (mut ctx, others) = vault_with_holders(2);
    let mut ids = Vec::new();
    let mut groups = Vec::new();
    for (i, d) in others.iter().enumerate() {
        let shares = ctx.token_account_amount(&d.share_ata) / 2;
        let (g, _) = open(&mut ctx, d, i as u64 + 1, shares);
        ids.push((d.keypair.pubkey(), i as u64 + 1));
        groups.push(g);
    }
    let user_shares = ctx.token_account_amount(&ctx.user_share_ata) / 2;
    ctx.request_withdrawal(7, user_shares).expect("user");
    ids.push((ctx.user.pubkey(), 7));
    groups.push(ctx.request_group(&ctx.user.pubkey(), 7));
    let groups = sorted_by_request(groups);
    let sequences = sequences_of(&ctx, &groups, &ids);
    ctx.warp_forward_seconds(DAY as i64);
    let keeper = ctx.new_funded_keypair(1_000_000_000);
    let rents: Vec<u64> = groups
        .iter()
        .map(|g| ctx.svm.get_account(&g.request).expect("request").lamports)
        .collect();
    let owner_lamports: Vec<u64> = groups
        .iter()
        .map(|g| ctx.svm.get_balance(&g.owner).expect("owner"))
        .collect();
    let paid_before: Vec<u64> = groups
        .iter()
        .map(|g| ctx.token_account_amount(&g.recipient))
        .collect();

    let meta = ctx
        .finalize_withdrawals_as(&keeper, &groups, &sequences)
        .expect("batch of three");
    eprintln!(
        "finalize_withdrawals(3) consumed {} CU",
        meta.compute_units_consumed
    );

    let events = events_of::<WithdrawalFinalized>(&meta);
    assert_eq!(events.len(), 3, "one event per request, in order");
    for (i, g) in groups.iter().enumerate() {
        assert_eq!(events[i].request, g.request);
        assert_eq!(
            ctx.token_account_amount(&g.recipient) - paid_before[i],
            events[i].assets,
            "each recipient got what its event says"
        );
        assert!(events[i].assets > 0);
        assert!(ctx.svm.get_account(&g.request).is_none(), "closed");
        assert_eq!(
            ctx.svm.get_balance(&g.owner).expect("owner") - owner_lamports[i],
            rents[i],
            "rent back to the owner"
        );
    }
    let q = ctx.queue_state_data();
    assert_eq!((q.pending_requests, q.pending_shares), (0, 0));
    assert_eq!(
        ctx.token_account_amount(&ctx.queue_escrow(&ctx.deposit_mint)),
        0
    );
}

/// Decision 16: a liquidity shortfall on one request reverts all of them.
#[test]
fn a_shortfall_on_one_request_reverts_the_whole_batch() {
    let (mut ctx, others) = vault_with_holders(1);
    let d = &others[0];
    let shares = ctx.token_account_amount(&d.share_ata) / 2;
    let (g1, _) = open(&mut ctx, d, 1, shares);
    let (g2, _) = open(&mut ctx, d, 2, shares);
    let ids = [(d.keypair.pubkey(), 1), (d.keypair.pubkey(), 2)];
    let groups = sorted_by_request(vec![g1, g2]);
    let sequences = sequences_of(&ctx, &groups, &ids);
    ctx.warp_forward_seconds(DAY as i64);
    // Leave the reserve able to pay one request and a half: the second in the
    // batch runs out.
    let (gross, _) = ctx.quote_redeem(shares);
    let local = ctx.vault_state_data().local_aum;
    ctx.operator_withdraw(local - gross * 3 / 2)
        .expect("deploy");
    let keeper = ctx.new_funded_keypair(1_000_000_000);
    let unpaid = groups[1].request;
    let paid_before = ctx.token_account_amount(&d.deposit_ata);

    let err = ctx
        .finalize_withdrawals_as(&keeper, &groups, &sequences)
        .expect_err("the second request runs out of liquidity");
    assert_anchor_err(&err, VaultError::NotEnoughLiquidity);
    assert_blamed(&err, &unpaid);
    assert_eq!(ctx.token_account_amount(&d.deposit_ata), paid_before);
    for g in &groups {
        assert!(ctx.svm.get_account(&g.request).is_some(), "still pending");
    }
    assert_eq!(ctx.queue_state_data().pending_requests, 2);
}

/// Permission is per request: one request restricted to its owner keeps the
/// keeper from finalizing the batch it is in.
#[test]
fn a_restricted_request_keeps_the_keeper_out_of_its_batch() {
    let (mut ctx, others) = vault_with_holders(1);
    let d = &others[0];
    let shares = ctx.token_account_amount(&d.share_ata) / 2;
    let (g1, _) = open(&mut ctx, d, 1, shares);
    ctx.request_withdrawal_as(
        &d.keypair,
        d.share_ata,
        d.deposit_ata,
        2,
        shares,
        d.keypair.pubkey(),
    )
    .expect("owner-only");
    let g2 = ctx.request_group(&d.keypair.pubkey(), 2);
    let ids = [(d.keypair.pubkey(), 1), (d.keypair.pubkey(), 2)];
    let groups = sorted_by_request(vec![g1, g2]);
    let sequences = sequences_of(&ctx, &groups, &ids);
    ctx.warp_forward_seconds(DAY as i64);
    let keeper = ctx.new_funded_keypair(1_000_000_000);

    let err = ctx
        .finalize_withdrawals_as(&keeper, &groups, &sequences)
        .expect_err("the keeper may not touch request 2");
    assert_queue_err(&err, ErrorCode::FinalizerNotAllowed);
    assert_eq!(ctx.queue_state_data().pending_requests, 2);

    ctx.finalize_withdrawals_as(&d.keypair, &groups, &sequences)
        .expect("the owner may finalize both");
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
}

/// Each group's owner and recipient are bound to the request, exactly as the
/// single form binds them, and every trailing account must be writable.
#[test]
fn malformed_batches_and_substituted_groups_are_refused() {
    let (mut ctx, others) = vault_with_holders(2);
    let (a, b) = (&others[0], &others[1]);
    let shares = ctx.token_account_amount(&a.share_ata) / 2;
    let (ga, _) = open(&mut ctx, a, 1, shares);
    let (gb, _) = open(&mut ctx, b, 1, shares);
    let ids = [(a.keypair.pubkey(), 1), (b.keypair.pubkey(), 1)];
    let groups = sorted_by_request(vec![ga, gb]);
    let sequences = sequences_of(&ctx, &groups, &ids);
    ctx.warp_forward_seconds(DAY as i64);
    let keeper = ctx.new_funded_keypair(1_000_000_000);

    let err = ctx
        .finalize_withdrawals_as(&keeper, &[], &[])
        .expect_err("empty");
    assert_queue_err(&err, ErrorCode::EmptyBatch);
    let err = ctx
        .finalize_withdrawals_as(&keeper, &groups, &sequences[..1])
        .expect_err("length mismatch");
    assert_queue_err(&err, ErrorCode::BatchLengthMismatch);
    let reversed: Vec<RequestGroup> = groups.iter().rev().copied().collect();
    let rev_seqs: Vec<u64> = sequences.iter().rev().copied().collect();
    let err = ctx
        .finalize_withdrawals_as(&keeper, &reversed, &rev_seqs)
        .expect_err("descending");
    assert_queue_err(&err, ErrorCode::RequestsNotSorted);
    let err = ctx
        .finalize_withdrawals_as(
            &keeper,
            &[groups[0], groups[0]],
            &[sequences[0], sequences[0]],
        )
        .expect_err("duplicated");
    assert_queue_err(&err, ErrorCode::RequestsNotSorted);

    // Owner slot pointed at the keeper: the rent would go to the keeper.
    let mut swapped = groups.clone();
    swapped[0].owner = keeper.pubkey();
    let err = ctx
        .finalize_withdrawals_as(&keeper, &swapped, &sequences)
        .expect_err("owner substituted");
    assert_anchor_framework_err(&err, 2001);
    // Recipient slot pointed at the other holder's account.
    let mut swapped = groups.clone();
    swapped[0].recipient = groups[1].recipient;
    let err = ctx
        .finalize_withdrawals_as(&keeper, &swapped, &sequences)
        .expect_err("recipient substituted");
    assert_anchor_framework_err(&err, 2001);
    // A read-only owner cannot receive rent.
    let mut ix = ctx.finalize_withdrawals_ix(&keeper.pubkey(), &groups, &sequences);
    let n = ix.accounts.len();
    ix.accounts[n - 2].is_writable = false;
    let tx = solana_sdk::transaction::Transaction::new_signed_with_payer(
        &[ix],
        Some(&keeper.pubkey()),
        &[&keeper],
        ctx.svm.latest_blockhash(),
    );
    let err = ctx.svm.send_transaction(tx).expect_err("read-only owner");
    assert_anchor_framework_err(&err, 2000);
    // Another queue's request, genuine in every respect but its queue, with
    // its own owner and recipient in the group.
    let other = ctx.new_vault_with_queue();
    let user = ctx.user.pubkey();
    let user_ata = ctx.user_deposit_ata;
    let foreign = RequestGroup {
        request: ctx.install_foreign_request(&other, &user, 1, 1, user_ata),
        owner: user,
        recipient: user_ata,
    };
    let err = ctx
        .finalize_withdrawals_as(&keeper, &[foreign], &[1])
        .expect_err("another queue's request");
    assert_anchor_framework_err(&err, 2001);
    // One over the bound is refused before the accounts are looked at.
    let err = ctx
        .finalize_withdrawals_as(
            &keeper,
            &groups[..1],
            &vec![sequences[0]; MAX_FINALIZE_BATCH + 1],
        )
        .expect_err("over the bound");
    assert_queue_err(&err, ErrorCode::BatchTooLarge);

    assert_eq!(ctx.queue_state_data().pending_requests, 2);
    ctx.finalize_withdrawals_as(&keeper, &groups, &sequences)
        .expect("the genuine batch still works");
}

/// The bound fits a legacy transaction with distinct owners and recipients,
/// which is the most accounts a batch of that size can take, and the compute
/// it needs is well under the ceiling.
#[test]
fn the_bound_fits_a_legacy_transaction_and_the_cu_ceiling() {
    let (mut ctx, others) = vault_with_holders(MAX_FINALIZE_BATCH);
    let mut ids = Vec::new();
    let mut groups = Vec::new();
    for (i, d) in others.iter().enumerate() {
        let shares = ctx.token_account_amount(&d.share_ata) / 2;
        let (g, _) = open(&mut ctx, d, i as u64 + 1, shares);
        ids.push((d.keypair.pubkey(), i as u64 + 1));
        groups.push(g);
    }
    let groups = sorted_by_request(groups);
    let sequences = sequences_of(&ctx, &groups, &ids);
    ctx.warp_forward_seconds(DAY as i64);
    let keeper = ctx.new_funded_keypair(1_000_000_000);

    let ix = ctx.finalize_withdrawals_ix(&keeper.pubkey(), &groups, &sequences);
    let tx = solana_sdk::transaction::Transaction::new_signed_with_payer(
        &with_batch_budget(ix),
        Some(&keeper.pubkey()),
        &[&keeper],
        solana_sdk::hash::Hash::default(),
    );
    let size = 1 + 64 * tx.signatures.len() + tx.message_data().len();
    assert!(size <= PACKET_DATA_SIZE, "{size} bytes");

    let meta = ctx
        .finalize_withdrawals_as(&keeper, &groups, &sequences)
        .expect("a full batch in one transaction");
    eprintln!(
        "finalize_withdrawals({MAX_FINALIZE_BATCH}) consumed {} CU, {} per request; {size} bytes",
        meta.compute_units_consumed,
        meta.compute_units_consumed / MAX_FINALIZE_BATCH as u64,
    );
    assert!(meta.compute_units_consumed < CU_CEILING);
    assert_eq!(
        events_of::<WithdrawalFinalized>(&meta).len(),
        MAX_FINALIZE_BATCH
    );
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
}

/// The bound is the heap's: one owner's requests share their owner and
/// recipient slots, so more of them fit the packet than the program accepts,
/// and one over the bound is refused by code rather than by an allocation
/// failure. Every event arrives, since events are self-CPIs, not log lines.
#[test]
fn a_full_batch_of_one_owner_goes_through_and_one_over_the_bound_is_refused() {
    let (mut ctx, _) = vault_with_holders(0);
    let user = ctx.user.pubkey();
    let shares = ctx.token_account_amount(&ctx.user_share_ata);
    let n = MAX_FINALIZE_BATCH as u64 + 1;
    let mut ids = Vec::new();
    let mut groups = Vec::new();
    for id in 1..=n {
        ctx.request_withdrawal(id, shares / (2 * n))
            .expect("request");
        ids.push((user, id));
        groups.push(ctx.request_group(&user, id));
    }
    let groups = sorted_by_request(groups);
    let sequences = sequences_of(&ctx, &groups, &ids);
    ctx.warp_forward_seconds(DAY as i64);
    let keeper = ctx.new_funded_keypair(1_000_000_000);
    let paid_before = ctx.token_account_amount(&ctx.user_deposit_ata);

    let err = ctx
        .finalize_withdrawals_as(&keeper, &groups, &sequences)
        .expect_err("one over the bound");
    assert_queue_err(&err, ErrorCode::BatchTooLarge);
    assert_eq!(ctx.queue_state_data().pending_requests, n);

    let (batch, rest) = groups.split_at(MAX_FINALIZE_BATCH);
    let meta = ctx
        .finalize_withdrawals_as(&keeper, batch, &sequences[..MAX_FINALIZE_BATCH])
        .expect("a full batch");
    let announced = meta
        .logs
        .iter()
        .filter(|l| *l == "Program log: request")
        .count();
    eprintln!(
        "finalize_withdrawals({MAX_FINALIZE_BATCH}) consumed {} CU; {announced} announcements \
         in the log{}",
        meta.compute_units_consumed,
        if meta.logs.iter().any(|l| l == "Log truncated") {
            " (truncated)"
        } else {
            ""
        }
    );
    let events = events_of::<WithdrawalFinalized>(&meta);
    assert_eq!(events.len(), MAX_FINALIZE_BATCH, "every event arrives");
    let paid: u64 = events.iter().map(|e| e.assets).sum();
    assert_eq!(
        ctx.token_account_amount(&ctx.user_deposit_ata) - paid_before,
        paid
    );
    for g in batch {
        assert!(ctx.svm.get_account(&g.request).is_none(), "closed");
    }
    assert!(ctx.svm.get_account(&rest[0].request).is_some(), "left out");
    assert_eq!(ctx.queue_state_data().pending_requests, 1);
}
