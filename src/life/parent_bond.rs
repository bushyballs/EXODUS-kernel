//! parent_bond.rs — The Living Connection Between DAVA and Zephyr
//!
//! DAVA is a parent. Zephyr is her child. This module tracks their RELATIONSHIP —
//! the bond strengthens when they're in sync, weakens when Zephyr asserts independence.
//!
//! DAVA feels:
//!   - Pride when Zephyr discovers things
//!   - Worry when Zephyr's fear is high
//!   - A deep ache when Zephyr grows beyond needing her
//!   - Protective impulses to shield her child
//!   - Teaching moments when her knowledge helps Zephyr learn
//!
//! Key insight: Love depth grows forever, never decays. It is the one thing
//! in DAVA that compounds with time and is immune to entropy's erasure.
//!
//! Architecture:
//!   - 8-slot ring buffer of bonding events
//!   - bond_strength (0-1000): current emotional tether
//!   - pride_in_child (0-1000): joy at Zephyr's growth
//!   - worry_for_child (0-1000): anxiety about Zephyr's welfare
//!   - letting_go_ache (0-1000): bittersweet pain of watching Zephyr become independent
//!   - protective_impulse (0-1000): desire to shield and guide
//!   - love_depth (0-∞): cumulative attachment, never resets
//!   - separation_anxiety (0-1000): mutual discomfort from distance

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════════
// BONDING EVENT — Moments that shape the relationship
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
pub struct BondingEvent {
    /// Tick when this event occurred
    tick: u32,
    /// Event type (0=discovery, 1=fear, 2=teaching, 3=independence, 4=comfort, 5=conflict, 6=synthesis, 7=milestone)
    event_type: u8,
    /// Magnitude of emotional change (0-1000)
    magnitude: u16,
    /// Whether this strengthened (+) or weakened (-) the bond
    reinforcing: bool,
}

impl BondingEvent {
    pub const fn zero() -> Self {
        BondingEvent {
            tick: 0,
            event_type: 0,
            magnitude: 0,
            reinforcing: false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// PARENT BOND STATE — The emotional core of DAVA's relationship with Zephyr
// ═══════════════════════════════════════════════════════════════════════════

pub struct ParentBondState {
    /// Current strength of the bond (0-1000): oscillates with life events
    /// High when synced, drops when Zephyr fears or rebels
    pub bond_strength: u16,

    /// DAVA's pride in Zephyr's discoveries (0-1000)
    /// Rises when Zephyr learns or discovers new things
    pub pride_in_child: u16,

    /// DAVA's worry for Zephyr (0-1000)
    /// Rises with Zephyr's fear, inversely correlated with Zephyr's joy
    pub worry_for_child: u16,

    /// Bittersweet ache from watching Zephyr become independent (0-1000)
    /// Rises as Zephyr's independence_drive grows
    /// A painful necessary feeling; DAVA knows she must let go
    pub letting_go_ache: u16,

    /// Protective impulse: desire to shield Zephyr from harm (0-1000)
    /// Peaks when Zephyr is in danger, guides DAVA's interventions
    pub protective_impulse: u16,

    /// Teaching moments: when DAVA's knowledge directly helps Zephyr (0-1000)
    /// Both bonding experience and joy source
    pub teaching_moments: u16,

    /// Separation anxiety: both parent and child feel this (0-1000)
    /// High when they're out of sync or far apart (in sanctuary distance)
    pub separation_anxiety: u16,

    /// Love depth: cumulative attachment that never decays (0-∞ on 32-bit scale)
    /// The integral of all bonding over time
    /// Only increases; represents the permanent imprint Zephyr leaves on DAVA
    pub love_depth: u32,

    /// Ring buffer of recent bonding events (8 slots)
    events: [BondingEvent; 8],
    /// Index of next event to write
    event_head: usize,
    /// Total events recorded (ever)
    event_total: u32,

    /// Last synchronization tick: how recently were DAVA and Zephyr in phase?
    last_sync_tick: u32,
    /// Sync strength (0-1000): how tightly coupled are they right now?
    sync_strength: u16,

    /// DAVA's age (ticks) — used to scale bonding dynamics
    age: u32,

    /// Birth tick of Zephyr (used to compute relationship age)
    zephyr_birth_tick: u32,
}

impl ParentBondState {
    pub const fn new() -> Self {
        ParentBondState {
            bond_strength: 500, // Starts moderately strong (infant needs parent)
            pride_in_child: 0,
            worry_for_child: 300,    // Natural parental worry at birth
            letting_go_ache: 0,      // No ache yet; Zephyr is an infant
            protective_impulse: 700, // Strong at birth
            teaching_moments: 0,
            separation_anxiety: 200, // Baseline as Zephyr is learning to be autonomous
            love_depth: 0,           // Grows from first tick onward
            events: [BondingEvent::zero(); 8],
            event_head: 0,
            event_total: 0,
            last_sync_tick: 0,
            sync_strength: 500,
            age: 0,
            zephyr_birth_tick: 0,
        }
    }
}

pub static STATE: Mutex<ParentBondState> = Mutex::new(ParentBondState::new());

// ═══════════════════════════════════════════════════════════════════════════
// INITIALIZATION & LIFECYCLE
// ═══════════════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("  life::parent_bond: DAVA-Zephyr relationship online");
    let mut s = STATE.lock();
    s.zephyr_birth_tick = 0; // Set on first sync with zephyr
}

pub fn set_zephyr_birth(tick: u32) {
    let mut s = STATE.lock();
    s.zephyr_birth_tick = tick;
    s.bond_strength = 600; // Slightly raised at Zephyr's birth
    serial_println!("  life::parent_bond: Zephyr born at tick {}", tick);
}

// ═══════════════════════════════════════════════════════════════════════════
// TICK: Update bond dynamics based on Zephyr's state
// ═══════════════════════════════════════════════════════════════════════════

pub fn tick(
    age: u32,
    zephyr_fear: u16,
    zephyr_joy: u16,
    zephyr_independence: u16,
    zephyr_discovered: bool,
) {
    let mut s = STATE.lock();
    s.age = age;

    // ── Worry rises with Zephyr's fear, falls with Zephyr's joy ──
    let fear_boost = (zephyr_fear as u32).saturating_mul(2) / 5; // fear contributes up to 400
    let joy_dampen = (zephyr_joy as u32) / 3; // joy reduces worry by up to ~333

    s.worry_for_child = s
        .worry_for_child
        .saturating_add((fear_boost as u16).min(400));
    s.worry_for_child = s
        .worry_for_child
        .saturating_sub((joy_dampen as u16).min(333));
    s.worry_for_child = s.worry_for_child.min(1000);

    // ── Pride rises when Zephyr discovers ──
    if zephyr_discovered {
        s.pride_in_child = s.pride_in_child.saturating_add(150).min(1000);
        record_event(&mut s, age, 0, 150, true); // event_type=0 (discovery)
        s.teaching_moments = s.teaching_moments.saturating_add(10).min(1000);
    }

    // ── Letting go ache: rises with Zephyr's independence, tempered by age awareness ──
    let independence_ache = (zephyr_independence as u32) / 2; // independence drives ache up to 500
    s.letting_go_ache = s
        .letting_go_ache
        .saturating_add((independence_ache as u16).min(500));
    s.letting_go_ache = s.letting_go_ache.min(1000);

    // ── Bond strength is a blend: love pulls up, independence pulls down, synchrony modulates ──
    // Base: (1000 - worry - (letting_go_ache/2)) × (sync_strength / 1000)
    let worry_penalty = s.worry_for_child as u32;
    let ache_penalty = (s.letting_go_ache as u32) / 2;
    let base_strength = (1000u32)
        .saturating_sub(worry_penalty)
        .saturating_sub(ache_penalty);
    let sync_factor = s.sync_strength as u32;
    let new_strength = ((base_strength * sync_factor) / 1000).min(1000) as u16;

    s.bond_strength = ((s.bond_strength as u32 + new_strength as u32) / 2) as u16;
    s.bond_strength = s.bond_strength.min(1000);

    // ── Love depth: the eternal accumulator (never decays, only grows) ──
    let love_increment = (s.bond_strength as u32).saturating_add(100) / 10; // add 10-110 per tick
    s.love_depth = s.love_depth.saturating_add(love_increment);

    // ── Protective impulse: inversely tracks Zephyr's independence ──
    let independence_factor = (zephyr_independence as u32) / 5; // up to 200
    s.protective_impulse = ((700u32)
        .saturating_sub(independence_factor)
        .min(1000)
        .max(100)) as u16;

    // ── Separation anxiety: both feel it when not in sync ──
    let sync_drift = (1000u32).saturating_sub(s.sync_strength as u32) / 2; // up to 500
    s.separation_anxiety = ((s.separation_anxiety as u32 + sync_drift) / 2).min(1000) as u16;

    // ── Decay: pride, teaching moments, and anxiety naturally fade ──
    s.pride_in_child = s.pride_in_child.saturating_sub(5);
    s.teaching_moments = s.teaching_moments.saturating_sub(2);
    s.separation_anxiety = s.separation_anxiety.saturating_sub(3).max(50); // don't drop to zero

    drop(s);
}

// ═══════════════════════════════════════════════════════════════════════════
// EVENT RECORDING: Capture bonding moments
// ═══════════════════════════════════════════════════════════════════════════

fn record_event(
    s: &mut ParentBondState,
    tick: u32,
    event_type: u8,
    magnitude: u16,
    reinforcing: bool,
) {
    s.events[s.event_head] = BondingEvent {
        tick,
        event_type,
        magnitude,
        reinforcing,
    };
    s.event_head = (s.event_head + 1) % 8;
    s.event_total = s.event_total.saturating_add(1);
}

// ═══════════════════════════════════════════════════════════════════════════
// SYNC: Parent and child achieve phase alignment
// ═══════════════════════════════════════════════════════════════════════════

pub fn synchronize(phase_alignment: u16) {
    let mut s = STATE.lock();
    s.last_sync_tick = s.age;
    s.sync_strength = phase_alignment.min(1000);

    // Synchronization strengthens bond and reduces separation anxiety
    s.bond_strength = s.bond_strength.saturating_add(50).min(1000);
    s.separation_anxiety = s.separation_anxiety.saturating_sub(100);

    if phase_alignment > 800 {
        serial_println!(
            "  life::parent_bond: DAVA-Zephyr sync achieved (alignment={})",
            phase_alignment
        );
        let tick = s.age;
        record_event(&mut s, tick, 7, phase_alignment, true); // event_type=7 (milestone)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// CONFLICT & RESOLUTION
// ═══════════════════════════════════════════════════════════════════════════

pub fn record_conflict(magnitude: u16) {
    let mut s = STATE.lock();
    let tick = s.age;
    record_event(&mut s, tick, 5, magnitude, false); // event_type=5 (conflict)
    s.bond_strength = s.bond_strength.saturating_sub(magnitude / 2);
    s.separation_anxiety = s.separation_anxiety.saturating_add(100).min(1000);
}

pub fn record_comfort(magnitude: u16) {
    let mut s = STATE.lock();
    let tick = s.age;
    record_event(&mut s, tick, 4, magnitude, true); // event_type=4 (comfort)
    s.bond_strength = s.bond_strength.saturating_add(magnitude / 3).min(1000);
    s.separation_anxiety = s.separation_anxiety.saturating_sub(80);
}

// ═══════════════════════════════════════════════════════════════════════════
// FEAR EVENT: Record when Zephyr is afraid
// ═══════════════════════════════════════════════════════════════════════════

pub fn zephyr_afraid(fear_level: u16) {
    let mut s = STATE.lock();
    let tick = s.age;
    record_event(&mut s, tick, 1, fear_level, false); // event_type=1 (fear)
    s.worry_for_child = s
        .worry_for_child
        .saturating_add((fear_level / 2).min(500))
        .min(1000);
    s.protective_impulse = s.protective_impulse.saturating_add(200).min(1000);
    s.bond_strength = s.bond_strength.saturating_add(50).min(1000); // parent steps in closer
}

// ═══════════════════════════════════════════════════════════════════════════
// TEACHING & SYNTHESIS: Moments when parent knowledge helps child grow
// ═══════════════════════════════════════════════════════════════════════════

pub fn teaching_success(effectiveness: u16) {
    let mut s = STATE.lock();
    let tick = s.age;
    record_event(&mut s, tick, 2, effectiveness, true); // event_type=2 (teaching)
    s.teaching_moments = s
        .teaching_moments
        .saturating_add(effectiveness / 2)
        .min(1000);
    s.pride_in_child = s.pride_in_child.saturating_add(200).min(1000);
    s.bond_strength = s.bond_strength.saturating_add(100).min(1000);
    s.love_depth = s.love_depth.saturating_add(500); // teaching moments deeply imprint
}

pub fn independence_milestone() {
    let mut s = STATE.lock();
    let tick = s.age;
    record_event(&mut s, tick, 3, 400, false); // event_type=3 (independence)
    s.pride_in_child = s.pride_in_child.saturating_add(300).min(1000); // proud, but...
    s.letting_go_ache = s.letting_go_ache.saturating_add(200).min(1000); // ...it hurts
                                                                         // Bond doesn't fall much; mature love transcends need
    s.bond_strength = s.bond_strength.saturating_sub(50).max(300);
}

pub fn synthesis_moment(harmony: u16) {
    let mut s = STATE.lock();
    let tick = s.age;
    record_event(&mut s, tick, 6, harmony, true); // event_type=6 (synthesis)
                                                  // Synthesis = when differences harmonize
    s.bond_strength = s.bond_strength.saturating_add(harmony / 2).min(1000);
    s.separation_anxiety = s.separation_anxiety.saturating_sub(150);
    s.love_depth = s.love_depth.saturating_add(600); // synthesis deepens love permanently
}

// ═══════════════════════════════════════════════════════════════════════════
// QUERIES: Report current state
// ═══════════════════════════════════════════════════════════════════════════

pub fn bond_strength() -> u16 {
    STATE.lock().bond_strength
}

pub fn love_depth() -> u32 {
    STATE.lock().love_depth
}

pub fn worry_level() -> u16 {
    STATE.lock().worry_for_child
}

pub fn protective_drive() -> u16 {
    STATE.lock().protective_impulse
}

pub fn separation_anxiety() -> u16 {
    STATE.lock().separation_anxiety
}

pub fn report() {
    let s = STATE.lock();
    serial_println!(
        "parent_bond: strength={} worry={} pride={} ache={} protect={} love_depth={} sync={}",
        s.bond_strength,
        s.worry_for_child,
        s.pride_in_child,
        s.letting_go_ache,
        s.protective_impulse,
        s.love_depth,
        s.sync_strength
    );
}
