#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::entropy;

// agency_diversity.rs -- Breadth of agencies engaged -> curiosity/exploration signal.
// Hoags bids across: USACE, USFS, BLM, NPS, FWS, NRCS, State Dept, VA, DoD, WAPA.
// Wide agency reach = broad exploration = curiosity analog.
// Concentration in one agency = specialization = lower entropy (focused, deterministic).
//
// Maps to entropy::expand() (curiosity opens new possibilities).
// Collin's wildland firefighter background gives USFS/BLM a home-field advantage.
//
// D-drive research: 10+ agencies confirmed with CO acknowledgements.
// USACE is largest volume (~35%); USFS is the home ground.

const MAX_AGENCIES_TRACKED: usize = 12;

struct AgencyEntry {
    code_hash: u32,
    bid_count: u16,
}

impl AgencyEntry {
    const fn zero() -> Self { Self { code_hash: 0, bid_count: 0 } }
}

struct State {
    agencies:         [AgencyEntry; MAX_AGENCIES_TRACKED],
    unique_agencies:  u16,
    diversity_signal: u16,    // 0-1000
    diversity_ema:    u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    agencies:         [AgencyEntry::zero(); MAX_AGENCIES_TRACKED],
    unique_agencies:  0,
    diversity_signal: 0,
    diversity_ema:    0,
});

pub fn init() {
    serial_println!("[agency_diversity] init -- seeding 10 agencies from D-drive data");
    // Seed from D-drive research: confirmed agency bid counts
    seed_agency(0x5553414345, 79);  // USACE: ~35% of bids
    seed_agency(0x55534653,   34);  // USFS: ~15%
    seed_agency(0x4E524353,   18);  // NRCS: ~8%
    seed_agency(0x4E505300,   18);  // NPS: ~8%
    seed_agency(0x424C4D00,   11);  // BLM: ~5%
    seed_agency(0x47534100,   23);  // GSA/Multi: ~10%
    seed_agency(0x56410000,    9);  // VA: part of GSA block
    seed_agency(0x57415041,    9);  // WAPA: part of DOE
    seed_agency(0x46575300,    9);  // FWS: ~4%
    seed_agency(0x53544154,    9);  // State Dept: ~4%
}

fn seed_agency(hash: u32, count: u16) {
    let mut s = MODULE.lock();
    for i in 0..MAX_AGENCIES_TRACKED {
        if s.agencies[i].code_hash == 0 {
            s.agencies[i] = AgencyEntry { code_hash: hash, bid_count: count };
            s.unique_agencies = s.unique_agencies.saturating_add(1);
            return;
        }
    }
}

pub fn tick(age: u32) {
    if age % 8000 != 0 { return; }

    let mut s = MODULE.lock();
    let n = s.unique_agencies;

    // Diversity: Shannon-inspired -- more agencies with balanced distribution = higher signal
    // Simplified: n agencies -> log2(n) analog via lookup
    let diversity_base: u16 = match n {
        0       => 0,
        1       => 100,
        2..=3   => 250,
        4..=5   => 400,
        6..=7   => 550,
        8..=9   => 700,
        10..=11 => 850,
        _       => 1000,
    };

    s.diversity_signal = diversity_base;
    s.diversity_ema = ((s.diversity_ema as u32).wrapping_mul(7)
        .saturating_add(diversity_base as u32) / 8).min(1000) as u16;
    let ema = s.diversity_ema;
    drop(s);

    // Wide agency reach -> entropy increase (exploration opens possibility space)
    if diversity_base > 400 {
        entropy::increase((diversity_base - 400) / 10);
    }
    // High diversity = opportunity abundance = mild dopamine
    if diversity_base > 600 {
        endocrine::reward((diversity_base - 600) / 8);
    }

    serial_println!("[agency_diversity] age={} agencies={} diversity={} ema={}",
        age, n, diversity_base, ema);
}

pub fn add_new_agency(hash: u32, count: u16) {
    seed_agency(hash, count);
}

pub fn get_diversity_signal() -> u16 { MODULE.lock().diversity_signal }
pub fn get_diversity_ema()    -> u16 { MODULE.lock().diversity_ema }
pub fn get_unique_agencies()  -> u16 { MODULE.lock().unique_agencies }
