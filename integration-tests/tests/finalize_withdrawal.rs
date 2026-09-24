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
/// requested half their shares as request 1. Returns the context and the
/// shares in that request.
fn holder_with_request(ctx: VaultCtx, cooldown: u64) -> (VaultCtx, u64) {
    let mut ctx = ctx;
    ctx.mint_to_user(DEPOSIT_AMOUNT);
    ctx.deposit(DEPOSIT_AMOUNT).expect("deposit");
    ctx.open_queue(cooldown);
    let half = ctx.token_account_amount(&ctx.user_share_ata) / 2;
    ctx.request_withdrawal(1, half).expect("request");
    (ctx, half)
}

/// As above, on classic SPL, with the cooldown already run.
fn mature_request() -> (VaultCtx, u64) {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh(), DAY);
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
    let (mut ctx, half) = mature_request();
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
/// it.
#[test]
fn the_payout_is_priced_when_finalized_not_when_requested() {
    let (mut ctx, half) = mature_request();
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
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh_token_2022(), DAY);
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
    let (mut ctx, half) = mature_request();
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
    let (mut ctx, half) = mature_request();
    let user = ctx.user.pubkey();
    let quarter = half / 2;
    ctx.request_withdrawal(2, quarter).expect("second");
    ctx.request_withdrawal(3, quarter).expect("third");
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
    let (mut ctx, half) = mature_request();
    ctx.request_withdrawal(2, half / 2).expect("second");
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
    let (mut ctx, _) = holder_with_request(VaultCtx::fresh(), DAY);
    let user = ctx.user.pubkey();
    let eligible_at = ctx.request_state_data(&user, 1).eligible_at;
    ctx.warp_forward_seconds(eligible_at - ctx.now() - 1);
    let before = untouched(&ctx, &user, 1);

    let err = ctx.finalize_withdrawal(1, 1).expect_err("one second early");
    assert_queue_err(&err, ErrorCode::CooldownNotElapsed);
    assert_anchor_framework_err(&err, 6012);
    assert_eq!(untouched(&ctx, &user, 1), before, "nothing moved");

    ctx.warp_forward_seconds(1);
    ctx.finalize_withdrawal(1, 1)
        .expect("at the instant itself");
}

/// A zero cooldown is eligible in the same slot as the request.
#[test]
fn a_zero_cooldown_finalizes_immediately() {
    let (mut ctx, _) = holder_with_request(VaultCtx::fresh(), 0);
    ctx.finalize_withdrawal(1, 1).expect("no wait");
}

/// After the window a request can only be cancelled; the deadline instant
/// counts as expired, and a disabled window never expires.
#[test]
fn an_expired_request_cannot_be_finalized() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh(), DAY);
    let user = ctx.user.pubkey();
    ctx.set_fulfillment_window(DAY).expect("window");
    ctx.request_withdrawal(2, half / 4).expect("windowed");
    ctx.request_withdrawal(3, half / 4).expect("windowed too");
    ctx.set_fulfillment_window(0).expect("no window");
    ctx.request_withdrawal(4, half / 4).expect("unwindowed");
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
    let (mut ctx, _) = mature_request();
    let user = ctx.user.pubkey();
    let before = untouched(&ctx, &user, 1);
    let err = ctx.finalize_withdrawal(1, 2).expect_err("wrong stamp");
    assert_queue_err(&err, ErrorCode::StaleRequestSequence);
    assert_eq!(untouched(&ctx, &user, 1), before);
}

/// Once closed, the request account is gone: a second finalize finds nothing
/// to act on, and the counters do not move again.
#[test]
fn a_finalized_request_cannot_be_finalized_again() {
    let (mut ctx, _) = mature_request();
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
    assert_eq!(untouched(&ctx, &user, 1), after);

    // The id is free again, and the new request carries a fresh stamp.
    ctx.request_withdrawal(1, 1).expect("reuse the id");
    assert_eq!(ctx.request_state_data(&user, 1).sequence, 2);
}

// ---- permission (decision 14) ----

/// The permission table from the design doc, one request per row, plus the
/// owner finalizing a restricted request. A refused caller is refused with
/// nothing moved; then a permitted one finalizes.
#[test]
fn finalization_permission_follows_the_table() {
    let (mut ctx, half) = mature_request();
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
        ctx.request_withdrawal_as(&user, share_account, recipient, id, half / 4, f.pubkey())
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
            assert_anchor_framework_err(&err, 6013);
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
    let (mut ctx, _) = mature_request();
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

/// The holder is paid the shares' value when the request is finalized, so any
/// yield the vault earns during the cooldown is theirs.
#[test]
fn yield_earned_during_the_cooldown_is_paid_out() {
    let (mut ctx, half) = holder_with_request(VaultCtx::fresh(), DAY);
    let (_, quoted_at_request) = ctx.quote_redeem(half);
    let deployed = ctx.vault_state_data().local_aum / 4;
    ctx.operator_withdraw(deployed).expect("deploy");
    ctx.operator_update_aum(deployed + deployed / 1_000)
        .expect("a 0.1% gain, inside the increase limit");
    ctx.warp_forward_seconds(DAY as i64);
    let (_, quoted_now) = ctx.quote_redeem(half);
    assert!(quoted_now > quoted_at_request, "the share price rose");

    let before = ctx.token_account_amount(&ctx.user_deposit_ata);
    ctx.finalize_withdrawal(1, 1).expect("finalize");
    assert_eq!(
        ctx.token_account_amount(&ctx.user_deposit_ata) - before,
        quoted_now
    );
}

/// Decision 7: pause guards the vault's assets, so it blocks finalize, through
/// the CPI, and nothing else.
#[test]
fn a_paused_vault_refuses_finalization_and_the_request_survives() {
    let (mut ctx, _) = mature_request();
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
/// re-checked. The request is fixed, so the owner's way out is to cancel.
#[test]
fn a_recipient_reassigned_to_the_queue_after_the_request_is_refused() {
    let (mut ctx, _) = mature_request();
    let user = ctx.user.insecure_clone();
    let (ata, queue_pda) = (ctx.user_deposit_ata, ctx.withdrawal_queue_pda());
    ctx.set_token_account_authority_as(&user, &ata, &queue_pda);
    let before = untouched(&ctx, &user.pubkey(), 1);

    let err = ctx
        .finalize_withdrawal(1, 1)
        .expect_err("queue-owned recipient");
    assert_queue_err(&err, ErrorCode::InvalidRecipient);
    assert_eq!(untouched(&ctx, &user.pubkey(), 1), before);
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
    let (mut ctx, _) = mature_request();
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
    let (mut ctx, _) = mature_request();
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

/// The vault binds the fee account only by `fee_recipient`'s authority, so a
/// `fee_recipient` set to the queue PDA would let the asset escrow pass as the
/// fee account, and the fee would ride the balance delta to the recipient. The
/// queue refuses the escrow in that slot whatever `fee_recipient` says.
#[test]
fn the_asset_escrow_is_refused_as_the_fee_account() {
    let (mut ctx, _) = mature_request();
    let user = ctx.user.pubkey();
    let admin = ctx.admin.insecure_clone();
    let queue = ctx.withdrawal_queue_pda();
    ctx.set_fee_recipient_as(&admin, queue)
        .expect("set_fee_recipient validates nothing");
    let escrow_assets = ctx.queue_escrow(&ctx.deposit_mint);
    let before = untouched(&ctx, &user, 1);

    let mut accounts = ctx.finalize_withdrawal_accounts(&user, &user, 1);
    accounts.fee_recipient_account = escrow_assets;
    let user_kp = ctx.user.insecure_clone();
    let err = ctx
        .send_finalize_withdrawal(&user_kp, accounts, 1)
        .expect_err("the escrow as the fee account");
    assert_queue_err(&err, ErrorCode::FeeAccountIsEscrow);
    assert_anchor_framework_err(&err, 6022);
    assert_eq!(untouched(&ctx, &user, 1), before);
}

/// A request belongs to one queue. Presenting another vault's whole account set
/// with it is refused by the request's `has_one = queue` before the CPI.
#[test]
fn a_request_cannot_be_finalized_through_another_vaults_queue() {
    let (mut ctx, _) = mature_request();
    let signer = ctx.user.insecure_clone();
    let user = signer.pubkey();
    let other = ctx.new_vault_with_queue();
    let fee_recipient = ctx.fee_recipient.pubkey();
    let fee_account = ctx.create_ata_for(&fee_recipient, &other.deposit_mint);

    let mut accounts = ctx.finalize_withdrawal_accounts(&user, &user, 1);
    accounts.queue = other.queue;
    accounts.vault_state = other.vault_state;
    accounts.vault_deposit_ata = other.vault_token;
    accounts.fee_recipient_account = fee_account;
    accounts.escrow_shares = other.escrow_shares;
    accounts.escrow_assets = other.escrow_assets;
    accounts.share_mint = other.share_mint;
    accounts.deposit_mint = other.deposit_mint;
    let err = ctx
        .send_finalize_withdrawal(&signer, accounts, 1)
        .expect_err("another vault's queue");
    assert_anchor_framework_err(&err, 2001);
    assert!(ctx.svm.get_account(&ctx.request_pda(&user, 1)).is_some());
}

// ---- several requests in one transaction ----

/// Holders with a mature request each, ready for one keeper transaction.
fn holders_with_mature_requests(n: usize) -> (VaultCtx, Vec<Pubkey>) {
    let mut ctx = VaultCtx::fresh();
    let mut owners = Vec::new();
    for _ in 0..n {
        let d = ctx.new_depositor(DEPOSIT_AMOUNT);
        ctx.deposit_as(&d, DEPOSIT_AMOUNT).expect("deposit");
        owners.push(d);
    }
    ctx.open_queue(DAY);
    for d in &owners {
        let shares = ctx.token_account_amount(&d.share_ata) / 2;
        ctx.request_withdrawal_as(
            &d.keypair,
            d.share_ata,
            d.deposit_ata,
            1,
            shares,
            Pubkey::default(),
        )
        .expect("request");
    }
    ctx.warp_forward_seconds(DAY as i64);
    let keys = owners.iter().map(|d| d.keypair.pubkey()).collect();
    (ctx, keys)
}

/// The failing instruction's index, from the transaction-level error.
fn failed_instruction(err: &FailedTransactionMetadata) -> u8 {
    match err.err {
        solana_sdk::transaction::TransactionError::InstructionError(index, _) => index,
        ref other => panic!("expected an instruction error, got {other:?}"),
    }
}

/// There is no batch instruction: a keeper puts several `finalize_withdrawal`
/// instructions in one transaction instead. Five requests from five holders,
/// the worst case for accounts, fit a legacy transaction and the compute
/// ceiling, each instruction with its own heap, and each request settles on its
/// own: its event, its payout and its rent back to its owner.
#[test]
fn a_keeper_finalizes_five_requests_in_one_transaction() {
    let (mut ctx, owners) = holders_with_mature_requests(5);
    let keeper = keeper(&mut ctx);
    let mut ixs = vec![
        solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(1_400_000),
    ];
    let mut before = Vec::new();
    for owner in &owners {
        let seq = ctx.request_state_data(owner, 1).sequence;
        ixs.push(ctx.finalize_withdrawal_ix(&keeper.pubkey(), owner, 1, seq));
        let rent = ctx
            .svm
            .get_account(&ctx.request_pda(owner, 1))
            .expect("request")
            .lamports;
        before.push((ctx.svm.get_balance(owner).expect("owner"), rent));
    }
    // The blockhash is fixed-width, so a default one measures the size exactly.
    let tx = solana_sdk::transaction::Transaction::new_signed_with_payer(
        &ixs,
        Some(&keeper.pubkey()),
        &[&keeper],
        solana_sdk::hash::Hash::default(),
    );
    let size = 1 + 64 * tx.signatures.len() + tx.message_data().len();
    assert!(size <= 1232, "{size} bytes");

    let meta = ctx
        .send_instructions(&keeper, &ixs)
        .expect("five finalizes in one transaction");
    eprintln!(
        "5 x finalize_withdrawal: {} CU, {size} bytes",
        meta.compute_units_consumed
    );
    assert!(meta.compute_units_consumed < 1_400_000);
    let events = events_of::<WithdrawalFinalized>(&meta);
    assert_eq!(events.len(), 5);
    for ((owner, event), (lamports, rent)) in owners.iter().zip(&events).zip(&before) {
        assert_eq!(event.request, ctx.request_pda(owner, 1), "in order");
        assert!(event.assets > 0);
        assert!(ctx.svm.get_account(&ctx.request_pda(owner, 1)).is_none());
        assert_eq!(
            ctx.svm.get_balance(owner).expect("owner"),
            lamports + rent,
            "rent back to its own owner"
        );
    }
    assert_eq!(ctx.queue_state_data().pending_requests, 0);
}

/// The transaction is atomic: one refused finalize reverts the ones before it,
/// and the runtime names the instruction that failed.
#[test]
fn one_refused_finalize_reverts_the_whole_transaction() {
    let (mut ctx, owners) = holders_with_mature_requests(3);
    let keeper = keeper(&mut ctx);
    let mut ixs = Vec::new();
    for (i, owner) in owners.iter().enumerate() {
        let seq = ctx.request_state_data(owner, 1).sequence;
        let seq = if i == 2 { seq + 1 } else { seq };
        ixs.push(ctx.finalize_withdrawal_ix(&keeper.pubkey(), owner, 1, seq));
    }
    let paid_before: Vec<u64> = owners
        .iter()
        .map(|o| ctx.token_account_amount(&ctx.request_state_data(o, 1).recipient_token_account))
        .collect();

    let err = ctx
        .send_instructions(&keeper, &ixs)
        .expect_err("the third has a stale sequence");
    assert_queue_err(&err, ErrorCode::StaleRequestSequence);
    assert_eq!(failed_instruction(&err), 2);
    assert_eq!(ctx.queue_state_data().pending_requests, 3, "none paid");
    for (owner, paid) in owners.iter().zip(paid_before) {
        let recipient = ctx.request_state_data(owner, 1).recipient_token_account;
        assert_eq!(ctx.token_account_amount(&recipient), paid);
    }
}

/// A refusal from inside the vault's redeem rolls back too: the first request
/// is paid by the vault and the second runs out of liquidity, and the first
/// payout, its burn and its closed account all come back.
#[test]
fn a_shortfall_part_way_through_reverts_the_payouts_before_it() {
    let (mut ctx, owners) = holders_with_mature_requests(2);
    let keeper = keeper(&mut ctx);
    let shares = ctx.request_state_data(&owners[0], 1).shares;
    let (gross, _) = ctx.quote_redeem(shares);
    let local = ctx.vault_state_data().local_aum;
    ctx.operator_withdraw(local - gross * 3 / 2)
        .expect("leave room for one payout and a half");
    let first_recipient = ctx
        .request_state_data(&owners[0], 1)
        .recipient_token_account;
    let paid_before = ctx.token_account_amount(&first_recipient);
    let supply_before = ctx.share_mint_supply();
    let ixs: Vec<_> = owners
        .iter()
        .map(|o| {
            let seq = ctx.request_state_data(o, 1).sequence;
            ctx.finalize_withdrawal_ix(&keeper.pubkey(), o, 1, seq)
        })
        .collect();

    let err = ctx
        .send_instructions(&keeper, &ixs)
        .expect_err("the second runs out of liquidity");
    assert_anchor_err(&err, VaultError::NotEnoughLiquidity);
    assert_eq!(failed_instruction(&err), 1);
    assert_eq!(ctx.token_account_amount(&first_recipient), paid_before);
    assert_eq!(ctx.share_mint_supply(), supply_before, "the burn is undone");
    for owner in &owners {
        assert!(ctx.svm.get_account(&ctx.request_pda(owner, 1)).is_some());
    }
    assert_eq!(ctx.queue_state_data().pending_requests, 2);
}
