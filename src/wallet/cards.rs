use crate::sync::Mutex;
/// Card management for Genesis wallet
///
/// Loyalty cards, gift cards, boarding passes,
/// event tickets, membership cards, digital IDs.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum CardType {
    Loyalty,
    GiftCard,
    BoardingPass,
    EventTicket,
    MembershipCard,
    DigitalId,
    Insurance,
    StudentId,
}

struct WalletCard {
    id: u32,
    card_type: CardType,
    issuer: [u8; 32],
    issuer_len: usize,
    barcode_data: [u8; 64],
    barcode_len: usize,
    balance_cents: Option<u64>, // for gift cards
    points: Option<u32>,        // for loyalty
    expiry: Option<u64>,
    last_used: u64,
    use_count: u32,
}

struct CardManager {
    cards: Vec<WalletCard>,
    next_id: u32,
}

static CARDS: Mutex<Option<CardManager>> = Mutex::new(None);

impl CardManager {
    fn new() -> Self {
        CardManager {
            cards: Vec::new(),
            next_id: 1,
        }
    }

    fn add_card(&mut self, card_type: CardType, issuer: &[u8]) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut iss = [0u8; 32];
        let ilen = issuer.len().min(32);
        iss[..ilen].copy_from_slice(&issuer[..ilen]);
        self.cards.push(WalletCard {
            id,
            card_type,
            issuer: iss,
            issuer_len: ilen,
            barcode_data: [0; 64],
            barcode_len: 0,
            balance_cents: None,
            points: None,
            expiry: None,
            last_used: 0,
            use_count: 0,
        });
        id
    }

    fn use_card(&mut self, card_id: u32, timestamp: u64) -> bool {
        if let Some(card) = self.cards.iter_mut().find(|c| c.id == card_id) {
            if let Some(exp) = card.expiry {
                if timestamp > exp {
                    return false;
                }
            }
            card.last_used = timestamp;
            card.use_count = card.use_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn get_nearby_usable(&self, timestamp: u64) -> Vec<u32> {
        self.cards
            .iter()
            .filter(|c| c.expiry.map_or(true, |e| timestamp < e))
            .map(|c| c.id)
            .collect()
    }
}

pub fn init() {
    let mut c = CARDS.lock();
    *c = Some(CardManager::new());
    serial_println!("    Wallet: card manager (loyalty, passes, IDs) ready");
}
