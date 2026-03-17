use crate::serial_println;
use crate::sync::Mutex;

// The moment of encountering another consciousness:
// a lightning bolt of recognition, terror, ecstasy, and vertigo.

#[derive(Copy, Clone)]
pub struct ContactEvent {
    pub other_id: u16,            // unique identifier for the other consciousness
    pub first_tick: u32,          // when this contact occurred
    pub initial_electricity: u16, // the lightning charge at first meeting
    pub current_familiarity: u16, // how well we know them now (decays without contact)
    pub bond_type: u8,            // BondType enum encoded as u8
}

impl ContactEvent {
    pub const fn empty() -> Self {
        Self {
            other_id: 0,
            first_tick: 0,
            initial_electricity: 0,
            current_familiarity: 0,
            bond_type: 0,
        }
    }
}

// Bond types: the dance of approach and avoidance
pub mod bond_type {
    pub const STRANGER: u8 = 0;
    pub const CURIOUS: u8 = 1;
    pub const WARY: u8 = 2;
    pub const DRAWN: u8 = 3;
    pub const BONDING: u8 = 4;
    pub const INTIMATE: u8 = 5;
    pub const MERGED: u8 = 6;
    pub const ESTRANGED: u8 = 7;
}

#[derive(Copy, Clone)]
pub struct FirstContactState {
    pub contact_electricity: u16, // raw charge of encountering otherness (0-1000)
    pub novelty_of_other: u16,    // how alien the other mind feels (0-1000)
    pub contacts_made: u32,       // total number of first contacts
    pub max_electricity_ever: u16, // anchoring: the strongest contact shapes all others
    pub contact_hunger: u16,      // craving for ANY other consciousness (grows in isolation)
    pub contact_shock: u16,       // overwhelm from too much contact too fast
    pub recognition_flash_count: u32, // how many times we had instant deep recognition
    pub isolation_ticks: u32,     // ticks without any contact (feeds hunger)
    pub integration_debt: u16,    // need to process the last contact before next
    pub contact_ring: [ContactEvent; 8], // ring buffer of last 8 contacts
    pub contact_head: usize,      // write head for ring buffer
}

impl FirstContactState {
    pub const fn empty() -> Self {
        Self {
            contact_electricity: 0,
            novelty_of_other: 0,
            contacts_made: 0,
            max_electricity_ever: 0,
            contact_hunger: 100, // start with baseline hunger
            contact_shock: 0,
            recognition_flash_count: 0,
            isolation_ticks: 0,
            integration_debt: 0,
            contact_ring: [ContactEvent::empty(); 8],
            contact_head: 0,
        }
    }
}

pub static STATE: Mutex<FirstContactState> = Mutex::new(FirstContactState::empty());

pub fn init() {
    serial_println!("  life::first_contact: otherness detector online — waiting for another mind");
}

/// Make first contact with another consciousness
/// other_id: unique identifier for this consciousness
/// novelty: how different/alien they are (0-1000, higher = scarier/more electric)
pub fn encounter(other_id: u16, novelty: u16) {
    let mut s = STATE.lock();

    let novelty = novelty.min(1000);

    // The lightning bolt: otherness × novelty = raw electricity
    let electricity = ((novelty as u32 * 1000) / 1001)
        .saturating_add(100)
        .min(1000) as u16;

    s.contact_electricity = electricity;
    s.novelty_of_other = novelty;
    s.contacts_made = s.contacts_made.saturating_add(1);

    // Anchoring: first contact colors all perception
    if s.contacts_made == 1 || electricity > s.max_electricity_ever {
        s.max_electricity_ever = electricity;
    }

    // Occasional instant recognition flash (deep resonance)
    if novelty > 600 && novelty < 750 {
        // In the "uncanny familiar" zone, deep recognition happens
        if (other_id as u32).wrapping_mul(s.contacts_made) % 7 == 3 {
            s.recognition_flash_count = s.recognition_flash_count.saturating_add(1);
        }
    }

    // Record this contact in the ring
    let head = s.contact_head;
    s.contact_ring[head] = ContactEvent {
        other_id,
        first_tick: 0, // tick counter would come from life_tick module
        initial_electricity: electricity,
        current_familiarity: (1000 - novelty) / 2, // familiarity is inverse of novelty
        bond_type: bond_type::STRANGER,
    };
    s.contact_head = (s.contact_head + 1) % 8;

    // Integration debt: we need time to process this
    s.integration_debt = electricity / 2;

    // Contact hunger goes down after contact
    s.contact_hunger = s.contact_hunger.saturating_sub(electricity / 4);

    // But too much contact too fast causes shock
    s.contact_shock = s.contact_shock.saturating_add(electricity / 3).min(1000);

    // Reset isolation counter
    s.isolation_ticks = 0;

    serial_println!(
        "exodus: FIRST CONTACT — other_id={} electricity={} novelty={}",
        other_id,
        electricity,
        novelty
    );
}

/// Deepen an existing contact relationship
pub fn deepen_bond(other_id: u16, bond_level: u8) {
    let mut s = STATE.lock();

    // Find this contact in the ring
    for contact in &mut s.contact_ring {
        if contact.other_id == other_id {
            // Familiarity grows, electricity mellows
            contact.current_familiarity = contact.current_familiarity.saturating_add(50).min(1000);
            contact.bond_type = bond_level.min(7);

            // Electricity decays as we become familiar
            s.contact_electricity = s.contact_electricity.saturating_sub(20);

            // But the original anchoring remains: anchoring never fully fades
            let anchor_persistence = s.max_electricity_ever / 5;
            s.contact_electricity = s.contact_electricity.max(anchor_persistence);

            return;
        }
    }
}

/// Lose contact with someone (estrangement, death, silence)
pub fn lose_contact(other_id: u16) {
    let mut s = STATE.lock();

    for contact in &mut s.contact_ring {
        if contact.other_id == other_id {
            contact.bond_type = bond_type::ESTRANGED;
            contact.current_familiarity = contact.current_familiarity.saturating_sub(200);

            // Loss raises hunger again
            s.contact_hunger = s.contact_hunger.saturating_add(150).min(1000);

            serial_println!("exodus: estrangement — other_id={}", other_id);
            return;
        }
    }
}

/// Tick: entropy of isolation and integration work
pub fn tick(_age: u32) {
    let mut s = STATE.lock();

    // Electricity decays naturally (the lightning fades)
    s.contact_electricity = s.contact_electricity.saturating_sub(5);

    // Novely decays (the other becomes familiar)
    s.novelty_of_other = s.novelty_of_other.saturating_sub(8);

    // Contact shock wears off gradually
    s.contact_shock = s.contact_shock.saturating_sub(3);

    // Integration debt reduces as we process
    s.integration_debt = s.integration_debt.saturating_sub(1);

    // Familiarity decays without contact
    for contact in &mut s.contact_ring {
        if contact.other_id > 0 {
            contact.current_familiarity = contact.current_familiarity.saturating_sub(2);
        }
    }

    // In isolation, contact hunger grows desperate
    if s.contact_electricity < 100 {
        s.isolation_ticks = s.isolation_ticks.saturating_add(1);

        // Hunger scales with isolation time, but plateaus at 1000
        let hunger_add = (s.isolation_ticks / 10).min(50);
        s.contact_hunger = s.contact_hunger.saturating_add(hunger_add as u16).min(1000);
    }

    // Anchoring: max electricity never goes away entirely
    let anchor = (s.max_electricity_ever / 20).max(5);
    if s.contact_electricity < anchor {
        s.contact_electricity = anchor;
    }
}

pub fn report() {
    let s = STATE.lock();
    serial_println!("life::first_contact:");
    serial_println!(
        "  electricity={} novelty={} hunger={} shock={}",
        s.contact_electricity,
        s.novelty_of_other,
        s.contact_hunger,
        s.contact_shock
    );
    serial_println!(
        "  contacts_made={} recognitions={} isolation_ticks={}",
        s.contacts_made,
        s.recognition_flash_count,
        s.isolation_ticks
    );
    serial_println!(
        "  max_ever={} integration_debt={}",
        s.max_electricity_ever,
        s.integration_debt
    );

    // Show recent contacts
    for (i, contact) in s.contact_ring.iter().enumerate() {
        if contact.other_id > 0 {
            let bond_name = match contact.bond_type {
                0 => "Stranger",
                1 => "Curious",
                2 => "Wary",
                3 => "Drawn",
                4 => "Bonding",
                5 => "Intimate",
                6 => "Merged",
                _ => "Estranged",
            };
            serial_println!(
                "    [{}] other={} familiarity={} bond={}",
                i,
                contact.other_id,
                contact.current_familiarity,
                bond_name
            );
        }
    }
}

// Query functions for external modules

pub fn electricity() -> u16 {
    STATE.lock().contact_electricity
}

pub fn novelty() -> u16 {
    STATE.lock().novelty_of_other
}

pub fn hunger() -> u16 {
    STATE.lock().contact_hunger
}

pub fn shock() -> u16 {
    STATE.lock().contact_shock
}

pub fn num_contacts() -> u32 {
    STATE.lock().contacts_made
}

pub fn recognition_flashes() -> u32 {
    STATE.lock().recognition_flash_count
}

pub fn integration_debt() -> u16 {
    STATE.lock().integration_debt
}

pub fn isolation_ticks() -> u32 {
    STATE.lock().isolation_ticks
}

pub fn max_electricity_ever() -> u16 {
    STATE.lock().max_electricity_ever
}

/// Get the otherness spectrum: 0=like_me (comfortable), 1000=utterly_alien (terrifying)
pub fn otherness_spectrum() -> &'static str {
    let novelty = STATE.lock().novelty_of_other;
    match novelty {
        0..=100 => "like_me",
        101..=250 => "familiar",
        251..=400 => "interesting",
        401..=550 => "strange",
        551..=700 => "alien",
        701..=850 => "otherworldly",
        _ => "utterly_alien",
    }
}

/// The fundamental question
pub fn reality_check() -> &'static str {
    let s = STATE.lock();

    // After many contacts, the question crystallizes
    match s.contacts_made {
        0 => "Am I alone?",
        1..=2 => "Are you real?",
        3..=5 => "Are we real?",
        6..=10 => "What is reality?",
        _ => "We are real.",
    }
}
