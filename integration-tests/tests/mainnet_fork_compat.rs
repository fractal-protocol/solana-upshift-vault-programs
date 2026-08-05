//! Mainnet fork / on-chain layout-compatibility guard.
//!
//! Loads the built `august_vault` bytecode (`target/deploy/august_vault.so`)
//! into LiteSVM together with the REAL on-chain accounts of all three live
//! vaults — USDC, jitoSOL, and the one that is off par — dumped byte-for-byte
//! from mainnet into `tests/fixtures/*.bin`, and proves the current code operates
//! on the *existing* state correctly:
//!   1. deserializes the existing `VaultState` (guards the account layout);
//!   2. reads the vault reserve token account back, consistent with accounting;
//!   3. exercises a real user deposit against the live USDC vault state, minting
//!      shares exactly per the on-chain formula with correct accounting;
//!   4. redeems those shares straight back out, so `assets_for_redeem` — which
//!      carries the raised offsets and the pro-rata cap — is exercised against
//!      real state too, and the round trip is shown not to extract value.
//!
//! The USDC and jitoSOL snapshots are exactly 1:1 (supply == total_assets), a
//! state in which the share-price offsets cancel identically — so those two tests
//! are insensitive to the offsets' *values*. They guard compatibility with
//! existing accounts, not the tuning of the math.
//!
//! That insensitivity was total until the third fixture was added: with only
//! at-par vaults, reverting the offsets from 10^6 back to the deployed value of 1
//! left every test in this file passing, so nothing here observed what the retune
//! did to real state. The off-par vault closes that — see
//! `offpar_vault_real_state_is_above_par_and_prices_inside_pro_rata`, which is
//! deliberately offset-SENSITIVE and pins the size of the change.
//!
//! WHY THIS EXISTS: every other test uses *fresh* vaults, so they can't catch a
//! change that breaks compatibility with accounts created by an *earlier*
//! program version. For an upgradeable program with live funds this is the one
//! test that guards "new code can still read + operate on old on-chain state" —
//! the core risk of any program upgrade.
//!
//! The fixtures are FROZEN snapshots (USDC + jitoSOL at mainnet ~slot
//! 434,275,342; the off-par vault at ~slot 437,402,306 — they need not share a
//! slot, since each test only reads its own vault's accounts), so this test is
//! deterministic and network-free — live vault activity does NOT affect it.
//! The `*_AUM` constants are the field values *in that snapshot*; asserting them
//! verifies the byte→field mapping (i.e. the layout). If an intentional change
//! alters the `VaultState` layout or deposit math, refresh the fixtures and
//! these constants in the SAME PR. To refresh a fixture:
//!   curl -s <rpc> -d '{"jsonrpc":"2.0","id":1,"method":"getAccountInfo",
//!     "params":["<pubkey>",{"encoding":"base64"}]}' \
//!     | jq -r .result.value.data[0] | base64 -d > tests/fixtures/<name>.bin

use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use august_vault::{
    accounts as ix_accounts, instruction as ix_data,
    state::vault::{VaultState, EXTRA_SHARES, VIRTUAL_ASSETS},
};
use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::Instruction,
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use spl_token::{
    solana_program::program_option::COption,
    state::{Account as SplAccount, AccountState, Mint as SplMint},
};
use std::str::FromStr;

const SPL_TOKEN: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";

// Field values in the frozen snapshot (used to verify the byte→field mapping).
const USDC_LOCAL_AUM: u64 = 92_501_610_605;
const USDC_DEPLOYED_AUM: u64 = 1_760_151_000_000;
const JITO_LOCAL_AUM: u64 = 584_082_970;
const JITO_DEPLOYED_AUM: u64 = 1_582_093_475;
/// The one live vault that is OFF par (see `offpar_vault_real_state_is_above_par`).
const OFFPAR_SUPPLY: u64 = 200_999_500_249;
const OFFPAR_LOCAL_AUM: u64 = 0;
const OFFPAR_DEPLOYED_AUM: u64 = 402_000_000_000;

fn pk(s: &str) -> Pubkey {
    Pubkey::from_str(s).unwrap()
}

fn new_svm() -> LiteSVM {
    let mut svm = LiteSVM::new();
    let prog = include_bytes!("../../target/deploy/august_vault.so").to_vec();
    svm.add_program(august_vault::ID, &prog)
        .expect("load august_vault.so — build the program first");
    svm
}

/// Inject a raw account at `addr` with the given owner + data (rent-exempt).
fn inject(svm: &mut LiteSVM, addr: Pubkey, owner: Pubkey, data: Vec<u8>) {
    let lamports = Rent::default().minimum_balance(data.len()).max(1);
    svm.set_account(
        addr,
        Account {
            lamports,
            data,
            owner,
            executable: false,
            rent_epoch: 0,
        },
    )
    .expect("set_account");
}

/// Build an SPL token account owned by `owner` for `mint` holding `amount`.
fn packed_token(mint: Pubkey, owner: Pubkey, amount: u64) -> Vec<u8> {
    let acct = SplAccount {
        mint,
        owner,
        amount,
        delegate: COption::None,
        state: AccountState::Initialized,
        is_native: COption::None,
        delegated_amount: 0,
        close_authority: COption::None,
    };
    let mut buf = vec![0u8; SplAccount::LEN];
    SplAccount::pack(acct, &mut buf).unwrap();
    buf
}

fn token_amount(svm: &LiteSVM, addr: &Pubkey) -> u64 {
    SplAccount::unpack(&svm.get_account(addr).unwrap().data[..SplAccount::LEN])
        .unwrap()
        .amount
}

fn read_vault_state(svm: &LiteSVM, addr: &Pubkey) -> VaultState {
    let acct = svm.get_account(addr).unwrap();
    VaultState::try_deserialize(&mut acct.data.as_slice())
        .expect("current code must deserialize the real on-chain VaultState")
}

/// The live vaults predate `share_offset`, so its bytes are old padding and must
/// read as zero — which `share_offset()` resolves to the default.
///
/// This is the field the upgrade's pricing depends on and it was the one field
/// the byte→field assertions omitted. The unit test
/// `share_offset_stays_at_its_byte_offset` only proves the position in a
/// synthetic `Default` serialization; nothing established it on real bytes, so a
/// field inserted ahead of it would shift it into occupied padding and re-price
/// both live vaults with whatever those bytes happen to hold, silently.
///
/// If a refreshed fixture is ever taken from a vault created *after* this
/// release it will store a real offset and this assertion will fail. That is the
/// intended signal: `ref_shares` / `ref_assets` hardcode the default offset, so
/// they would need the stored value threaded through before the fixture can be
/// used.
fn assert_live_vault_offset_is_legacy_zero(vs: &VaultState) {
    assert_eq!(
        vs.share_offset, 0,
        "live vaults predate share_offset, so its bytes must read as zero \
         (byte→field check at account byte 199)"
    );
    assert_eq!(
        vs.share_offset(),
        EXTRA_SHARES,
        "a stored zero must resolve to the default offset — this is what prices \
         both live vaults after the upgrade"
    );
}

/// Independent reference for the deposit share formula:
/// `floor(amount * (supply + EXTRA_SHARES) / (total_assets + VIRTUAL_ASSETS))`.
///
/// Deliberately does NOT call the program's `shares_for_deposit`, so a drift in
/// the *shape* of the program's formula is caught here instead of being silently
/// mirrored in the expectation.
///
/// Valid **at or above par only**: it implements the offset term and omits the
/// pro-rata floor, which cannot bind while `total_assets >= supply`. Every
/// caller here is at or above par, and the assertion below enforces that. If a
/// future fixture refresh captures a vault that has booked a loss, add
/// `.max(amount * supply / total_assets)`.
///
/// It reads the offset *constants* rather than hardcoding them: hardcoded values
/// would go stale the moment the offsets are retuned, leaving this reference
/// quietly asserting a formula the program no longer uses. The offsets' chosen
/// values are pinned by the unit tests in `state/vault.rs` and by
/// `share_burn_pricing.rs`; what this file guards is the byte→field mapping and
/// that live state still round-trips.
fn ref_shares(supply: u64, total_assets: u64, amount: u64) -> u64 {
    // Guard the precondition rather than trust it: this implements the offset
    // term only and omits the pro-rata floor, which cannot bind at or above par.
    // Every current caller is at or above par, but this file's header instructs
    // future maintainers to refresh the fixtures from mainnet — and a vault that
    // has booked a loss would land below par, where this reference would silently
    // disagree with the program instead of failing loudly here.
    assert!(
        total_assets >= supply,
        "ref_shares is only valid at or above par (total_assets={total_assets}, \
         supply={supply}); below par the program applies a pro-rata floor this \
         reference does not implement"
    );
    ((amount as u128 * (supply as u128 + EXTRA_SHARES)) / (total_assets as u128 + VIRTUAL_ASSETS))
        as u64
}

/// Companion reference for `assets_for_redeem`, valid at or above par for the
/// same reason (there the offset term is the smaller and the pro-rata cap is
/// inert).
fn ref_assets(supply: u64, total_assets: u64, shares: u64) -> u64 {
    assert!(
        total_assets >= supply,
        "ref_assets is only valid at or above par (total_assets={total_assets}, \
         supply={supply})"
    );
    ((shares as u128 * (total_assets as u128 + VIRTUAL_ASSETS)) / (supply as u128 + EXTRA_SHARES))
        as u64
}

/// Re-serialize a (possibly modified) VaultState back into its account, keeping
/// the original 455-byte allocation. Used to engineer a non-1:1 snapshot.
fn overwrite_vault_state(svm: &mut LiteSVM, addr: Pubkey, vs: &VaultState) {
    use anchor_lang::AccountSerialize;
    let mut data = Vec::new();
    vs.try_serialize(&mut data).unwrap();
    let orig_len = svm.get_account(&addr).unwrap().data.len();
    assert!(
        data.len() <= orig_len,
        "serialized VaultState grew past its allocation"
    );
    data.resize(orig_len, 0);
    inject(svm, addr, august_vault::ID, data);
}

#[test]
fn usdc_vault_real_state_read_and_deposit() {
    let mut svm = new_svm();
    let spl = pk(SPL_TOKEN);

    let vault_state = pk("HegTiqVxUvnh3fD9ZA2v7PF3XnoqHgz4ytJRZKdLJ5ra");
    let share_mint = pk("CnhPtD2gHHrUvfuA6HrDdLQBKjGgVL8HZMJNCZdXuWEs");
    let vault_ata = pk("Tit1tGJ4F7F1LeGcyUhTstisxRLEfaNMuB8bBxf65ED");
    let deposit_mint = pk("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

    inject(
        &mut svm,
        vault_state,
        august_vault::ID,
        include_bytes!("fixtures/usdc_vault_state.bin").to_vec(),
    );
    inject(
        &mut svm,
        share_mint,
        spl,
        include_bytes!("fixtures/usdc_share_mint.bin").to_vec(),
    );
    inject(
        &mut svm,
        vault_ata,
        spl,
        include_bytes!("fixtures/usdc_vault_ata.bin").to_vec(),
    );
    inject(
        &mut svm,
        deposit_mint,
        spl,
        include_bytes!("fixtures/usdc_mint.bin").to_vec(),
    );

    // 1. READ — current struct maps the real bytes to the right fields (layout).
    let vs = read_vault_state(&svm, &vault_state);
    assert_eq!(vs.deposit_mint, deposit_mint, "deposit_mint field");
    assert_eq!(vs.share_mint, share_mint, "share_mint field");
    assert_eq!(vs.vault_version, [0]);
    assert!(!vs.paused, "vault not paused");
    assert_eq!(
        vs.local_aum, USDC_LOCAL_AUM,
        "snapshot local_aum (byte→field check)"
    );
    assert_eq!(
        vs.deployed_aum, USDC_DEPLOYED_AUM,
        "snapshot deployed_aum (byte→field check)"
    );
    assert_live_vault_offset_is_legacy_zero(&vs);

    // Snapshot-agnostic invariants (hold for any healthy vault state).
    assert_eq!(
        vs.total_assets().unwrap(),
        vs.local_aum + vs.deployed_aum,
        "invariant: total_assets == local_aum + deployed_aum"
    );
    assert_eq!(
        token_amount(&svm, &vault_ata),
        vs.local_aum,
        "invariant: on-chain reserve balance == local_aum accounting"
    );

    let total_assets = vs.total_assets().unwrap();
    let supply = SplMint::unpack(&svm.get_account(&share_mint).unwrap().data[..SplMint::LEN])
        .unwrap()
        .supply;

    // 3. EXERCISE — a real user deposit against the live state.
    let user = Keypair::new();
    svm.airdrop(&user.pubkey(), 1_000_000_000).unwrap();
    let deposit: u64 = 1_000_000; // 1 USDC (6 decimals)
    let user_usdc = Keypair::new().pubkey();
    let user_shares_acct = Keypair::new().pubkey();
    inject(
        &mut svm,
        user_usdc,
        spl,
        packed_token(deposit_mint, user.pubkey(), deposit),
    );
    inject(
        &mut svm,
        user_shares_acct,
        spl,
        packed_token(share_mint, user.pubkey(), 0),
    );

    // Independent expectation (not the program's own helper). This snapshot is
    // ~1:1 (supply == total_assets), so 1 USDC in -> 1 share; the non-unit
    // rounding path is covered by `usdc_deposit_rounding_on_nonunit_state`.
    let expected_shares = ref_shares(supply, total_assets, deposit);
    assert_eq!(
        expected_shares, deposit,
        "frozen: 1:1 snapshot mints 1 share per asset unit"
    );

    let ix = Instruction {
        program_id: august_vault::ID,
        accounts: ix_accounts::Deposit {
            vault_state,
            vault_token_ata: vault_ata,
            sender_token_account: user_usdc,
            sender_share_account: user_shares_acct,
            share_mint,
            deposit_mint,
            signer: user.pubkey(),
            token_program: spl,
        }
        .to_account_metas(None),
        data: ix_data::Deposit { amount: deposit }.data(),
    };
    let bh = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&user.pubkey()), &[&user], bh);
    let res = svm.send_transaction(tx);
    assert!(
        res.is_ok(),
        "deposit against real USDC vault state failed: {:?}",
        res.err()
    );

    // accounting
    let minted = token_amount(&svm, &user_shares_acct);
    assert_eq!(
        minted, expected_shares,
        "shares minted must match the on-chain formula"
    );
    assert!(minted > 0, "deposit minted zero shares");
    assert_eq!(
        token_amount(&svm, &vault_ata),
        USDC_LOCAL_AUM + deposit,
        "reserve += deposit"
    );
    let vs_after = read_vault_state(&svm, &vault_state);
    assert_eq!(
        vs_after.local_aum,
        USDC_LOCAL_AUM + deposit,
        "local_aum += deposit"
    );
    assert_eq!(
        vs_after.deployed_aum, USDC_DEPLOYED_AUM,
        "deployed_aum unchanged by user deposit"
    );

    println!(
        "USDC fork OK: reserve={USDC_LOCAL_AUM} supply={supply} deposit={deposit} -> shares={minted} (formula {expected_shares}); local_aum {USDC_LOCAL_AUM}->{}",
        vs_after.local_aum
    );

    // 4. EXERCISE THE OTHER DIRECTION — redeem straight back out.
    //
    // Without this, `assets_for_redeem` was never run against forked state at
    // all, even though it carries both the raised offsets and the new pro-rata
    // cap. The live vault's withdrawal_fee is 0, so the payout is the whole
    // amount; the fee-bearing path is covered by `fee_bearing_redeem.rs`.
    let fee_recipient_acct = Keypair::new().pubkey();
    inject(
        &mut svm,
        fee_recipient_acct,
        spl,
        packed_token(deposit_mint, vs_after.fee_recipient, 0),
    );
    assert_eq!(
        vs_after.withdrawal_fee, 0,
        "fixture assumption: the live USDC vault charges no withdrawal fee"
    );

    let supply_after_deposit =
        SplMint::unpack(&svm.get_account(&share_mint).unwrap().data[..SplMint::LEN])
            .unwrap()
            .supply;
    let total_after_deposit = vs_after.total_assets().unwrap();
    let expected_assets = ref_assets(supply_after_deposit, total_after_deposit, minted);

    let redeem_ix = Instruction {
        program_id: august_vault::ID,
        accounts: ix_accounts::Redeem {
            vault_state,
            vault_deposit_ata: vault_ata,
            sender_token_account: user_usdc,
            sender_share_account: user_shares_acct,
            fee_recipient_account: fee_recipient_acct,
            share_mint,
            deposit_mint,
            signer: user.pubkey(),
            token_program: spl,
        }
        .to_account_metas(None),
        data: ix_data::Redeem { shares: minted }.data(),
    };
    let bh = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[redeem_ix], Some(&user.pubkey()), &[&user], bh);
    let res = svm.send_transaction(tx);
    assert!(
        res.is_ok(),
        "redeem against real USDC vault state failed: {:?}",
        res.err()
    );

    let returned = token_amount(&svm, &user_usdc);
    assert_eq!(
        returned, expected_assets,
        "assets returned must match the on-chain formula"
    );
    // The round trip must not extract value from the vault — the property the
    // floor/cap pair exists to preserve, here against real on-chain state.
    assert!(
        returned <= deposit,
        "round trip extracted value: paid {deposit}, took {returned}"
    );
    assert_eq!(
        token_amount(&svm, &user_shares_acct),
        0,
        "all shares burned on redeem"
    );
    let vs_final = read_vault_state(&svm, &vault_state);
    assert_eq!(
        vs_final.local_aum,
        token_amount(&svm, &vault_ata),
        "local_aum must still equal the reserve after the round trip"
    );
    println!("USDC fork redeem OK: {minted} shares -> {returned} units (paid {deposit})");
}

#[test]
fn jito_vault_real_state_read() {
    let mut svm = new_svm();
    let spl = pk(SPL_TOKEN);

    let vault_state = pk("2tmMcVv2Ene7wFGebPivhwYhAZyjaJoibMz1GYVaXsB1");
    let share_mint = pk("5RnDkuCHK8BMbpJJPPsHvin7gKpCgsAfbihHTPnCHMjE");
    let vault_ata = pk("GvW2AZwiXSfHVjNEYTuK8nZz2opxv5cW8RxsMrRVoY9K");
    let deposit_mint = pk("J1toso1uCk3RLmjorhTtrVwY9HJ7X8V9yYac6Y7kGCPn");

    inject(
        &mut svm,
        vault_state,
        august_vault::ID,
        include_bytes!("fixtures/jito_vault_state.bin").to_vec(),
    );
    inject(
        &mut svm,
        share_mint,
        spl,
        include_bytes!("fixtures/jito_share_mint.bin").to_vec(),
    );
    inject(
        &mut svm,
        vault_ata,
        spl,
        include_bytes!("fixtures/jito_vault_ata.bin").to_vec(),
    );
    inject(
        &mut svm,
        deposit_mint,
        spl,
        include_bytes!("fixtures/jito_mint.bin").to_vec(),
    );

    let vs = read_vault_state(&svm, &vault_state);
    assert_eq!(vs.deposit_mint, deposit_mint);
    assert_eq!(vs.share_mint, share_mint);
    assert_eq!(vs.vault_version, [0]);
    assert!(!vs.paused);
    assert_eq!(
        vs.local_aum, JITO_LOCAL_AUM,
        "snapshot local_aum (byte→field check)"
    );
    assert_eq!(
        vs.deployed_aum, JITO_DEPLOYED_AUM,
        "snapshot deployed_aum (byte→field check)"
    );
    assert_live_vault_offset_is_legacy_zero(&vs);
    assert_eq!(
        vs.total_assets().unwrap(),
        vs.local_aum + vs.deployed_aum,
        "invariant: total_assets == local_aum + deployed_aum"
    );
    assert_eq!(
        token_amount(&svm, &vault_ata),
        vs.local_aum,
        "invariant: on-chain reserve balance == local_aum accounting"
    );
    println!("jitoSOL fork OK: local_aum={JITO_LOCAL_AUM} deployed_aum={JITO_DEPLOYED_AUM} reserve={JITO_LOCAL_AUM}");
}

/// Guards floor-rounding of the deposit math on a NON-1:1 share price, which the
/// (currently ~1:1) live snapshots don't exercise. Takes the REAL USDC vault +
/// share mint but re-injects the VaultState with a SYNTHETIC total_assets =
/// 2 * supply (0.5 share price). A 3-unit deposit then yields
/// 3*(S+EXTRA_SHARES)/(2S+VIRTUAL_ASSETS) ≈ 1.5 -> floors to 1 share.
/// Expectation is BOTH the independent reference formula and a frozen literal.
#[test]
fn usdc_deposit_rounding_on_nonunit_state() {
    let mut svm = new_svm();
    let spl = pk(SPL_TOKEN);
    let vault_state = pk("HegTiqVxUvnh3fD9ZA2v7PF3XnoqHgz4ytJRZKdLJ5ra");
    let share_mint = pk("CnhPtD2gHHrUvfuA6HrDdLQBKjGgVL8HZMJNCZdXuWEs");
    let vault_ata = pk("Tit1tGJ4F7F1LeGcyUhTstisxRLEfaNMuB8bBxf65ED");
    let deposit_mint = pk("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    inject(
        &mut svm,
        vault_state,
        august_vault::ID,
        include_bytes!("fixtures/usdc_vault_state.bin").to_vec(),
    );
    inject(
        &mut svm,
        share_mint,
        spl,
        include_bytes!("fixtures/usdc_share_mint.bin").to_vec(),
    );
    inject(
        &mut svm,
        vault_ata,
        spl,
        include_bytes!("fixtures/usdc_vault_ata.bin").to_vec(),
    );
    inject(
        &mut svm,
        deposit_mint,
        spl,
        include_bytes!("fixtures/usdc_mint.bin").to_vec(),
    );

    let supply = SplMint::unpack(&svm.get_account(&share_mint).unwrap().data[..SplMint::LEN])
        .unwrap()
        .supply;

    // Synthetic 0.5x share price: total_assets = 2 * supply.
    let total_assets: u64 = supply.checked_mul(2).unwrap();
    let mut vs = read_vault_state(&svm, &vault_state);
    vs.deployed_aum = total_assets - vs.local_aum;
    overwrite_vault_state(&mut svm, vault_state, &vs);
    assert_eq!(
        read_vault_state(&svm, &vault_state).total_assets().unwrap(),
        total_assets
    );

    let user = Keypair::new();
    svm.airdrop(&user.pubkey(), 1_000_000_000).unwrap();
    let deposit: u64 = 3;
    let user_usdc = Keypair::new().pubkey();
    let user_shares_acct = Keypair::new().pubkey();
    inject(
        &mut svm,
        user_usdc,
        spl,
        packed_token(deposit_mint, user.pubkey(), deposit),
    );
    inject(
        &mut svm,
        user_shares_acct,
        spl,
        packed_token(share_mint, user.pubkey(), 0),
    );

    let expected = ref_shares(supply, total_assets, deposit);
    assert_eq!(
        expected, 1,
        "reference: 3 * (S + offset) / (2S + offset) floors to 1"
    );

    let ix = Instruction {
        program_id: august_vault::ID,
        accounts: ix_accounts::Deposit {
            vault_state,
            vault_token_ata: vault_ata,
            sender_token_account: user_usdc,
            sender_share_account: user_shares_acct,
            share_mint,
            deposit_mint,
            signer: user.pubkey(),
            token_program: spl,
        }
        .to_account_metas(None),
        data: ix_data::Deposit { amount: deposit }.data(),
    };
    let bh = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&user.pubkey()), &[&user], bh);
    assert!(
        svm.send_transaction(tx).is_ok(),
        "non-1:1 deposit against real USDC vault failed"
    );

    let minted = token_amount(&svm, &user_shares_acct);
    assert_eq!(
        minted, expected,
        "handler must floor-match the independent reference"
    );
    assert_eq!(
        minted, 1,
        "frozen: 0.5x price, 3 units in -> 1 share (rounded down, favoring the vault)"
    );
    println!("USDC rounding OK: 0.5x price, deposit=3 -> shares={minted} (floored from 1.4999)");
}

/// The only live mainnet vault that is **off par**, and therefore the only one on
/// which the offset retune (1 -> 10^6) changes any number at all.
///
/// Both other fixtures sit at `total_assets == supply`, where `(T+O)/(S+O)` is
/// identically 1.0 for every offset and both clamps are equalities — so they
/// cannot distinguish the old pricing from the new one, no matter how the offsets
/// move. That made the retune's effect on real state unobserved by this file.
///
/// It also covers a second state the other two miss: `local_aum == 0` with the
/// whole balance in `deployed_aum`, i.e. fully deployed with an empty reserve.
///
/// Snapshot: mainnet ~slot 437,402,306.
#[test]
fn offpar_vault_real_state_is_above_par_and_prices_inside_pro_rata() {
    let mut svm = new_svm();
    let spl = pk(SPL_TOKEN);

    let vault_state = pk("EhuUbTe3RcowbE9zp5TeaUpMjX5pEYWj6yEtQ2tAQKH7");
    let share_mint = pk("5xwaaHP3vM2Part8bZsYBg3hjDkvTeSddUmwhyXHiNuS");
    let vault_ata = pk("BQUFduRuJHcmZup9CVRuLwT4UKjw2cQpYXf9t3xEujuv");
    let deposit_mint = pk("CD4VCkNGiFc6a6iPdaXZr6WyvRnQqvGE8DQvyk7bCK5n");

    inject(
        &mut svm,
        vault_state,
        august_vault::ID,
        include_bytes!("fixtures/offpar_vault_state.bin").to_vec(),
    );
    inject(
        &mut svm,
        share_mint,
        spl,
        include_bytes!("fixtures/offpar_share_mint.bin").to_vec(),
    );
    inject(
        &mut svm,
        vault_ata,
        spl,
        include_bytes!("fixtures/offpar_vault_ata.bin").to_vec(),
    );
    inject(
        &mut svm,
        deposit_mint,
        spl,
        include_bytes!("fixtures/offpar_mint.bin").to_vec(),
    );

    let vs = read_vault_state(&svm, &vault_state);
    assert_eq!(vs.deposit_mint, deposit_mint);
    assert_eq!(vs.share_mint, share_mint);
    assert_live_vault_offset_is_legacy_zero(&vs);
    assert_eq!(vs.local_aum, OFFPAR_LOCAL_AUM, "snapshot local_aum");
    assert_eq!(
        vs.deployed_aum, OFFPAR_DEPLOYED_AUM,
        "snapshot deployed_aum"
    );

    // Fully deployed: the reserve is empty and every asset is reported off-vault.
    // Neither other fixture exercises this.
    assert_eq!(
        token_amount(&svm, &vault_ata),
        vs.local_aum,
        "invariant: reserve balance == local_aum accounting"
    );
    assert_eq!(token_amount(&svm, &vault_ata), 0, "reserve is empty");

    let supply = SplMint::unpack(&svm.get_account(&share_mint).unwrap().data[..SplMint::LEN])
        .unwrap()
        .supply;
    assert_eq!(supply, OFFPAR_SUPPLY, "snapshot share supply");

    let total_assets = vs.total_assets().unwrap();
    assert_eq!(total_assets, OFFPAR_LOCAL_AUM + OFFPAR_DEPLOYED_AUM);
    // The whole point of this fixture. If a refresh ever lands it at par, the
    // assertions below stop testing anything and this fails to say so.
    assert!(
        total_assets > supply,
        "this fixture exists to cover an ABOVE-par vault (total_assets={total_assets}, \
         supply={supply}); at par every offset gives the same answer"
    );

    // ---------------------------------------------------------------------
    // EXERCISE THE LOADED BYTECODE. Everything above reads state; the assertions
    // that matter must run the deposit/redeem HANDLERS in the .so that
    // `new_svm()` loaded. Calling `vs.shares_for_deposit(..)` here instead would
    // only re-run host-side Rust: a regression in the handler's legacy-offset
    // wiring — the very thing this fixture exists to catch — would leave a
    // helper-only test green.
    // ---------------------------------------------------------------------
    let one = 1_000_000_000u64; // 1 whole token / share (both mints are 9 decimals)

    assert_eq!(
        vs.withdrawal_fee, 0,
        "fixture assumption: this vault charges no withdrawal fee, so the redeem \
         payout below is gross == net"
    );

    let user = Keypair::new();
    svm.airdrop(&user.pubkey(), 1_000_000_000).unwrap();
    let user_tokens = Keypair::new().pubkey();
    let user_shares = Keypair::new().pubkey();
    inject(
        &mut svm,
        user_tokens,
        spl,
        packed_token(deposit_mint, user.pubkey(), one),
    );
    inject(
        &mut svm,
        user_shares,
        spl,
        packed_token(share_mint, user.pubkey(), 0),
    );

    // --- DEPOSIT. Above par the offset term is the LARGER, so `max` takes it and
    // the depositor mints slightly more than pro rata.
    let pro_rata_shares = (one as u128 * supply as u128 / total_assets as u128) as u64;
    let expected_shares = ref_shares(supply, total_assets, one);
    let ix = Instruction {
        program_id: august_vault::ID,
        accounts: ix_accounts::Deposit {
            vault_state,
            vault_token_ata: vault_ata,
            sender_token_account: user_tokens,
            sender_share_account: user_shares,
            share_mint,
            deposit_mint,
            signer: user.pubkey(),
            token_program: spl,
        }
        .to_account_metas(None),
        data: ix_data::Deposit { amount: one }.data(),
    };
    let bh = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&user.pubkey()), &[&user], bh);
    let res = svm.send_transaction(tx);
    assert!(
        res.is_ok(),
        "deposit against real off-par vault state failed: {:?}",
        res.err()
    );

    let minted = token_amount(&svm, &user_shares);
    assert_eq!(minted, expected_shares, "independent formula");
    assert_eq!(minted, 500_000_000, "frozen literal");
    assert!(
        minted > pro_rata_shares,
        "above par the offset term must be the larger (minted={minted}, \
         pro_rata={pro_rata_shares})"
    );
    // The retune's effect on live pricing, now backed by an executed transaction
    // rather than arithmetic: the DEPLOYED build (fca11d73) used offsets of 1,
    // which round to pro rata at this scale, so it would have minted 1,244 fewer
    // base units (+0.00025% for the depositor).
    assert_eq!(
        minted - 499_998_756,
        1_244,
        "deposit shift vs the deployed build"
    );

    let vs_after = read_vault_state(&svm, &vault_state);
    assert_eq!(
        vs_after.local_aum,
        OFFPAR_LOCAL_AUM + one,
        "local_aum += deposit"
    );
    assert_eq!(
        vs_after.deployed_aum, OFFPAR_DEPLOYED_AUM,
        "deployed_aum unchanged by a user deposit"
    );
    assert_eq!(
        token_amount(&svm, &vault_ata),
        one,
        "the deposit is the vault's entire reserve — it started empty"
    );

    // --- REDEEM straight back out. The deposit above is what makes this possible:
    // the reserve started at zero (fully deployed), so there was nothing to pay a
    // redemption from.
    let fee_recipient_acct = Keypair::new().pubkey();
    inject(
        &mut svm,
        fee_recipient_acct,
        spl,
        packed_token(deposit_mint, vs_after.fee_recipient, 0),
    );
    let supply_after = SplMint::unpack(&svm.get_account(&share_mint).unwrap().data[..SplMint::LEN])
        .unwrap()
        .supply;
    let total_after = vs_after.total_assets().unwrap();
    let expected_assets = ref_assets(supply_after, total_after, minted);
    let pro_rata_assets = (minted as u128 * total_after as u128 / supply_after as u128) as u64;

    let redeem_ix = Instruction {
        program_id: august_vault::ID,
        accounts: ix_accounts::Redeem {
            vault_state,
            vault_deposit_ata: vault_ata,
            sender_token_account: user_tokens,
            sender_share_account: user_shares,
            fee_recipient_account: fee_recipient_acct,
            share_mint,
            deposit_mint,
            signer: user.pubkey(),
            token_program: spl,
        }
        .to_account_metas(None),
        data: ix_data::Redeem { shares: minted }.data(),
    };
    let bh = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[redeem_ix], Some(&user.pubkey()), &[&user], bh);
    let res = svm.send_transaction(tx);
    assert!(
        res.is_ok(),
        "redeem against real off-par vault state failed: {:?}",
        res.err()
    );

    let returned = token_amount(&svm, &user_tokens);
    assert_eq!(returned, expected_assets, "independent formula");
    assert_eq!(returned, 999_999_998, "frozen literal");
    // The direction that makes a share-burn attack unprofitable: above par the
    // holder is paid strictly INSIDE pro rata.
    assert!(
        returned < pro_rata_assets,
        "above par the offset term must be the smaller (returned={returned}, \
         pro_rata={pro_rata_assets})"
    );
    // And the round trip must not extract value, against real state.
    assert!(
        returned <= one,
        "round trip extracted value: paid {one}, took {returned}"
    );
    assert_eq!(
        token_amount(&svm, &user_shares),
        0,
        "all shares burned on redeem"
    );
    let vs_final = read_vault_state(&svm, &vault_state);
    assert_eq!(
        vs_final.local_aum,
        token_amount(&svm, &vault_ata),
        "local_aum must still equal the reserve after the round trip"
    );

    println!(
        "off-par fork OK: supply={supply} total_assets={total_assets} \
         deposit({one}) -> {minted} shares (pro_rata {pro_rata_shares}); \
         redeem back -> {returned} (pro_rata {pro_rata_assets})"
    );
}
