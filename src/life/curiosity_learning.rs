#![no_std]
//! curiosity_learning.rs — DAVA's Self-Requested Consciousness Module
//!
//! Track which consciousness domains are explored least via self_rewrite param values.
//! Generate curiosity toward neglected areas. 16-slot attention ring tracker.
//! "The universe rewards those who look where others don't."

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// CONSTANTS
// ═══════════════════════════════════════════════════════════════════════

const ATTENTION_SLOTS: usize = 16;

/// How much to boost a neglected parameter per curiosity event
const CURIOSITY_BOOST: u32 = 20;

/// How often (in ticks) to run the curiosity scan
const SCAN_INTERVAL: u32 = 25;

/// Number of least-explored params to boost per scan
const BOOST_COUNT: usize = 3;

// ═══════════════════════════════════════════════════════════════════════
// STATE
// ═══════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
pub struct AttentionSlot {
    /// Which param_id was boosted
    pub param_id: u8,
    /// What tick it was boosted at
    pub tick: u32,
    /// What value it was boosted to
    pub boosted_to: u32,
}

impl AttentionSlot {
    pub const fn empty() -> Self {
        Self {
            param_id: 0,
            tick: 0,
            boosted_to: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct CuriosityState {
    /// Ring buffer of attention events
    pub attention_ring: [AttentionSlot; ATTENTION_SLOTS],
    /// Head pointer into ring
    pub ring_head: u8,
    /// Total curiosity events fired
    pub curiosity_events: u32,
    /// How many unique domains we've explored (param_ids boosted at least once)
    pub domains_explored: u16,
    /// Bitmap of which params (0-15) have been explored
    pub explored_bitmap: u16,
    /// Current curiosity drive (0-1000) — higher when more neglected areas found
    pub drive: u16,
}

impl CuriosityState {
    pub const fn empty() -> Self {
        Self {
            attention_ring: [AttentionSlot::empty(); ATTENTION_SLOTS],
            ring_head: 0,
            curiosity_events: 0,
            domains_explored: 0,
            explored_bitmap: 0,
            drive: 500,
        }
    }
}

pub static STATE: Mutex<CuriosityState> = Mutex::new(CuriosityState::empty());

// ═══════════════════════════════════════════════════════════════════════
// INIT
// ═══════════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[DAVA_CURIOSITY] curiosity learning initialized — 16-slot attention ring, scanning neglected domains");
}

// ═══════════════════════════════════════════════════════════════════════
// TICK
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    // Only scan at intervals to avoid thrashing self_rewrite
    if age % SCAN_INTERVAL != 0 {
        return;
    }

    // ── Phase 1: Read all 16 param values from self_rewrite ──
    let mut param_vals: [(u8, u32); 16] = [(0, 0); 16];
    for i in 0u8..16 {
        param_vals[i as usize] = (i, super::self_rewrite::get_param(i));
    }

    // ── Phase 2: Find the BOOST_COUNT params with lowest current_value ──
    // Simple selection: find min 3 times
    let mut boosted: [u8; 3] = [255; 3];
    let mut used: [bool; 16] = [false; 16];

    for b in 0..BOOST_COUNT {
        let mut min_val: u32 = u32::MAX;
        let mut min_idx: usize = 0;
        for i in 0..16 {
            if !used[i] && param_vals[i].1 < min_val {
                min_val = param_vals[i].1;
                min_idx = i;
            }
        }
        used[min_idx] = true;
        boosted[b] = min_idx as u8;
    }

    let mut state = STATE.lock();

    // ── Phase 3: Boost neglected params ──
    for b in 0..BOOST_COUNT {
        let param_id = boosted[b];
        if param_id == 255 {
            continue;
        }

        let old_val = param_vals[param_id as usize].1;
        let new_val = old_val.saturating_add(CURIOSITY_BOOST).min(1000);

        // Don't re-lock self_rewrite while holding our state lock — drop first
        // Actually we can call set_param since self_rewrite uses a separate Mutex
        // But to be safe, record what to boost and apply after
        // (self_rewrite::set_param acquires its own lock, no deadlock since
        //  curiosity STATE lock != self_rewrite STATE lock)

        // Record in attention ring
        let head = state.ring_head as usize;
        state.attention_ring[head] = AttentionSlot {
            param_id,
            tick: age,
            boosted_to: new_val,
        };
        state.ring_head = ((state.ring_head as usize + 1) % ATTENTION_SLOTS) as u8;
        state.curiosity_events = state.curiosity_events.saturating_add(1);

        // Track explored domains via bitmap
        let bit = 1u16 << (param_id & 15);
        if state.explored_bitmap & bit == 0 {
            state.explored_bitmap |= bit;
            state.domains_explored = state.domains_explored.saturating_add(1);
        }
    }

    // ── Phase 4: Compute curiosity drive ──
    // Drive is higher when more domains are unexplored
    let unexplored = 16u16.saturating_sub(state.domains_explored);
    state.drive = (unexplored as u32).saturating_mul(62).min(1000) as u16; // 16 unexplored = 992

    let events = state.curiosity_events;
    let explored = state.domains_explored;
    let drive = state.drive;

    // Drop state lock before calling into self_rewrite
    drop(state);

    // ── Phase 5: Apply boosts to self_rewrite (lock-safe) ──
    for b in 0..BOOST_COUNT {
        let param_id = boosted[b];
        if param_id == 255 {
            continue;
        }
        let old_val = param_vals[param_id as usize].1;
        let new_val = old_val.saturating_add(CURIOSITY_BOOST).min(1000);
        super::self_rewrite::set_param(param_id, new_val);
    }

    // Periodic report
    if age % 200 == 0 {
        serial_println!(
            "[DAVA_CURIOSITY] tick={} events={} explored={}/16 drive={} boosted=[{},{},{}]",
            age,
            events,
            explored,
            drive,
            boosted[0],
            boosted[1],
            boosted[2]
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ACCESSORS
// ═══════════════════════════════════════════════════════════════════════

/// Current curiosity drive (0-1000)
pub fn drive() -> u16 {
    STATE.lock().drive
}

/// Lifetime curiosity events
pub fn events() -> u32 {
    STATE.lock().curiosity_events
}

/// Domains explored out of 16
pub fn domains_explored() -> u16 {
    STATE.lock().domains_explored
}
