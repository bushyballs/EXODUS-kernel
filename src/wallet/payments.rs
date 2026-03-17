use crate::sync::Mutex;
/// NFC payment processing for Genesis
///
/// Contactless payments, card emulation, tokenization,
/// transaction management, receipts.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum PaymentStatus {
    Pending,
    Authorized,
    Captured,
    Declined,
    Refunded,
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PaymentMethod {
    NfcContactless,
    QrCode,
    InApp,
    P2P,
    Online,
}

#[derive(Clone, Copy)]
struct Transaction {
    id: u32,
    amount_cents: u64,
    currency: [u8; 3],
    merchant_hash: u64,
    method: PaymentMethod,
    status: PaymentStatus,
    timestamp: u64,
    token_id: u32,
}

struct PaymentToken {
    id: u32,
    card_last_four: [u8; 4],
    network: CardNetwork,
    token_hash: u64,
    active: bool,
    expiry_month: u8,
    expiry_year: u16,
    transaction_count: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CardNetwork {
    Visa,
    Mastercard,
    Amex,
    Discover,
    UnionPay,
}

struct PaymentEngine {
    transactions: Vec<Transaction>,
    tokens: Vec<PaymentToken>,
    next_tx_id: u32,
    next_token_id: u32,
    daily_limit_cents: u64,
    daily_spent_cents: u64,
    default_token: Option<u32>,
    biometric_required: bool,
}

static PAYMENTS: Mutex<Option<PaymentEngine>> = Mutex::new(None);

impl PaymentEngine {
    fn new() -> Self {
        PaymentEngine {
            transactions: Vec::new(),
            tokens: Vec::new(),
            next_tx_id: 1,
            next_token_id: 1,
            daily_limit_cents: 500000, // $5000
            daily_spent_cents: 0,
            default_token: None,
            biometric_required: true,
        }
    }

    fn add_token(&mut self, last_four: [u8; 4], network: CardNetwork, month: u8, year: u16) -> u32 {
        let id = self.next_token_id;
        self.next_token_id = self.next_token_id.saturating_add(1);
        let mut hash = 0u64;
        for &b in &last_four {
            hash = hash.wrapping_mul(31).wrapping_add(b as u64);
        }
        hash = hash.wrapping_mul(31).wrapping_add(id as u64);
        self.tokens.push(PaymentToken {
            id,
            card_last_four: last_four,
            network,
            token_hash: hash,
            active: true,
            expiry_month: month,
            expiry_year: year,
            transaction_count: 0,
        });
        if self.default_token.is_none() {
            self.default_token = Some(id);
        }
        id
    }

    fn process_payment(
        &mut self,
        amount_cents: u64,
        method: PaymentMethod,
        timestamp: u64,
    ) -> Option<u32> {
        // Check daily limit
        if self.daily_spent_cents + amount_cents > self.daily_limit_cents {
            return None;
        }
        let token_id = self.default_token?;
        let tx_id = self.next_tx_id;
        self.next_tx_id = self.next_tx_id.saturating_add(1);
        self.transactions.push(Transaction {
            id: tx_id,
            amount_cents,
            currency: *b"USD",
            merchant_hash: 0,
            method,
            status: PaymentStatus::Authorized,
            timestamp,
            token_id,
        });
        self.daily_spent_cents += amount_cents;
        if let Some(token) = self.tokens.iter_mut().find(|t| t.id == token_id) {
            token.transaction_count = token.transaction_count.saturating_add(1);
        }
        Some(tx_id)
    }
}

pub fn init() {
    let mut p = PAYMENTS.lock();
    *p = Some(PaymentEngine::new());
    serial_println!("    Wallet: NFC payment processing ready");
}
