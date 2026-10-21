//! LiteSVM behind the Solana 2.x types the rest of this crate uses.
//!
//! LiteSVM 0.18 (the first line that runs SBPFv3) is built on the Solana 3/4
//! crates, while the programs, Anchor 0.31 and spl are on 2.x. Rather than
//! convert at every call site, this wrapper exposes the handful of LiteSVM
//! methods the tests use in 2.x terms. Transactions and errors cross by their
//! bincode wire form, which both lines keep identical; accounts, keys and hashes
//! cross field by field.

use litesvm::{error::LiteSVMError, LiteSVM};
use solana_account_v4::Account as AccountV4;
use solana_address_v2::Address;
use solana_sdk::{
    account::Account, hash::Hash, pubkey::Pubkey, sysvar::Sysvar, transaction::Transaction,
    transaction::TransactionError,
};

/// The parts of LiteSVM's metadata the tests read.
#[derive(Clone, Debug)]
pub struct TransactionMetadata {
    pub logs: Vec<String>,
    pub compute_units_consumed: u64,
    /// The data of every inner instruction, in execution order.
    pub inner_instruction_data: Vec<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct FailedTransactionMetadata {
    pub err: TransactionError,
    pub meta: TransactionMetadata,
}

pub type TransactionResult = Result<TransactionMetadata, FailedTransactionMetadata>;

#[derive(Clone)]
pub struct Svm(LiteSVM);

impl Default for Svm {
    fn default() -> Self {
        Self::new()
    }
}

impl Svm {
    pub fn new() -> Self {
        Self(LiteSVM::new())
    }

    pub fn get_account(&self, address: &Pubkey) -> Option<Account> {
        self.0.get_account(&address_of(address)).map(|a| Account {
            lamports: a.lamports,
            data: a.data,
            owner: Pubkey::new_from_array(a.owner.to_bytes()),
            executable: a.executable,
            rent_epoch: a.rent_epoch,
        })
    }

    pub fn set_account(&mut self, address: Pubkey, account: Account) -> Result<(), LiteSVMError> {
        let account = AccountV4 {
            lamports: account.lamports,
            data: account.data,
            owner: address_of(&account.owner),
            executable: account.executable,
            rent_epoch: account.rent_epoch,
        };
        self.0.set_account(address_of(&address), account)
    }

    pub fn get_balance(&self, address: &Pubkey) -> Option<u64> {
        self.0.get_balance(&address_of(address))
    }

    pub fn add_program(&mut self, program_id: Pubkey, bytes: &[u8]) -> Result<(), LiteSVMError> {
        self.0.add_program(address_of(&program_id), bytes)
    }

    pub fn airdrop(&mut self, address: &Pubkey, lamports: u64) -> TransactionResult {
        convert_result(self.0.airdrop(&address_of(address), lamports))
    }

    pub fn latest_blockhash(&self) -> Hash {
        Hash::new_from_array(self.0.latest_blockhash().to_bytes())
    }

    pub fn expire_blockhash(&mut self) {
        self.0.expire_blockhash();
    }

    pub fn send_transaction(&mut self, tx: Transaction) -> TransactionResult {
        let tx: solana_transaction_v4::Transaction = recode(&tx);
        convert_result(self.0.send_transaction(tx))
    }

    /// Read through the sysvar account rather than LiteSVM's typed getter, whose
    /// bound is the 3.x `Sysvar` trait. The account bytes are the bincode form.
    pub fn get_sysvar<T: Sysvar>(&self) -> T {
        let account = self
            .get_account(&T::id())
            .expect("sysvar account is installed");
        bincode::deserialize(&account.data).expect("decode sysvar")
    }

    /// LiteSVM refreshes its sysvar cache whenever a sysvar account is written.
    pub fn set_sysvar<T: Sysvar>(&mut self, sysvar: &T) {
        let account = Account {
            lamports: 1,
            data: bincode::serialize(sysvar).expect("encode sysvar"),
            owner: solana_sdk::sysvar::ID,
            executable: false,
            rent_epoch: 0,
        };
        self.set_account(T::id(), account).expect("set sysvar");
    }
}

fn address_of(pubkey: &Pubkey) -> Address {
    Address::new_from_array(pubkey.to_bytes())
}

/// Re-reads a value as the same type from the other Solana line. Only for types
/// whose bincode form is wire format, so it cannot drift between the two.
fn recode<A: serde::Serialize, B: serde::de::DeserializeOwned>(value: &A) -> B {
    let bytes = bincode::serialize(value).expect("encode for the LiteSVM boundary");
    match bincode::deserialize(&bytes) {
        Ok(v) => v,
        Err(e) => panic!(
            "{} has no {} equivalent: {e}",
            std::any::type_name::<A>(),
            std::any::type_name::<B>()
        ),
    }
}

fn convert_result(result: litesvm::types::TransactionResult) -> TransactionResult {
    let meta = |m: litesvm::types::TransactionMetadata| TransactionMetadata {
        inner_instruction_data: m
            .inner_instructions
            .into_iter()
            .flatten()
            .map(|inner| inner.instruction.data)
            .collect(),
        logs: m.logs,
        compute_units_consumed: m.compute_units_consumed,
    };
    match result {
        Ok(m) => Ok(meta(m)),
        Err(f) => {
            let err: solana_transaction_error_v3::TransactionError = f.err;
            Err(FailedTransactionMetadata {
                err: recode(&err),
                meta: meta(f.meta),
            })
        }
    }
}
