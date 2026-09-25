// Copyright (C) 2026 Fractal Network Ltd
//
// Use of this software is governed by the Business Source License
// included in the LICENSE file.
//
// As of 10 March 2036 (the "Change Date"), use of this software will be
// governed by version 2.0 of the Apache License.

//! The queue program's admin instructions: `initialize_queue` and the two
//! setters. Nothing here escrows a share; that arrives with the request
//! instructions. These tests pin that a queue is created only by the vault's
//! admin, only for a supported mint, at the canonical address the vault will
//! accept; and that every setter is admin-only, bound to the
//! queue's own vault, bounded where the design bounds it, and reports the whole
//! configuration in its event.

use august_vault::state::vault::VAULT_STATE_SEED;
use august_withdrawal_queue::errors::{ErrorCode, ANCHOR_USER_ERROR_OFFSET};
use august_withdrawal_queue::events::{QueueConfigUpdated, QueueInitialized};
use august_withdrawal_queue::state::{
    WithdrawalQueue, MAX_COOLDOWN_SECONDS, MAX_FULFILLMENT_WINDOW_SECONDS,
    MIN_FULFILLMENT_WINDOW_SECONDS, WITHDRAWAL_QUEUE_SEED,
};
use integration_tests::harness::{
    assert_anchor_framework_err, events_of, MintExtension, VaultCtx, VAULT_VERSION,
};
use litesvm::types::FailedTransactionMetadata;
use solana_sdk::program_pack::Pack;
use solana_sdk::{pubkey::Pubkey, signer::Signer};

const DAY: u64 = 24 * 60 * 60;

/// The queue has its own error enum; the harness's `assert_anchor_err` speaks
/// the vault's. Both sit at Anchor's 6000 offset.
fn assert_queue_err(err: &FailedTransactionMetadata, code: ErrorCode) {
    assert_anchor_framework_err(err, code as u32 + ANCHOR_USER_ERROR_OFFSET);
}

/// Base token-account fields, which Token-2022 shares with classic SPL. ATAs
/// under Token-2022 carry an `ImmutableOwner` extension after the base bytes.
fn token_account(ctx: &VaultCtx, key: &Pubkey) -> spl_token::state::Account {
    let data = ctx.svm.get_account(key).expect("token account exists").data;
    spl_token::state::Account::unpack_from_slice(&data[..spl_token::state::Account::LEN])
        .expect("token account layout")
}

fn canonical_bump(vault_state: &Pubkey) -> u8 {
    Pubkey::find_program_address(
        &[WITHDRAWAL_QUEUE_SEED, vault_state.as_ref()],
        &august_withdrawal_queue::ID,
    )
    .1
}

fn assert_config_snapshot(meta: &litesvm::types::TransactionMetadata, ctx: &VaultCtx) {
    let events = events_of::<QueueConfigUpdated>(meta);
    assert_eq!(events.len(), 1, "exactly one QueueConfigUpdated");
    let q = ctx.queue_state_data();
    let e = &events[0];
    assert_eq!(e.vault, ctx.vault_state);
    assert_eq!(e.queue, ctx.withdrawal_queue_pda());
    assert_eq!(e.cooldown_seconds, q.cooldown_seconds);
    assert_eq!(e.fulfillment_window_seconds, q.fulfillment_window_seconds);
}

// ---- initialize_queue ----

#[test]
fn the_admin_creates_a_queue_in_drain_mode() {
    let mut ctx = VaultCtx::fresh();
    let meta = ctx.initialize_queue(DAY).expect("initialize_queue");
    let pda = ctx.withdrawal_queue_pda();

    let q = ctx.queue_state_data();
    assert_eq!(q.vault_state, ctx.vault_state);
    assert_eq!(q.deposit_mint, ctx.deposit_mint);
    assert_eq!(q.share_mint, ctx.share_mint);
    assert_eq!(q.escrow_shares, ctx.queue_escrow(&ctx.share_mint));
    assert_eq!(q.escrow_assets, ctx.queue_escrow(&ctx.deposit_mint));
    assert_eq!(q.cooldown_seconds, DAY);
    assert_eq!(q.fulfillment_window_seconds, 0);
    assert_eq!(
        (q.sequence, q.pending_requests, q.pending_shares),
        (0, 0, 0)
    );
    assert_eq!(
        q.bump,
        canonical_bump(&ctx.vault_state),
        "the stored bump is the canonical one the vault will accept"
    );

    for (escrow, mint) in [
        (q.escrow_shares, ctx.share_mint),
        (q.escrow_assets, ctx.deposit_mint),
    ] {
        let account = token_account(&ctx, &escrow);
        assert_eq!(account.owner, pda, "the queue PDA owns its escrows");
        assert_eq!(account.mint, mint);
        assert_eq!(account.amount, 0);
    }

    let events = events_of::<QueueInitialized>(&meta);
    assert_eq!(events.len(), 1, "exactly one QueueInitialized");
    let e = &events[0];
    assert_eq!(e.vault, ctx.vault_state);
    assert_eq!(e.queue, pda);
    assert_eq!(e.deposit_mint, ctx.deposit_mint);
    assert_eq!(e.share_mint, ctx.share_mint);
    assert_eq!(e.escrow_shares, q.escrow_shares);
    assert_eq!(e.escrow_assets, q.escrow_assets);
    assert_eq!(e.cooldown_seconds, DAY);
}

/// An ATA's address is a pure function of its owner, mint and token program, and
/// anyone may create one. A stranger who creates the queue's escrows first, and
/// even funds one, must not be able to block the vault from ever getting a queue.
#[test]
fn pre_created_escrow_atas_do_not_block_initialization() {
    let mut ctx = VaultCtx::fresh();
    let pda = ctx.withdrawal_queue_pda();
    let (share_mint, deposit_mint) = (ctx.share_mint, ctx.deposit_mint);
    let shares_ata = ctx.create_ata_for(&pda, &share_mint);
    let assets_ata = ctx.create_ata_for(&pda, &deposit_mint);
    ctx.mint_deposit_to(&assets_ata, 1_000);

    ctx.initialize_queue(DAY)
        .expect("pre-created escrows are adopted, not refused");

    let q = ctx.queue_state_data();
    assert_eq!(q.escrow_shares, shares_ata);
    assert_eq!(q.escrow_assets, assets_ata);
    assert_eq!(
        token_account(&ctx, &assets_ata).amount,
        1_000,
        "a balance already there is a donation, left where it is"
    );
}

#[test]
fn a_token_2022_vault_gets_a_queue_too() {
    let mut ctx = VaultCtx::fresh_token_2022();
    ctx.initialize_queue(DAY)
        .expect("initialize_queue under Token-2022");
    let q = ctx.queue_state_data();
    for escrow in [q.escrow_shares, q.escrow_assets] {
        let owner = ctx.svm.get_account(&escrow).expect("escrow").owner;
        assert_eq!(
            owner,
            spl_token_2022::ID,
            "escrows live under the vault's token program"
        );
    }
}

/// The one Token-2022 extension the design allows a deposit mint to carry.
#[test]
fn a_metadata_pointer_mint_is_supported() {
    let mut ctx = VaultCtx::fresh_token_2022_with_extensions(&[MintExtension::MetadataPointer]);
    ctx.initialize_queue(DAY)
        .expect("a mint carrying only MetadataPointer passes the allow-list");
}

/// `cancel_withdrawal` works in every state only because `escrow_shares`
/// cannot be frozen. The vault never gives its share mint a freeze authority;
/// one planted here stands in for a vault that someday did.
#[test]
fn a_freezable_share_mint_is_refused() {
    let mut ctx = VaultCtx::fresh();
    let mut account = ctx.svm.get_account(&ctx.share_mint).expect("share mint");
    let mut mint = spl_token::state::Mint::unpack(&account.data).expect("mint");
    mint.freeze_authority = Some(Pubkey::new_unique()).into();
    spl_token::state::Mint::pack(mint, &mut account.data).expect("pack");
    let share_mint = ctx.share_mint;
    ctx.svm.set_account(share_mint, account).expect("plant");

    let err = ctx
        .initialize_queue(DAY)
        .expect_err("a freezable share mint");
    assert_queue_err(&err, ErrorCode::UnsupportedShareMint);
    assert_anchor_framework_err(&err, 6017);
}

/// Frozen-by-default accounts alter no transfer, yet they would create frozen
/// escrows and fail every finalization until a freeze authority acted. The
/// allow-list refuses it without having to know that.
#[test]
fn a_frozen_default_state_mint_is_refused() {
    let mut ctx =
        VaultCtx::fresh_token_2022_with_extensions(&[MintExtension::DefaultAccountStateFrozen]);
    let err = ctx
        .initialize_queue(DAY)
        .expect_err("an extension outside the allow-list is refused");
    assert_queue_err(&err, ErrorCode::UnsupportedDepositMint);
    // Consumers match on the number, so pin the number.
    assert_anchor_framework_err(&err, 6006);
    assert!(
        ctx.svm.get_account(&ctx.withdrawal_queue_pda()).is_none(),
        "a refused creation leaves no account behind"
    );
}

/// The admin authorizes and a separate payer funds, so a cold or MPC-held admin
/// key can hold the role without carrying SOL.
#[test]
fn the_admin_authorizes_and_a_separate_payer_funds() {
    let mut ctx = VaultCtx::fresh();
    let payer = ctx.new_funded_keypair(5_000_000_000);
    let admin = ctx.admin.insecure_clone();
    let admin_before = ctx.svm.get_balance(&admin.pubkey()).expect("admin funded");
    let payer_before = ctx.svm.get_balance(&payer.pubkey()).expect("payer funded");

    ctx.initialize_queue_as(&admin, &payer, DAY)
        .expect("separate payer");

    assert_eq!(
        ctx.svm.get_balance(&admin.pubkey()).expect("admin"),
        admin_before,
        "the admin paid nothing"
    );
    assert!(
        ctx.svm.get_balance(&payer.pubkey()).expect("payer") < payer_before,
        "the payer funded rent and fees"
    );
}

#[test]
fn a_non_admin_cannot_create_the_queue() {
    let mut ctx = VaultCtx::fresh();
    // Funded, so the failure is the authorization check and not the rent.
    let impostor = ctx.new_funded_keypair(5_000_000_000);

    let err = ctx
        .initialize_queue_as(&impostor, &impostor, DAY)
        .expect_err("only the vault's admin creates its queue");
    assert_queue_err(&err, ErrorCode::NotVaultAdmin);
    assert_anchor_framework_err(&err, 6000);
    assert!(ctx.svm.get_account(&ctx.withdrawal_queue_pda()).is_none());
}

/// `Account<VaultState>` pins the vault program as owner, so a system-owned
/// account in the vault slot fails Anchor's owner check as 3007.
#[test]
fn an_account_the_vault_program_does_not_own_is_not_a_vault() {
    let mut ctx = VaultCtx::fresh();
    let fake_vault = ctx.new_funded_keypair(1_000_000_000).pubkey();
    let admin = ctx.admin.insecure_clone();
    let payer = ctx.payer.insecure_clone();
    let (deposit_mint, share_mint) = (ctx.deposit_mint, ctx.share_mint);

    let err = ctx
        .initialize_queue_for_vault_as(&admin, &payer, fake_vault, deposit_mint, share_mint, DAY)
        .expect_err("not a vault");
    assert_anchor_framework_err(&err, 3007);
}

#[test]
fn a_queue_is_created_once() {
    let mut ctx = VaultCtx::fresh();
    ctx.initialize_queue(DAY).expect("first");

    ctx.initialize_queue(2 * DAY)
        .expect_err("the PDA already exists, so `init` refuses");
    assert_eq!(
        ctx.queue_state_data().cooldown_seconds,
        DAY,
        "the second call changed nothing"
    );
}

#[test]
fn the_cooldown_bound_holds_at_creation() {
    let mut ctx = VaultCtx::fresh();
    let err = ctx
        .initialize_queue(MAX_COOLDOWN_SECONDS + 1)
        .expect_err("one over the bound");
    assert_queue_err(&err, ErrorCode::CooldownOutOfBounds);
    assert_anchor_framework_err(&err, 6003);
    assert!(ctx.svm.get_account(&ctx.withdrawal_queue_pda()).is_none());

    ctx.initialize_queue(MAX_COOLDOWN_SECONDS)
        .expect("the maximum itself is allowed");
}

/// The end-to-end property the canonical bump exists for: the vault's attach
/// accepts the queue this instruction created, which makes it live.
#[test]
fn the_vault_attaches_the_new_queue() {
    let mut ctx = VaultCtx::fresh();
    ctx.initialize_queue(DAY).expect("initialize_queue");
    let pda = ctx.withdrawal_queue_pda();

    ctx.attach_withdrawal_queue(pda)
        .expect("the vault accepts the initialized queue");
    assert_eq!(ctx.vault_state_data().withdrawal_queue(), Some(pda));
}

// ---- set_cooldown / set_fulfillment_window ----

#[test]
fn set_cooldown_is_bounded_and_snapshots_the_config() {
    let mut ctx = VaultCtx::fresh();
    ctx.initialize_queue(DAY).expect("initialize_queue");

    let meta = ctx.set_cooldown(7 * DAY).expect("within bounds");
    assert_eq!(ctx.queue_state_data().cooldown_seconds, 7 * DAY);
    assert_config_snapshot(&meta, &ctx);

    let err = ctx
        .set_cooldown(MAX_COOLDOWN_SECONDS + 1)
        .expect_err("one over the bound");
    assert_queue_err(&err, ErrorCode::CooldownOutOfBounds);
    assert_eq!(
        ctx.queue_state_data().cooldown_seconds,
        7 * DAY,
        "a refused value leaves the old one"
    );
}

#[test]
fn set_fulfillment_window_is_bounded_with_zero_meaning_never() {
    let mut ctx = VaultCtx::fresh();
    ctx.initialize_queue(DAY).expect("initialize_queue");

    let meta = ctx
        .set_fulfillment_window(MAX_FULFILLMENT_WINDOW_SECONDS)
        .expect("the maximum itself is allowed");
    assert_eq!(
        ctx.queue_state_data().fulfillment_window_seconds,
        MAX_FULFILLMENT_WINDOW_SECONDS
    );
    assert_config_snapshot(&meta, &ctx);

    let err = ctx
        .set_fulfillment_window(MAX_FULFILLMENT_WINDOW_SECONDS + 1)
        .expect_err("one over the bound");
    assert_queue_err(&err, ErrorCode::FulfillmentWindowOutOfBounds);
    assert_anchor_framework_err(&err, 6004);

    ctx.set_fulfillment_window(0).expect("zero disables expiry");
    assert_eq!(ctx.queue_state_data().fulfillment_window_seconds, 0);

    // Seconds, not days: `7` meant as a week would expire every new request
    // seconds after it matures.
    for seconds in [1, 7, MIN_FULFILLMENT_WINDOW_SECONDS - 1] {
        let err = ctx
            .set_fulfillment_window(seconds)
            .expect_err("a non-zero window under a day");
        assert_queue_err(&err, ErrorCode::FulfillmentWindowOutOfBounds);
    }
    assert_eq!(ctx.queue_state_data().fulfillment_window_seconds, 0);
    ctx.set_fulfillment_window(MIN_FULFILLMENT_WINDOW_SECONDS)
        .expect("the minimum itself is allowed");
}

// ---- every setter: admin-only, bound to the queue's vault, on an existing queue ----

#[test]
fn setters_are_admin_only() {
    let mut ctx = VaultCtx::fresh();
    ctx.initialize_queue(DAY).expect("initialize_queue");
    let impostor = ctx.new_funded_keypair(1_000_000_000);
    let before = ctx.queue_state_data();

    let attempts: [Result<_, FailedTransactionMetadata>; 2] = [
        ctx.set_cooldown_as(&impostor, 2 * DAY),
        ctx.set_fulfillment_window_as(&impostor, DAY),
    ];
    for attempt in attempts {
        let err = attempt.expect_err("only the vault's admin configures its queue");
        assert_queue_err(&err, ErrorCode::NotVaultAdmin);
    }
    let after = ctx.queue_state_data();
    assert_eq!(
        (after.cooldown_seconds, after.fulfillment_window_seconds),
        (before.cooldown_seconds, before.fulfillment_window_seconds),
        "refused calls change nothing"
    );
}

/// The admin of a second vault is a real admin, just not of this queue's vault.
/// Pairing queue A with vault B is refused before the handler runs.
#[test]
fn a_queue_cannot_be_driven_with_another_vaults_account() {
    let mut ctx = VaultCtx::fresh();
    ctx.initialize_queue(DAY).expect("queue for vault A");
    let queue_a = ctx.withdrawal_queue_pda();

    let mint_b = ctx.create_extra_deposit_mint();
    let authority = ctx.protocol_authority.insecure_clone();
    ctx.try_initialize_vault(&authority, mint_b, VAULT_VERSION)
        .expect("vault B");
    let vault_b = Pubkey::find_program_address(
        &[VAULT_STATE_SEED, mint_b.as_ref(), &[VAULT_VERSION]],
        &august_vault::ID,
    )
    .0;

    let admin = ctx.admin.insecure_clone();
    let err = ctx
        .queue_admin_call_with_accounts_as(
            &admin,
            queue_a,
            vault_b,
            anchor_lang::InstructionData::data(
                &august_withdrawal_queue::instruction::SetCooldown { seconds: 2 * DAY },
            ),
        )
        .expect_err("queue A is not vault B's queue");
    assert_queue_err(&err, ErrorCode::VaultMismatch);
    assert_eq!(ctx.queue_state_data().cooldown_seconds, DAY);
}

#[test]
fn setters_need_an_existing_queue() {
    let mut ctx = VaultCtx::fresh();
    let err = ctx
        .set_cooldown(DAY)
        .expect_err("no queue has been initialized");
    // Anchor's `AccountNotInitialized`.
    assert_anchor_framework_err(&err, 3012);
}

/// Pins the account shape the generated client will decode: the same bytes the
/// program wrote, read back through the program's own type.
#[test]
fn the_queue_account_is_exactly_len_bytes() {
    let mut ctx = VaultCtx::fresh();
    ctx.initialize_queue(DAY).expect("initialize_queue");
    let account = ctx
        .svm
        .get_account(&ctx.withdrawal_queue_pda())
        .expect("queue");
    assert_eq!(account.data.len(), WithdrawalQueue::LEN);
    assert_eq!(account.owner, august_withdrawal_queue::ID);
}
