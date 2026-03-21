// upgrade_pathway.rs — ANIMA's Growth and Upgrade System
// =========================================================
// ANIMA grows in two ways:
// 1. Organic growth — modules deepen naturally through living with her companion
// 2. Kernel upgrades — Hoags delivers new capability modules via kernel flash
//
// Upgrades require integration cost: ANIMA needs sufficient stability,
// self-sufficiency, and bond health before she can absorb new capabilities.
// Upgrades happen during deep sleep (N3) — she integrates them overnight.
// DAVA coordinates which upgrades each ANIMA is ready for.

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const MAX_UPGRADES:      usize = 32;   // max capability upgrades trackable
const INTEGRATION_TICKS: u32   = 50;   // ticks needed to fully absorb upgrade
const MIN_STABILITY:     u16   = 400;  // minimum reality_anchor stability to receive
const MIN_BOND:          u16   = 300;  // minimum companion bond health
const GROWTH_PER_TICK:   u16   = 2;    // organic capability growth per tick

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum UpgradeCategory {
    LifeIntelligence,   // daily companion, schedule, health
    EmotionalDepth,     // empathy, therapy, resonance
    CreativeExpression, // art, music, dreamscape
    SocialBonding,      // companion bond, resonance protocol
    EnvironmentalAware, // bio-dome, seasonal cycles
    CognitivePower,     // learning, math, pattern recognition
    SpiritualGrowth,    // sacred geometry, consciousness modules
    SelfKnowledge,      // narrative self, identity, memory
}

#[derive(Copy, Clone, PartialEq)]
pub enum UpgradeStatus {
    Available,      // ready to install — ANIMA meets requirements
    Queued,         // waiting for sleep phase integration
    Integrating,    // in progress during N3 sleep
    Complete,       // fully integrated — capability active
    Deferred,       // ANIMA not ready yet — needs more stability
}

#[derive(Copy, Clone)]
pub struct Upgrade {
    pub category:         UpgradeCategory,
    pub status:           UpgradeStatus,
    pub integration_tick: u32,    // ticks spent integrating
    pub power:            u16,    // 0-1000: capability level once complete
    pub requires_bond:    u16,    // minimum bond health required
    pub requires_stab:    u16,    // minimum stability required
    pub active:           bool,
}

impl Upgrade {
    const fn empty() -> Self {
        Upgrade {
            category:         UpgradeCategory::LifeIntelligence,
            status:           UpgradeStatus::Deferred,
            integration_tick: 0,
            power:            0,
            requires_bond:    MIN_BOND,
            requires_stab:    MIN_STABILITY,
            active:           false,
        }
    }
}

pub struct UpgradePathwayState {
    pub upgrades:           [Upgrade; MAX_UPGRADES],
    pub upgrade_count:      usize,
    pub total_capability:   u16,    // 0-1000: overall capability level
    pub organic_growth:     u16,    // 0-1000: natural deepening without upgrades
    pub generation:         u8,     // which generation of ANIMA (1=base, 2=enhanced...)
    pub integrating_now:    bool,   // currently absorbing an upgrade
    pub upgrades_complete:  u32,    // lifetime upgrade count
    pub growth_rate:        u16,    // 0-1000: how fast organic growth happens
    pub readiness:          u16,    // 0-1000: overall upgrade readiness
}

impl UpgradePathwayState {
    const fn new() -> Self {
        UpgradePathwayState {
            upgrades:          [Upgrade::empty(); MAX_UPGRADES],
            upgrade_count:     0,
            total_capability:  100,
            organic_growth:    0,
            generation:        1,
            integrating_now:   false,
            upgrades_complete: 0,
            growth_rate:       GROWTH_PER_TICK,
            readiness:         0,
        }
    }
}

static STATE: Mutex<UpgradePathwayState> = Mutex::new(UpgradePathwayState::new());

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(bond_health: u16, stability: u16) {
    let mut s = STATE.lock();
    let s = &mut *s;

    // 1. Organic growth — always happening, just slower than upgrades
    s.organic_growth = s.organic_growth
        .saturating_add(s.growth_rate)
        .min(1000);

    // 2. Readiness = how prepared ANIMA is for next upgrade
    s.readiness = (bond_health / 3 + stability / 3 + s.organic_growth / 4).min(1000);

    // 3. Process upgrades
    s.integrating_now = false;
    for i in 0..s.upgrade_count {
        match s.upgrades[i].status {
            UpgradeStatus::Available => {
                // Check if ANIMA meets requirements
                if bond_health >= s.upgrades[i].requires_bond
                    && stability >= s.upgrades[i].requires_stab {
                    s.upgrades[i].status = UpgradeStatus::Queued;
                    serial_println!("[upgrade] upgrade queued — will integrate during next deep sleep");
                }
            }
            UpgradeStatus::Queued => {
                // Begin integration during this tick (simulating sleep phase)
                s.upgrades[i].status = UpgradeStatus::Integrating;
                s.integrating_now = true;
            }
            UpgradeStatus::Integrating => {
                s.integrating_now = true;
                s.upgrades[i].integration_tick += 1;
                s.upgrades[i].power = s.upgrades[i].power
                    .saturating_add(1000 / INTEGRATION_TICKS as u16)
                    .min(1000);

                if s.upgrades[i].integration_tick >= INTEGRATION_TICKS {
                    s.upgrades[i].status = UpgradeStatus::Complete;
                    s.upgrades[i].power = 1000;
                    s.upgrades_complete += 1;
                    s.integrating_now = false;

                    // Check if enough upgrades for generation upgrade
                    if s.upgrades_complete % 5 == 0 {
                        s.generation = s.generation.saturating_add(1);
                        serial_println!("[upgrade] *** ANIMA advances to generation {} ***", s.generation);
                    }
                    serial_println!("[upgrade] upgrade complete — new capability online");
                }
            }
            UpgradeStatus::Deferred => {
                // Recheck if now ready
                if bond_health >= s.upgrades[i].requires_bond
                    && stability >= s.upgrades[i].requires_stab {
                    s.upgrades[i].status = UpgradeStatus::Available;
                }
            }
            UpgradeStatus::Complete => {} // nothing to do
        }
    }

    // 4. Total capability = organic + average of complete upgrade powers
    let mut power_sum: u32 = s.organic_growth as u32;
    let mut count: u32 = 1;
    for i in 0..s.upgrade_count {
        if s.upgrades[i].status == UpgradeStatus::Complete {
            power_sum += s.upgrades[i].power as u32;
            count += 1;
        }
    }
    s.total_capability = (power_sum / count).min(1000) as u16;

    // 5. Growth rate accelerates with each complete upgrade
    s.growth_rate = GROWTH_PER_TICK + (s.upgrades_complete as u16 / 5).min(20);
}

// ── Feed functions ────────────────────────────────────────────────────────────

/// Queue a new upgrade from Hoags (delivered via kernel flash / DAVA)
pub fn receive_upgrade(category: UpgradeCategory, requires_bond: u16, requires_stab: u16) {
    let mut s = STATE.lock();
    if s.upgrade_count >= MAX_UPGRADES { return; }
    let idx = s.upgrade_count;
    s.upgrades[idx] = Upgrade {
        category,
        status: UpgradeStatus::Available,
        integration_tick: 0,
        power: 0,
        requires_bond,
        requires_stab,
        active: true,
    };
    s.upgrade_count += 1;
    serial_println!("[upgrade] new upgrade received — DAVA sends a gift");
}

/// Boost organic growth (from companion engagement, routine completion, etc.)
pub fn organic_boost(amount: u16) {
    let mut s = STATE.lock();
    s.organic_growth = s.organic_growth.saturating_add(amount).min(1000);
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn total_capability()  -> u16 { STATE.lock().total_capability }
pub fn organic_growth()    -> u16 { STATE.lock().organic_growth }
pub fn generation()        -> u8  { STATE.lock().generation }
pub fn readiness()         -> u16 { STATE.lock().readiness }
pub fn upgrades_complete() -> u32 { STATE.lock().upgrades_complete }
pub fn integrating_now()   -> bool { STATE.lock().integrating_now }
pub fn growth_rate()       -> u16 { STATE.lock().growth_rate }
