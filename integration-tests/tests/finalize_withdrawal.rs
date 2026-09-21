// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! `finalize_withdrawal` redeems a mature request's escrowed shares through the
//! vault and pays the recipient. The caller chooses only the moment: what is
//! paid, and where, comes from the request, and a vault refusal leaves the
//! request exactly as it was.

use august_vault::errors::ErrorCode as VaultError;
use august_vault::instructions::redeem::WithdrawEvt;
use august_vault::state::vault::VAULT_STATE_SEED;
use august_withdrawal_queue::accounts::FinalizeWithdrawal as Finalize;
use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::WithdrawalFinalized;
use integration_tests::harness::{
    assert_anchor_err, assert_anchor_framework_err, events_of, VaultCtx, DEPOSIT_DECIMALS,
    VAULT_VERSION,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer};

const DEPOSIT_AMOUNT: u64 = 10 * 10u64.pow(DEPOSIT_DECIMALS as u32);
const DAY: u64 = 24 * 60 * 60;
/// 1% of the vault's 1_000_000 fee denominator.
const ONE_PERCENT: u32 = 10_000;
/// Anchor's `AccountNotInitialized`, which is what a closed request reads as.
const ACCOUNT_NOT_INITIALIZED: u32 = 3012;

fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

/// Every slot a refused finalize must leave alone: the request's bytes and
/// rent, every balance the payout touches, the share supply, the vault's
/// reserve figure and the queue's counters. Not the owner's lamports: an owner
/// who signs a refused call still pays the transaction fee.
#[derive(Debug, PartialEq, Eq)]
struct Untouched {
    request: Option<(Vec<u8>, u64)>,
    escrow_shares: u64,
    escrow_assets: u64,
    /// `None` when the recipient account does not exist.
    recipient: Option<u64>,
    fee_recipient: u64,
    share_supply: u64,
    local_aum: u64,
    counters: (u64, u64),
}

fn untouched(ctx: &VaultCtx, owner: &Pubkey, id: u64) -> Untouched {
    let request = ctx
        .svm
        .get_account(&ctx.request_pda(owner, id))
        .filter(|a| a.lamports > 0)
        .map(|a| (a.data, a.lamports));
    let recipient = request
        .as_ref()
        .map(|_| ctx.request_state_data(owner, id).recipient_token_account)
        .unwrap_or(ctx.user_deposit_ata);
    let q = ctx.queue_state_data();
    Untouched {
        request,
        escrow_shares: ctx.token_account_amount(&ctx.queue_escrow(&ctx.share_mint)),
        escrow_assets: ctx.token_account_amount(&ctx.queue_escrow(&ctx.deposit_mint)),
        recipient: ctx
            .svm
            .get_account(&recipient)
            .filter(|a| a.lamports > 0)
            .map(|_| ctx.token_account_amount(&recipient)),
        fee_recipient: ctx.token_account_amount(&ctx.fee_recipient_deposit_ata),
        share_supply: ctx.share_mint_supply(),
        local_aum: ctx.vault_state_data().local_aum,
        counters: (q.pending_requests, q.pending_shares),
    }
}

/// A vault with a holder and its queue open under `cooldown`; the holder has
/// requested half their shares as request 1 with floor `min`. Returns the
/// context and the shares in that request.
fn holder_with_request(ctx: VaultCtx, cooldown: u64, min: u64) -> (VaultCtx, u64) {
    let mut ctx = ctx;
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(cooldown);
    let half = ctx.token_account_amount(&ctx.user_share_ata) / 2;
    ctx.request_withdrawal(1, half, min).expect("request");
    (ctx, half)
}

/// As above, on classic SPL, with the cooldown already run.
fn mature_request(min: u64) -> (VaultCtx, u64) {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh(), DAY, min);
    ctx.warp_forward_seconds(DAY as i64);
    (ctx, half)
}

fn keeper(ctx: &mut VaultCtx) -> Keypair {
    ctx.new_funded_keypair(1_000_000_000)
}

fn assert_full_finalized_event(
    meta: &litesvm::types::TransactionMetadata,
    ctx: &VaultCtx,
    owner: &Pubkey,
    id: u64,
    sequence: u64,
    finalizer: &Pubkey,
    recipient: &Pubkey,
    shares: u64,
    assets: u64,
) {
    let events = events_of::<WithdrawalFinalized>(meta);
    assert_eq!(events.len(), 1, "exactly one WithdrawalFinalized");
    let e = &events[0];
    assert_eq!(e.vault, ctx.vault_state);
    assert_eq!(e.queue, ctx.withdrawal_queue_pda());
    assert_eq!(e.request, ctx.request_pda(owner, id));
    assert_eq!((e.request_id, e.owner, e.sequence), (id, *owner, sequence));
    assert_eq!((e.recipient, e.finalizer), (*recipient, *finalizer));
    assert_eq!((e.shares, e.assets), (shares, assets));
}

// ---- the payout ----

/// The whole contract in one transaction: anyone finalizes, the recipient gets
/// exactly the vault's net quote, the fee goes where the vault sends it, the
/// escrow keeps nothing, and the request closes with its rent to the owner.
#[test]
fn anyone_finalizes_a_mature_request_and_the_recipient_gets_the_vaults_net_payout() {
    let (mut ctx, half) = mature_request(0);
    ctx.set_withdrawal_fee(ONE_PERCENT).expect("fee");
    let user = ctx.user.pubkey();
    let keeper = keeper(&mut ctx);
    let (gross, net) = ctx.quote_redeem(half);
    assert!(gross > net && net > 0, "the fee is in play");
    let before = untouched(&ctx, &user, 1);
    let request_rent = before.request.as_ref().expect("request exists").1;
    let owner_before = ctx.svm.get_balance(&user).expect("owner");
    let keeper_before = ctx.svm.get_balance(&keeper.pubkey()).expect("keeper");

    let meta = ctx
        .finalize_withdrawal_as(&keeper, &user, 1, 1)
        .expect("finalize");
    eprintln!(
        "finalize_withdrawal consumed {} CU",
        meta.compute_units_consumed
    );
    assert!(
        meta.compute_units_consumed < 200_000,
        "the design caps finalize at 200k CU; measured {}",
        meta.compute_units_consumed
    );

    let after = untouched(&ctx, &user, 1);
    let paid = |s: &Untouched| s.recipient.expect("recipient exists");
    assert_eq!(paid(&after) - paid(&before), net, "paid the net quote");
    assert_eq!(
        after.fee_recipient - before.fee_recipient,
        gross - net,
        "the vault took its fee"
    );
    assert_eq!(after.escrow_assets, 0, "the asset escrow is transit only");
    assert_eq!(before.escrow_shares - after.escrow_shares, half);
    assert_eq!(
        before.share_supply - after.share_supply,
        half,
        "shares burned"
    );
    assert_eq!(before.local_aum - after.local_aum, gross);
    assert_eq!(after.request, None, "the request account is closed");
    assert_eq!(
        ctx.svm.get_balance(&user).expect("owner") - owner_before,
        request_rent,
        "rent returns to the owner, who did not sign"
    );
    assert!(
        ctx.svm.get_balance(&keeper.pubkey()).expect("keeper") < keeper_before,
        "the finalizer paid the fee and got nothing else"
    );
    assert_eq!(after.counters, (0, 0));
    assert_full_finalized_event(
        &meta,
        &ctx,
        &user,
        1,
        1,
        &keeper.pubkey(),
        &ctx.user_deposit_ata,
        half,
        net,
    );

    // Decision 9: the vault's event names the queue PDA in every identity slot
    // and is attributed through the enclosing finalize.
    let vault_events = events_of::<WithdrawEvt>(&meta);
    assert_eq!(vault_events.len(), 1);
    let v = &vault_events[0];
    let queue = ctx.withdrawal_queue_pda();
    assert_eq!((v.caller, v.receiver, v.owner), (queue, queue, queue));
    assert_eq!((v.shares, v.assets), (half, net));
}

/// Pricing happens at finalization: a fee set after the request is charged on
/// it, and the recipient's floor is the only protection against that.
#[test]
fn the_payout_is_priced_when_finalized_not_when_requested() {
    let (mut ctx, half) = mature_request(0);
    let (_, quoted_at_request) = ctx.quote_redeem(half);
    ctx.set_withdrawal_fee(ONE_PERCENT).expect("fee");
    let (_, quoted_now) = ctx.quote_redeem(half);
    assert!(quoted_now < quoted_at_request);

    let before = ctx.token_account_amount(&ctx.user_deposit_ata);
    ctx.finalize_withdrawal(1, 1).expect("finalize");
    assert_eq!(
        ctx.token_account_amount(&ctx.user_deposit_ata) - before,
        quoted_now
    );
}

/// The same path under Token-2022: the CPI, the payout transfer and the close
/// all go through the interface.
#[test]
fn a_token_2022_request_finalizes_too() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh_token_2022(), DAY, 0);
    ctx.warp_forward_seconds(DAY as i64);
    let user = ctx.user.pubkey();
    let (_, net) = ctx.quote_redeem(half);
    let before = untouched(&ctx, &user, 1);

    let meta = ctx
        .finalize_withdrawal(1, 1)
        .expect("finalize under Token-2022");

    let after = untouched(&ctx, &user, 1);
    assert_eq!(
        after.recipient.expect("recipient") - before.recipient.expect("recipient"),
        net
    );
    assert_eq!(after.request, None);
    assert_eq!(after.counters, (0, 0));
    assert_eq!(events_of::<WithdrawalFinalized>(&meta)[0].assets, net);
}

/// A balance someone sent to the asset escrow is not the vault's payout and
/// stays where it is; the recipient gets the delta only.
#[test]
fn a_donation_to_the_asset_escrow_is_not_paid_out() {
    let (mut ctx, half) = mature_request(0);
    let escrow = ctx.queue_escrow(&ctx.deposit_mint);
    ctx.mint_deposit_to(&escrow, 1_000_000);
    let (_, net) = ctx.quote_redeem(half);
    let before = ctx.token_account_amount(&ctx.user_deposit_ata);

    ctx.finalize_withdrawal(1, 1).expect("finalize");

    assert_eq!(
        ctx.token_account_amount(&ctx.user_deposit_ata) - before,
        net
    );
    assert_eq!(
        ctx.token_account_amount(&escrow),
        1_000_000,
        "the donation is still there"
    );
}

#[test]
fn finalizing_one_request_leaves_the_others_untouched() {
    let (mut ctx, half) = mature_request(0);
    let user = ctx.user.pubkey();
    let quarter = half / 2;
    ctx.request_withdrawal(2, quarter, 0).expect("second");
    ctx.request_withdrawal(3, quarter, 0).expect("third");
    ctx.warp_forward_seconds(DAY as i64);
    let second = ctx.request_pda(&user, 2);
    let second_before = ctx.svm.get_account(&second).expect("second").data;

    ctx.finalize_withdrawal(3, 3).expect("finalize the third");

    assert_eq!(
        ctx.svm.get_account(&second).expect("second").data,
        second_before
    );
    assert!(ctx.svm.get_account(&ctx.request_pda(&user, 1)).is_some());
    let q = ctx.queue_state_data();
    assert_eq!((q.pending_requests, q.pending_shares), (2, half + quarter));
    assert_eq!(
        ctx.token_account_amount(&ctx.queue_escrow(&ctx.share_mint)),
        half + quarter
    );
}

/// Finalize does not depend on the vault's gate: a released vault lets the queue
/// redeem like any holder, so a request pending across a release is never
/// stranded. Only `release_vault` (WQ-09) reaches this state, so the test
/// writes the vault directly.
#[test]
fn a_cleared_gate_does_not_block_finalization() {
    let (mut ctx, half) = mature_request(0);
    ctx.request_withdrawal(2, half / 2, 0).expect("second");
    ctx.warp_forward_seconds(DAY as i64);

    let mut vault = ctx.vault_state_data();
    vault.withdrawal_queue_authority = Pubkey::default();
    ctx.force_overwrite_vault_state(vault);
    ctx.finalize_withdrawal(1, 1)
        .expect("finalize with the gate cleared");
    ctx.finalize_withdrawal(2, 2).expect("and the next");
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
}

// ---- timing ----

#[test]
fn early_is_refused_until_the_exact_eligibility_instant() {
    let (mut ctx, _) = holder_with_request(VaultCtx::fresh(), DAY, 0);
    let user = ctx.user.pubkey();
    let eligible_at = ctx.request_state_data(&user, 1).eligible_at;
    ctx.warp_forward_seconds(eligible_at - ctx.now() - 1);
    let before = untouched(&ctx, &user, 1);

    let err = ctx.finalize_withdrawal(1, 1).expect_err("one second early");
    assert_queue_err(&err, ErrorCode::CooldownNotElapsed);
    assert_anchor_framework_err(&err, 6013);
    assert_eq!(untouched(&ctx, &user, 1), before, "nothing moved");

    ctx.warp_forward_seconds(1);
    ctx.finalize_withdrawal(1, 1)
        .expect("at the instant itself");
}

/// A zero cooldown is eligible in the same slot as the request.
#[test]
fn a_zero_cooldown_finalizes_immediately() {
    let (mut ctx, _) = holder_with_request(VaultCtx::fresh(), 0, 0);
    ctx.finalize_withdrawal(1, 1).expect("no wait");
}

/// After the window a request can only be cancelled; the deadline instant
/// counts as expired, and a disabled window never expires.
#[test]
fn an_expired_request_cannot_be_finalized() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh(), DAY, 0);
    let user = ctx.user.pubkey();
    ctx.set_fulfillment_window(DAY).expect("window");
    ctx.request_withdrawal(2, half / 4, 0).expect("windowed");
    ctx.request_withdrawal(3, half / 4, 0)
        .expect("windowed too");
    ctx.set_fulfillment_window(0).expect("no window");
    ctx.request_withdrawal(4, half / 4, 0).expect("unwindowed");
    let expires_at = ctx.request_state_data(&user, 2).expires_at;
    assert_eq!(ctx.request_state_data(&user, 4).expires_at, 0);

    ctx.warp_forward_seconds(expires_at - ctx.now() - 1);
    ctx.finalize_withdrawal(2, 2)
        .expect("one second before the deadline");

    ctx.warp_forward_seconds(1);
    let before = untouched(&ctx, &user, 3);
    let err = ctx.finalize_withdrawal(3, 3).expect_err("at the deadline");
    assert_queue_err(&err, ErrorCode::RequestExpired);
    assert_eq!(untouched(&ctx, &user, 3), before);

    ctx.warp_forward_seconds(400 * DAY as i64);
    ctx.finalize_withdrawal(4, 4)
        .expect("a disabled window never expires");
}

// ---- identity ----

#[test]
fn a_stale_sequence_is_refused() {
    let (mut ctx, _) = mature_request(0);
    let user = ctx.user.pubkey();
    let before = untouched(&ctx, &user, 1);
    let err = ctx.finalize_withdrawal(1, 2).expect_err("wrong stamp");
    assert_queue_err(&err, ErrorCode::StaleRequestSequence);
    assert_eq!(untouched(&ctx, &user, 1), before);
}

/// Once closed, the request account is gone: a second finalize, or an update,
/// finds nothing to act on, and the counters do not move again.
#[test]
fn a_finalized_request_cannot_be_finalized_or_updated_again() {
    let (mut ctx, _) = mature_request(0);
    let signer = ctx.user.insecure_clone();
    let user = signer.pubkey();
    // Built while the request exists: once it is closed there is nothing left
    // to read the owner and recipient off, which is rather the point.
    let again = ctx.finalize_withdrawal_accounts(&user, &user, 1);
    ctx.finalize_withdrawal(1, 1).expect("first");
    let after = untouched(&ctx, &user, 1);

    let err = ctx
        .send_finalize_withdrawal(&signer, again, 1)
        .expect_err("second");
    assert_anchor_framework_err(&err, ACCOUNT_NOT_INITIALIZED);
    let err = ctx
        .update_request(1, 1, Some(5), None, None)
        .expect_err("update a closed request");
    assert_anchor_framework_err(&err, ACCOUNT_NOT_INITIALIZED);
    assert_eq!(untouched(&ctx, &user, 1), after);

    // The id is free again, and the new request carries a fresh stamp.
    ctx.request_withdrawal(1, 1, 0).expect("reuse the id");
    assert_eq!(ctx.request_state_data(&user, 1).sequence, 2);
}

// ---- permission (decision 14) ----

/// The permission table from the design doc, one request per row, plus the
/// owner finalizing a restricted request. A refused caller is refused with
/// nothing moved; then a permitted one finalizes.
#[test]
fn finalization_permission_follows_the_table() {
    let (mut ctx, half) = mature_request(0);
    let user = ctx.user.insecure_clone();
    let (share_account, recipient) = (ctx.user_share_ata, ctx.user_deposit_ata);
    let f = keeper(&mut ctx);
    let stranger = keeper(&mut ctx);
    // Request 1 already exists with no finalizer; the other two name `f`.
    let rows: [(u64, Vec<&Keypair>, &Keypair); 3] = [
        (1, vec![], &stranger),      // anyone
        (2, vec![&stranger], &f),    // only F, and the owner
        (3, vec![&stranger], &user), // the owner, whatever is set
    ];
    for id in [2, 3] {
        ctx.request_withdrawal_as(&user, share_account, recipient, id, half / 4, 0, f.pubkey())
            .expect("request");
    }
    ctx.warp_forward_seconds(DAY as i64);

    for (id, refused, permitted) in rows.iter() {
        let before = untouched(&ctx, &user.pubkey(), *id);
        for caller in refused {
            let err = ctx
                .finalize_withdrawal_as(caller, &user.pubkey(), *id, *id)
                .unwrap_err();
            assert_queue_err(&err, ErrorCode::FinalizerNotAllowed);
            assert_anchor_framework_err(&err, 6014);
        }
        assert_eq!(untouched(&ctx, &user.pubkey(), *id), before, "row {id}");
        ctx.finalize_withdrawal_as(permitted, &user.pubkey(), *id, *id)
            .unwrap_or_else(|e| panic!("row {id} permitted caller: {:?}", e.err));
    }
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
}

// ---- vault refusals propagate and change nothing ----

/// The vault's errors come back unchanged, and a refused payout is a full
/// rollback: shares, supply, AUM, counters, request and rent all stay put.
#[test]
fn a_liquidity_shortfall_reverts_and_becomes_retryable_after_an_operator_deposit() {
    let (mut ctx, _) = mature_request(0);
    let user = ctx.user.pubkey();
    let reserve = ctx.token_account_amount(&ctx.vault_token_pda);
    ctx.operator_withdraw(reserve * 9 / 10).expect("deploy");
    let before = untouched(&ctx, &user, 1);

    let err = ctx.finalize_withdrawal(1, 1).expect_err("no liquidity");
    assert_anchor_err(&err, VaultError::NotEnoughLiquidity);
    assert_eq!(untouched(&ctx, &user, 1), before, "atomic rollback");

    ctx.operator_deposit(reserve * 9 / 10).expect("return");
    ctx.finalize_withdrawal(1, 1).expect("retry");
}

#[test]
fn a_floor_the_vault_cannot_meet_reverts_until_the_owner_lowers_it() {
    let (mut ctx, half) = mature_request(0);
    let user = ctx.user.pubkey();
    let (_, quote) = ctx.quote_redeem(half);
    ctx.update_request(1, 1, Some(quote), None, None)
        .expect("floor at today's quote");
    ctx.set_withdrawal_fee(ONE_PERCENT).expect("fee rises");
    let (_, lower_quote) = ctx.quote_redeem(half);
    let before = untouched(&ctx, &user, 1);

    let err = ctx.finalize_withdrawal(1, 1).expect_err("below the floor");
    assert_anchor_err(&err, VaultError::SlippageExceeded);
    assert_eq!(untouched(&ctx, &user, 1), before);

    ctx.update_request(1, 1, Some(lower_quote), None, None)
        .expect("owner lowers the floor");
    let paid_before = ctx.token_account_amount(&ctx.user_deposit_ata);
    ctx.finalize_withdrawal(1, 1).expect("now it clears");
    assert_eq!(
        ctx.token_account_amount(&ctx.user_deposit_ata) - paid_before,
        lower_quote
    );
}

/// Decision 7: pause guards the vault's assets, so it blocks finalize, through
/// the CPI, and nothing else.
#[test]
fn a_paused_vault_refuses_finalization_and_the_request_survives() {
    let (mut ctx, _) = mature_request(0);
    let user = ctx.user.pubkey();
    ctx.pause().expect("pause");
    let before = untouched(&ctx, &user, 1);

    let err = ctx.finalize_withdrawal(1, 1).expect_err("paused");
    assert_anchor_err(&err, VaultError::VaultPaused);
    assert_eq!(untouched(&ctx, &user, 1), before);

    ctx.unpause().expect("unpause");
    ctx.finalize_withdrawal(1, 1)
        .expect("finalize after unpause");
}

// ---- the recipient ----

/// Decision 5, at finalize. Classic SPL lets the owner reassign their ATA to
/// the queue PDA after requesting; paying it would strand the payout, so it is
/// re-checked. The owner recovers by pointing the request elsewhere.
#[test]
fn a_recipient_reassigned_to_the_queue_after_the_request_is_refused() {
    let (mut ctx, _) = mature_request(0);
    let user = ctx.user.insecure_clone();
    let (ata, queue_pda, deposit_mint) = (
        ctx.user_deposit_ata,
        ctx.withdrawal_queue_pda(),
        ctx.deposit_mint,
    );
    ctx.set_token_account_authority_as(&user, &ata, &queue_pda);
    let before = untouched(&ctx, &user.pubkey(), 1);

    let err = ctx
        .finalize_withdrawal(1, 1)
        .expect_err("queue-owned recipient");
    assert_queue_err(&err, ErrorCode::InvalidRecipient);
    assert_eq!(untouched(&ctx, &user.pubkey(), 1), before);

    let fresh = ctx.create_token_account_for(&user.pubkey(), &deposit_mint);
    ctx.update_request(1, 1, None, Some(fresh), None)
        .expect("re-point the request");
    ctx.finalize_withdrawal(1, 1).expect("paid elsewhere");
    assert!(ctx.token_account_amount(&fresh) > 0);
    assert_eq!(
        ctx.token_account_amount(&ata),
        0,
        "the reassigned account got nothing"
    );
}

/// A recipient closed after the request fails closed rather than paying into
/// whatever now sits at that address; recreating it makes the request payable.
#[test]
fn a_recipient_closed_after_the_request_fails_closed() {
    let (mut ctx, _) = mature_request(0);
    let user = ctx.user.insecure_clone();
    let (ata, deposit_mint) = (ctx.user_deposit_ata, ctx.deposit_mint);
    assert_eq!(ctx.token_account_amount(&ata), 0, "closable");
    ctx.close_token_account_as(&user, &ata, &user.pubkey());
    let before = untouched(&ctx, &user.pubkey(), 1);

    let err = ctx.finalize_withdrawal(1, 1).expect_err("closed recipient");
    assert_anchor_framework_err(&err, ACCOUNT_NOT_INITIALIZED);
    assert_eq!(untouched(&ctx, &user.pubkey(), 1), before);

    assert_eq!(ctx.create_ata_for(&user.pubkey(), &deposit_mint), ata);
    ctx.finalize_withdrawal(1, 1).expect("recreated");
}

// ---- account substitution ----

/// Every slot is bound: to the queue's stored keys, to the request's, or, for
/// the two the queue does not store, by the vault's own constraints inside the
/// CPI. A third-party finalizer in particular cannot point the payout at
/// themselves. Each substitution is refused with nothing moved.
#[test]
fn substituted_accounts_are_refused_and_a_finalizer_cannot_redirect_the_payout() {
    let (mut ctx, _) = mature_request(0);
    let user = ctx.user.pubkey();
    let keeper = keeper(&mut ctx);
    let keeper_key = keeper.pubkey();
    let (deposit_mint, share_mint) = (ctx.deposit_mint, ctx.share_mint);
    let keeper_assets = ctx.create_ata_for(&keeper.pubkey(), &deposit_mint);
    let keeper_shares = ctx.create_ata_for(&keeper.pubkey(), &share_mint);
    let mint_b = ctx.create_extra_deposit_mint();
    let authority = ctx.protocol_authority.insecure_clone();
    ctx.try_initialize_vault(&authority, mint_b, VAULT_VERSION)
        .expect("vault B");
    let vault_b = Pubkey::find_program_address(
        &[VAULT_STATE_SEED, mint_b.as_ref(), &[VAULT_VERSION]],
        &august_vault::ID,
    )
    .0;
    let genuine = ctx.finalize_withdrawal_accounts(&keeper.pubkey(), &user, 1);
    let before = untouched(&ctx, &user, 1);

    // (what, substitution, expected code)
    const HAS_ONE: u32 = 2001;
    let cases: Vec<(&str, Box<dyn Fn(&mut Finalize)>, u32)> = vec![
        (
            "the finalizer's own account as recipient",
            Box::new(move |a: &mut Finalize| a.recipient_token_account = keeper_assets),
            HAS_ONE,
        ),
        (
            "the finalizer's account as the asset escrow",
            Box::new(move |a: &mut Finalize| a.escrow_assets = keeper_assets),
            HAS_ONE,
        ),
        (
            "the finalizer's account as the share escrow",
            Box::new(move |a: &mut Finalize| a.escrow_shares = keeper_shares),
            HAS_ONE,
        ),
        (
            "the finalizer as the rent recipient",
            Box::new(move |a: &mut Finalize| a.owner = keeper_key),
            HAS_ONE,
        ),
        (
            "the deposit mint in the share-mint slot",
            Box::new(move |a: &mut Finalize| a.share_mint = deposit_mint),
            HAS_ONE,
        ),
        (
            "the share mint in the deposit-mint slot",
            Box::new(move |a: &mut Finalize| a.deposit_mint = share_mint),
            HAS_ONE,
        ),
        (
            "another vault",
            Box::new(move |a: &mut Finalize| a.vault_state = vault_b),
            ErrorCode::VaultMismatch as u32 + ANCHOR_USER_ERROR_OFFSET,
        ),
        // The vault binds these two itself: the reserve by seeds (2006), the
        // fee account by its authority (2015, ConstraintTokenOwner).
        (
            "the finalizer's account as the vault reserve",
            Box::new(move |a: &mut Finalize| a.vault_deposit_ata = keeper_assets),
            2006,
        ),
        (
            "the finalizer's account as the fee account",
            Box::new(move |a: &mut Finalize| a.fee_recipient_account = keeper_assets),
            2015,
        ),
    ];
    for (what, substitute, code) in cases {
        let mut accounts = ctx.finalize_withdrawal_accounts(&keeper_key, &user, 1);
        substitute(&mut accounts);
        let err = ctx
            .send_finalize_withdrawal(&keeper, accounts, 1)
            .err()
            .unwrap_or_else(|| panic!("{what} was accepted"));
        assert_anchor_framework_err(&err, code);
        assert_eq!(untouched(&ctx, &user, 1), before, "{what}");
    }
    assert_eq!(ctx.token_account_amount(&keeper_assets), 0);

    ctx.send_finalize_withdrawal(&keeper, genuine, 1)
        .expect("the genuine set still works");
}
