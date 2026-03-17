pub mod ai_wallet;
pub mod cards;
pub mod crypto_wallet;
/// Digital wallet and payments for Genesis
///
/// NFC payments, card tokenization, P2P transfers,
/// loyalty cards, transit passes, digital ID,
/// cryptocurrency wallet, and AI fraud detection.
///
/// Original implementation for Hoags OS.
pub mod payments;
pub mod transit;

use crate::{serial_print, serial_println};

pub fn init() {
    payments::init();
    cards::init();
    transit::init();
    crypto_wallet::init();
    ai_wallet::init();
    serial_println!("  Wallet initialized (NFC pay, cards, transit, crypto, AI fraud)");
}
