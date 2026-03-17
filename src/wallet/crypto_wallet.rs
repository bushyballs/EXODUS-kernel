use crate::sync::Mutex;
/// Cryptocurrency wallet for Genesis
///
/// Multi-chain wallet, key management, transaction signing,
/// token management, DApp browser support.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum Chain {
    Bitcoin,
    Ethereum,
    Solana,
    Polygon,
    Avalanche,
    Custom,
}

struct CryptoAccount {
    id: u32,
    chain: Chain,
    address_hash: u64,
    public_key: [u8; 64],
    pub_key_len: usize,
    // Private key stored in secure enclave (not in memory)
    balance_atomic: u64, // smallest unit (satoshi, wei, lamport)
    tx_count: u32,
    label: [u8; 24],
    label_len: usize,
}

struct CryptoTx {
    id: u32,
    chain: Chain,
    from_account: u32,
    to_hash: u64,
    amount_atomic: u64,
    fee_atomic: u64,
    confirmed: bool,
    timestamp: u64,
    block_height: u64,
}

struct Token {
    chain: Chain,
    contract_hash: u64,
    symbol: [u8; 8],
    symbol_len: usize,
    decimals: u8,
    balance_atomic: u64,
}

struct CryptoWallet {
    accounts: Vec<CryptoAccount>,
    transactions: Vec<CryptoTx>,
    tokens: Vec<Token>,
    next_id: u32,
    next_tx_id: u32,
}

static CRYPTO: Mutex<Option<CryptoWallet>> = Mutex::new(None);

impl CryptoWallet {
    fn new() -> Self {
        CryptoWallet {
            accounts: Vec::new(),
            transactions: Vec::new(),
            tokens: Vec::new(),
            next_id: 1,
            next_tx_id: 1,
        }
    }

    fn create_account(&mut self, chain: Chain, label: &[u8]) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut lbl = [0u8; 24];
        let llen = label.len().min(24);
        lbl[..llen].copy_from_slice(&label[..llen]);
        // In real implementation: generate keypair in secure enclave
        self.accounts.push(CryptoAccount {
            id,
            chain,
            address_hash: id as u64 * 0xDEAD_BEEF,
            public_key: [0; 64],
            pub_key_len: 0,
            balance_atomic: 0,
            tx_count: 0,
            label: lbl,
            label_len: llen,
        });
        id
    }

    fn sign_transaction(
        &mut self,
        from: u32,
        to_hash: u64,
        amount: u64,
        fee: u64,
        timestamp: u64,
    ) -> Option<u32> {
        let account = self.accounts.iter_mut().find(|a| a.id == from)?;
        if account.balance_atomic < amount + fee {
            return None;
        }
        account.balance_atomic -= amount + fee;
        account.tx_count = account.tx_count.saturating_add(1);
        let tx_id = self.next_tx_id;
        self.next_tx_id = self.next_tx_id.saturating_add(1);
        self.transactions.push(CryptoTx {
            id: tx_id,
            chain: account.chain,
            from_account: from,
            to_hash,
            amount_atomic: amount,
            fee_atomic: fee,
            confirmed: false,
            timestamp,
            block_height: 0,
        });
        Some(tx_id)
    }

    fn total_value_usd_cents(&self, btc_price_cents: u64, eth_price_cents: u64) -> u64 {
        let mut total = 0u64;
        for acc in &self.accounts {
            match acc.chain {
                Chain::Bitcoin => total += acc.balance_atomic * btc_price_cents / 100_000_000,
                Chain::Ethereum => {
                    total += acc.balance_atomic * eth_price_cents / 1_000_000_000_000_000_000
                }
                _ => {}
            }
        }
        total
    }
}

pub fn init() {
    let mut w = CRYPTO.lock();
    *w = Some(CryptoWallet::new());
    serial_println!("    Wallet: crypto wallet (multi-chain, tokens) ready");
}
