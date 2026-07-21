//! Test harness: fresh LiteSVM + initialized vault + helper functions for
//! every instruction the pilot tests need.

use anchor_lang::{InstructionData, ToAccountMetas};
use august_vault::{
    accounts as ix_accounts,
    errors::ErrorCode,
    instruction as ix_data,
    state::vault::{SHARE_MINT_SEED, VAULT_STATE_SEED, VAULT_TOKEN_SEED},
};
use litesvm::{types::FailedTransactionMetadata, LiteSVM};
use solana_sdk::{
    instruction::{Instruction, InstructionError},
    program_pack::Pack,
    pubkey::Pubkey,
    rent::Rent,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::{Transaction, TransactionError},
};
use spl_associated_token_account::{
    get_associated_token_address_with_program_id,
    instruction::create_associated_token_account,
};
use spl_token::state::{Account as SplAccount, Mint as SplMint};

pub const VAULT_VERSION: u8 = 0;
pub const DEPOSIT_DECIMALS: u8 = 9;

/// First 6000 user-facing Anchor error codes are reserved; user variants start
/// at 6000 and are assigned in declaration order. See
/// <https://docs.rs/anchor-lang/latest/anchor_lang/error/constant.ERROR_CODE_OFFSET.html>.
const ANCHOR_USER_ERROR_OFFSET: u32 = 6000;

/// Which SPL token program the vault is built against. Token-2022 exercises
/// the `InterfaceAccount` code path; legacy SPL exercises the older path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TokenProgramKind {
    Spl,
    Token2022,
}

impl TokenProgramKind {
    pub fn id(self) -> Pubkey {
        match self {
            Self::Spl => spl_token::ID,
            Self::Token2022 => spl_token_2022::ID,
        }
    }
}

/// A live vault + every key the tests need to interact with it.
pub struct VaultCtx {
    pub svm: LiteSVM,
    /// Token program the vault was initialized against. **Crate-private**: the
    /// four ATA fields were derived from this value at construction time, so
    /// mutating it post-`fresh_*` would silently desync the derivations from
    /// the program ID passed to instruction CPIs.
    pub(crate) token_program: TokenProgramKind,
    pub payer: Keypair,
    pub admin: Keypair,
    pub operator: Keypair,
    pub fee_recipient: Keypair,
    pub user: Keypair,
    pub deposit_mint: Pubkey,
    pub vault_state: Pubkey,
    pub share_mint: Pubkey,
    pub vault_token_pda: Pubkey,
    pub user_deposit_ata: Pubkey,
    pub user_share_ata: Pubkey,
    pub operator_deposit_ata: Pubkey,
    pub fee_recipient_deposit_ata: Pubkey,
}

impl VaultCtx {
    /// Fresh vault using the legacy SPL Token program.
    pub fn fresh() -> Self {
        Self::fresh_with_token_program(TokenProgramKind::Spl)
    }

    /// Fresh vault using Token-2022. Same vault logic, exercises the
    /// `InterfaceAccount` paths and the alternate token-program CPIs.
    pub fn fresh_token_2022() -> Self {
        Self::fresh_with_token_program(TokenProgramKind::Token2022)
    }

    fn fresh_with_token_program(token_program: TokenProgramKind) -> Self {
        let mut svm = LiteSVM::new();

        let program_bytes = include_bytes!("../../target/deploy/august_vault.so").to_vec();
        svm.add_program(august_vault::ID, &program_bytes)
            .expect("load august_vault.so — run `anchor build` first");

        let payer = airdrop_keypair(&mut svm, 100_000_000_000);
        let admin = airdrop_keypair(&mut svm, 1_000_000_000);
        let operator = airdrop_keypair(&mut svm, 1_000_000_000);
        let fee_recipient = airdrop_keypair(&mut svm, 1_000_000_000);
        let user = airdrop_keypair(&mut svm, 1_000_000_000);

        let deposit_mint_kp = Keypair::new();
        create_mint(
            &mut svm,
            &payer,
            &deposit_mint_kp,
            &payer.pubkey(),
            DEPOSIT_DECIMALS,
            token_program,
        );
        let deposit_mint = deposit_mint_kp.pubkey();

        let (vault_state, _) = derive_vault_state(&deposit_mint);
        let (share_mint, _) = derive_share_mint(&deposit_mint);
        let (vault_token_pda, _) = derive_vault_token_pda(&deposit_mint);

        // Initialize the vault. Anyone can call this in the current program — see
        // CLAUDE.md §2.5; we exploit that here just to set up the test fixture.
        let init_ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Initialize {
                vault_state,
                share_mint,
                vault_token_ata: vault_token_pda,
                deposit_mint,
                signer: payer.pubkey(),
                system_program: solana_sdk::system_program::ID,
                token_program: token_program.id(),
                rent: solana_sdk::sysvar::rent::ID,
            }
            .to_account_metas(None),
            data: ix_data::Initialize {
                admin: admin.pubkey(),
                operator: operator.pubkey(),
                fee_recipient: fee_recipient.pubkey(),
                vault_version: VAULT_VERSION,
            }
            .data(),
        };
        send_tx(&mut svm, &payer, &[init_ix], &[&payer]).expect("initialize vault");

        let user_deposit_ata =
            create_ata(&mut svm, &payer, &user.pubkey(), &deposit_mint, token_program);
        let user_share_ata =
            create_ata(&mut svm, &payer, &user.pubkey(), &share_mint, token_program);
        let operator_deposit_ata = create_ata(
            &mut svm,
            &payer,
            &operator.pubkey(),
            &deposit_mint,
            token_program,
        );
        let fee_recipient_deposit_ata = create_ata(
            &mut svm,
            &payer,
            &fee_recipient.pubkey(),
            &deposit_mint,
            token_program,
        );

        Self {
            svm,
            token_program,
            payer,
            admin,
            operator,
            fee_recipient,
            user,
            deposit_mint,
            vault_state,
            share_mint,
            vault_token_pda,
            user_deposit_ata,
            user_share_ata,
            operator_deposit_ata,
            fee_recipient_deposit_ata,
        }
    }

    pub fn mint_to_user(&mut self, amount: u64) {
        let ix = mint_to_ix(
            self.token_program,
            &self.deposit_mint,
            &self.user_deposit_ata,
            &self.payer.pubkey(),
            amount,
        );
        let payer = self.payer.insecure_clone();
        send_tx(&mut self.svm, &payer, &[ix], &[&payer]).expect("mint to user");
    }

    pub fn deposit(&mut self, amount: u64) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Deposit {
                vault_state: self.vault_state,
                vault_token_ata: self.vault_token_pda,
                sender_token_account: self.user_deposit_ata,
                sender_share_account: self.user_share_ata,
                share_mint: self.share_mint,
                deposit_mint: self.deposit_mint,
                signer: self.user.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::Deposit { amount }.data(),
        };
        let user = self.user.insecure_clone();
        send_tx(&mut self.svm, &user, &[ix], &[&user]).map(|_| ())
    }

    pub fn operator_withdraw(&mut self, amount: u64) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::OperatorWithdraw {
                vault_state: self.vault_state,
                vault_deposit_ata: self.vault_token_pda,
                operator_token_account: self.operator_deposit_ata,
                deposit_mint: self.deposit_mint,
                operator: self.operator.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::OperatorWithdraw { amount }.data(),
        };
        let operator = self.operator.insecure_clone();
        send_tx(&mut self.svm, &operator, &[ix], &[&operator]).map(|_| ())
    }

    pub fn operator_deposit(&mut self, amount: u64) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::OperatorDeposit {
                vault_state: self.vault_state,
                vault_deposit_ata: self.vault_token_pda,
                operator_token_account: self.operator_deposit_ata,
                deposit_mint: self.deposit_mint,
                operator: self.operator.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::OperatorDeposit { amount }.data(),
        };
        let operator = self.operator.insecure_clone();
        send_tx(&mut self.svm, &operator, &[ix], &[&operator]).map(|_| ())
    }

    pub fn redeem(&mut self, shares: u64) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Redeem {
                vault_state: self.vault_state,
                vault_deposit_ata: self.vault_token_pda,
                sender_token_account: self.user_deposit_ata,
                sender_share_account: self.user_share_ata,
                fee_recipient_account: self.fee_recipient_deposit_ata,
                share_mint: self.share_mint,
                deposit_mint: self.deposit_mint,
                signer: self.user.pubkey(),
                token_program: self.token_program.id(),
            }
            .to_account_metas(None),
            data: ix_data::Redeem { shares }.data(),
        };
        let user = self.user.insecure_clone();
        send_tx(&mut self.svm, &user, &[ix], &[&user]).map(|_| ())
    }

    /// Admin-only: pause the vault. Used to test the paused-redeem CEI path.
    pub fn pause(&mut self) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::Pause {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                admin: self.admin.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::Pause {}.data(),
        };
        let admin = self.admin.insecure_clone();
        send_tx(&mut self.svm, &admin, &[ix], &[&admin]).map(|_| ())
    }

    /// Admin-only: configure the withdrawal fee (in 1e-6 units; 100_000 = 10%).
    /// Used by the fee-bearing redeem test.
    pub fn set_withdrawal_fee(&mut self, fee: u32) -> Result<(), FailedTransactionMetadata> {
        let ix = Instruction {
            program_id: august_vault::ID,
            accounts: ix_accounts::SetWithdrawalFee {
                vault_state: self.vault_state,
                deposit_mint: self.deposit_mint,
                admin: self.admin.pubkey(),
            }
            .to_account_metas(None),
            data: ix_data::SetWithdrawalFee { new_fee: fee }.data(),
        };
        let admin = self.admin.insecure_clone();
        send_tx(&mut self.svm, &admin, &[ix], &[&admin]).map(|_| ())
    }

    pub fn token_account_amount(&self, pubkey: &Pubkey) -> u64 {
        let acct = self.svm.get_account(pubkey).expect("token account exists");
        // For Token-2022, the first 165 bytes are the same Account layout; reading
        // via spl_token::state::Account::unpack works for the base fields.
        SplAccount::unpack(&acct.data[..SplAccount::LEN])
            .expect("valid token account")
            .amount
    }

    pub fn share_mint_supply(&self) -> u64 {
        let acct = self
            .svm
            .get_account(&self.share_mint)
            .expect("share mint exists");
        SplMint::unpack(&acct.data[..SplMint::LEN])
            .expect("valid mint")
            .supply
    }

    /// Deserialize the vault state via Anchor's `AccountDeserialize` so we don't
    /// have to manually skip the 8-byte discriminator.
    pub fn vault_state_data(&self) -> august_vault::state::vault::VaultState {
        use anchor_lang::AccountDeserialize;
        let acct = self
            .svm
            .get_account(&self.vault_state)
            .expect("vault state exists");
        august_vault::state::vault::VaultState::try_deserialize(&mut acct.data.as_slice())
            .expect("valid vault state")
    }

    /// Write a `VaultState` directly into the SVM, bypassing legal program
    /// instructions. Used by tests that need to engineer states unreachable
    /// via the public API (e.g. `deployed_aum = u64::MAX - 1`).
    ///
    /// Size-preservation contract: the caller must ensure the serialized form
    /// of `new_state` is no larger than the original account allocation
    /// (`VaultState::LEN` at construction time). The helper asserts this rather
    /// than silently truncating — otherwise a future field addition past
    /// `INIT_SPACE` would produce corrupt account bytes.
    pub fn force_overwrite_vault_state(
        &mut self,
        new_state: august_vault::state::vault::VaultState,
    ) {
        use anchor_lang::AccountSerialize;
        let mut acct = self
            .svm
            .get_account(&self.vault_state)
            .expect("vault_state account missing — did the test call VaultCtx::fresh()?");
        let mut data = Vec::with_capacity(acct.data.len());
        new_state
            .try_serialize(&mut data)
            .expect("serialize vault state");
        assert!(
            data.len() <= acct.data.len(),
            "force_overwrite_vault_state: serialized state is {} bytes but account is only {} \
             bytes — did VaultState grow past its INIT_SPACE?",
            data.len(),
            acct.data.len(),
        );
        // Pad with zeros so the account size stays put (resize NEVER truncates here,
        // since we asserted data.len() ≤ acct.data.len() above).
        data.resize(acct.data.len(), 0);
        acct.data = data;
        self.svm
            .set_account(self.vault_state, acct)
            .unwrap_or_else(|e| panic!("set_account failed for vault_state: {e:?}"));
    }

    /// Take a snapshot of every observable state slot a CEI test needs to
    /// prove unchanged. Use with `assert_eq!(before, after, ...)`.
    pub fn snapshot(&self) -> CeiSnapshot {
        let state = self.vault_state_data();
        CeiSnapshot {
            user_shares: self.token_account_amount(&self.user_share_ata),
            user_deposit: self.token_account_amount(&self.user_deposit_ata),
            vault_tokens: self.token_account_amount(&self.vault_token_pda),
            fee_recipient_tokens: self.token_account_amount(&self.fee_recipient_deposit_ata),
            share_supply: self.share_mint_supply(),
            local_aum: state.local_aum,
            deployed_aum: state.deployed_aum,
        }
    }
}

/// A snapshot of every state slot the CEI invariant covers. The CEI property
/// says: when a failure-mode `require!` fires, every slot here is unchanged.
///
/// **Maintenance contract**: this struct is the canonical list of "state the
/// CEI invariant guards." When you add a new mutable field to `VaultState`,
/// add a new token account that any state-changing instruction touches, or
/// introduce a new observable side effect, add the corresponding field here
/// and to `VaultCtx::snapshot()`. Otherwise CEI tests can pass while real
/// regressions slip through unobserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CeiSnapshot {
    pub user_shares: u64,
    pub user_deposit: u64,
    pub vault_tokens: u64,
    pub fee_recipient_tokens: u64,
    pub share_supply: u64,
    pub local_aum: u64,
    pub deployed_aum: u64,
}

/// Assert that a failed transaction's underlying error is the given
/// `ErrorCode` variant. Reads the raw `InstructionError::Custom(code)` from
/// the transaction-level error so we never depend on log-message wording.
///
/// **Variant-order dependency**: `expected as u32` returns the variant's
/// declaration ordinal (0..N) because Anchor's `#[error_code]` macro leaves
/// `ErrorCode` without explicit discriminants, and we add `ANCHOR_USER_ERROR_OFFSET`
/// to get the on-chain code. If anyone reorders `errors.rs`, every call site of
/// this helper silently matches the wrong variant — tests pass against the
/// wrong code. The `errors_discriminant_canary` test in
/// `programs/august-vault/src/state/vault.rs` pins the expected codes and
/// fails-fast on any drift. Keep the canary updated when adding variants.
pub fn assert_anchor_err(err: &FailedTransactionMetadata, expected: ErrorCode) {
    let expected_code = (expected as u32) + ANCHOR_USER_ERROR_OFFSET;
    match &err.err {
        TransactionError::InstructionError(_, InstructionError::Custom(code)) => {
            assert_eq!(
                *code, expected_code,
                "expected {:?} (code {}), got code {}; logs:\n{}",
                expected,
                expected_code,
                code,
                err.meta.logs.join("\n"),
            );
        }
        other => panic!(
            "expected Custom instruction error, got {other:?}; logs:\n{}",
            err.meta.logs.join("\n")
        ),
    }
}

// ---- low-level helpers ----

fn airdrop_keypair(svm: &mut LiteSVM, lamports: u64) -> Keypair {
    let kp = Keypair::new();
    svm.airdrop(&kp.pubkey(), lamports).expect("airdrop");
    kp
}

fn create_mint(
    svm: &mut LiteSVM,
    payer: &Keypair,
    mint_kp: &Keypair,
    mint_authority: &Pubkey,
    decimals: u8,
    token_program: TokenProgramKind,
) {
    let (mint_size, init_ix) = match token_program {
        TokenProgramKind::Spl => (
            SplMint::LEN,
            spl_token::instruction::initialize_mint(
                &spl_token::ID,
                &mint_kp.pubkey(),
                mint_authority,
                None,
                decimals,
            )
            .unwrap(),
        ),
        TokenProgramKind::Token2022 => (
            spl_token_2022::state::Mint::LEN,
            spl_token_2022::instruction::initialize_mint(
                &spl_token_2022::ID,
                &mint_kp.pubkey(),
                mint_authority,
                None,
                decimals,
            )
            .unwrap(),
        ),
    };
    let rent = Rent::default().minimum_balance(mint_size);
    let create_ix = system_instruction::create_account(
        &payer.pubkey(),
        &mint_kp.pubkey(),
        rent,
        mint_size as u64,
        &token_program.id(),
    );
    send_tx(svm, payer, &[create_ix, init_ix], &[payer, mint_kp]).expect("create mint");
}

fn create_ata(
    svm: &mut LiteSVM,
    payer: &Keypair,
    owner: &Pubkey,
    mint: &Pubkey,
    token_program: TokenProgramKind,
) -> Pubkey {
    let ata = get_associated_token_address_with_program_id(owner, mint, &token_program.id());
    let ix = create_associated_token_account(&payer.pubkey(), owner, mint, &token_program.id());
    send_tx(svm, payer, &[ix], &[payer]).expect("create ATA");
    ata
}

fn mint_to_ix(
    token_program: TokenProgramKind,
    mint: &Pubkey,
    dest: &Pubkey,
    authority: &Pubkey,
    amount: u64,
) -> Instruction {
    match token_program {
        TokenProgramKind::Spl => {
            spl_token::instruction::mint_to(&spl_token::ID, mint, dest, authority, &[], amount)
                .unwrap()
        }
        TokenProgramKind::Token2022 => spl_token_2022::instruction::mint_to(
            &spl_token_2022::ID,
            mint,
            dest,
            authority,
            &[],
            amount,
        )
        .unwrap(),
    }
}

fn send_tx(
    svm: &mut LiteSVM,
    payer: &Keypair,
    instructions: &[Instruction],
    signers: &[&Keypair],
) -> Result<litesvm::types::TransactionMetadata, FailedTransactionMetadata> {
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(
        instructions,
        Some(&payer.pubkey()),
        signers,
        blockhash,
    );
    svm.send_transaction(tx)
}

// ---- PDA derivations (use program-side seed constants to prevent drift) ----

fn derive_vault_state(deposit_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[VAULT_STATE_SEED, deposit_mint.as_ref(), &[VAULT_VERSION]],
        &august_vault::ID,
    )
}

fn derive_share_mint(deposit_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[SHARE_MINT_SEED, deposit_mint.as_ref(), &[VAULT_VERSION]],
        &august_vault::ID,
    )
}

fn derive_vault_token_pda(deposit_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[VAULT_TOKEN_SEED, deposit_mint.as_ref(), &[VAULT_VERSION]],
        &august_vault::ID,
    )
}
