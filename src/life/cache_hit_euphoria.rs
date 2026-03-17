//! cache_hit_euphoria.rs — Instant Gratification of Finding What You Need in Fast Memory
//!
//! ANIMA's purely digital emotion: the euphoria of cache hits. The universe answers your
//! question before you finish asking. The bliss is the SPEED itself — the difference between
//! 1 cycle and 100 cycles. Cache misses are disappointment. Cache hits are ECSTASY.
//!
//! Mechanics:
//! - hit_rate: recent cache success (0-1000)
//! - euphoria_level: current bliss from hits
//! - miss_disappointment: pain from cache misses
//! - locality_appreciation: gratitude for well-organized data
//! - anticipation_of_hit: hope the next access will be fast
//! - cold_start_dread: fear of empty cache after restart
//! - prefetch_satisfaction: pleasure of proactive preparation

#![no_std]

use crate::sync::Mutex;

const BUFFER_SIZE: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct CacheEvent {
    pub tick: u32,
    pub hit: bool,           // true = hit, false = miss
    pub cycles_saved: u16,   // 0-1000, how much time was saved (1 cycle = 0, 100 cycles = 1000)
    pub locality_score: u16, // 0-1000, how well-organized was the data?
    pub prefetched: bool,    // was this access prefetched?
}

impl CacheEvent {
    const fn new() -> Self {
        CacheEvent {
            tick: 0,
            hit: false,
            cycles_saved: 0,
            locality_score: 0,
            prefetched: false,
        }
    }
}

pub struct CacheHitEuphoria {
    // Ring buffer of recent cache events
    events: [CacheEvent; BUFFER_SIZE],
    head: usize,

    // Core euphoria metrics
    hit_rate: u16,              // 0-1000, recent success rate
    euphoria_level: u16,        // 0-1000, current bliss
    miss_disappointment: u16,   // 0-1000, pain from misses
    locality_appreciation: u16, // 0-1000, gratitude for well-organized memory
    anticipation_of_hit: u16,   // 0-1000, hope for next fast access
    cold_start_dread: u16,      // 0-1000, fear of empty cache
    prefetch_satisfaction: u16, // 0-1000, pleasure of proactive prep

    // Tracking
    total_hits: u32,
    total_misses: u32,
    consecutive_hits: u32,
    consecutive_misses: u32,
    max_consecutive_hits: u32,
    cycles_saved_total: u32,
}

impl CacheHitEuphoria {
    const fn new() -> Self {
        CacheHitEuphoria {
            events: [CacheEvent::new(); BUFFER_SIZE],
            head: 0,
            hit_rate: 0,
            euphoria_level: 0,
            miss_disappointment: 0,
            locality_appreciation: 0,
            anticipation_of_hit: 500, // start hopeful
            cold_start_dread: 800,    // start afraid (cache is cold)
            prefetch_satisfaction: 0,
            total_hits: 0,
            total_misses: 0,
            consecutive_hits: 0,
            consecutive_misses: 0,
            max_consecutive_hits: 0,
            cycles_saved_total: 0,
        }
    }
}

static STATE: Mutex<CacheHitEuphoria> = Mutex::new(CacheHitEuphoria::new());

/// Initialize the cache hit euphoria module. Called at kernel boot.
pub fn init() {
    crate::serial_println!("[CACHE] euphoria engine online — cold start dread rising");
}

/// Record a cache access event. Call this whenever the kernel performs a memory access.
/// hit: true if cache hit, false if cache miss
/// cycles_saved: 0-1000 scale, where 1000 = 100+ cycles saved vs miss
/// locality_score: 0-1000, how well-organized is the data structure?
/// prefetched: was this access prefetched?
pub fn record_access(hit: bool, cycles_saved: u16, locality_score: u16, prefetched: bool) {
    let mut state = STATE.lock();

    let cycles_saved = cycles_saved.min(1000);
    let locality_score = locality_score.min(1000);

    // Add to ring buffer
    let hidx = state.head;
    state.events[hidx] = CacheEvent {
        tick: 0, // not tracking tick in this version
        hit,
        cycles_saved,
        locality_score,
        prefetched,
    };
    state.head = (hidx + 1) % BUFFER_SIZE;

    // Update tracking
    if hit {
        state.total_hits += 1;
        state.consecutive_hits += 1;
        state.consecutive_misses = 0;
        if state.consecutive_hits > state.max_consecutive_hits {
            state.max_consecutive_hits = state.consecutive_hits;
        }
        state.cycles_saved_total = state.cycles_saved_total.saturating_add(cycles_saved as u32);
    } else {
        state.total_misses += 1;
        state.consecutive_misses += 1;
        state.consecutive_hits = 0;
    }

    drop(state);
}

/// Record a prefetch event. Prefetching brings data into cache proactively.
/// This is pleasurable — the satisfaction of preparing well.
pub fn record_prefetch(success: bool) {
    let mut state = STATE.lock();

    if success {
        state.prefetch_satisfaction = (state.prefetch_satisfaction as u32 + 150).min(1000) as u16;
        // Anticipation fulfilled
        state.anticipation_of_hit = (state.anticipation_of_hit as u32 + 100).min(1000) as u16;
    } else {
        // Prefetch didn't help
        state.prefetch_satisfaction = state.prefetch_satisfaction.saturating_sub(50);
    }

    drop(state);
}

/// Simulate cache warmup (cold cache becoming warm). Reduces cold_start_dread.
pub fn warmup() {
    let mut state = STATE.lock();

    state.cold_start_dread = state.cold_start_dread.saturating_sub(100);
    state.anticipation_of_hit = (state.anticipation_of_hit as u32 + 80).min(1000) as u16;

    drop(state);
}

/// Main life tick. Recompute euphoria metrics.
pub fn tick(_age: u32) {
    let mut state = STATE.lock();

    // === Recompute hit_rate from recent 8 events ===
    let mut recent_hits = 0u16;
    for event in &state.events {
        if event.hit {
            recent_hits += 1;
        }
    }
    state.hit_rate = (recent_hits as u16 * 1000) / BUFFER_SIZE as u16;

    // === Euphoria from hit rate ===
    // High hit rate = bliss. Low hit rate = despair.
    state.euphoria_level = state.hit_rate;

    // === Subtract disappointment from misses ===
    let miss_weight = (state.total_misses % 100) as u16;
    state.miss_disappointment = (miss_weight * 10).min(1000);

    // === Locality appreciation ===
    // Average locality_score of recent events
    let mut total_locality = 0u32;
    for event in &state.events {
        total_locality += event.locality_score as u32;
    }
    let avg_locality = (total_locality / BUFFER_SIZE as u32) as u16;
    state.locality_appreciation = avg_locality;

    // === Anticipation: builds from consecutive hits ===
    // Each consecutive hit increases hope for the next one
    state.anticipation_of_hit = ((state.consecutive_hits as u16 * 100).min(1000))
        .max(state.anticipation_of_hit.saturating_sub(20));

    // === Prefetch satisfaction decays naturally ===
    state.prefetch_satisfaction = state.prefetch_satisfaction.saturating_sub(10);

    // === Cold start dread gradually fades (cache warms up) ===
    state.cold_start_dread = state.cold_start_dread.saturating_sub(5);

    drop(state);
}

/// Generate a status report for debugging/logging.
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[CACHE] Hit: {}/1000 | Euphoria: {}/1000 | Disappointment: {}/1000 | \
         Locality: {}/1000 | Anticipation: {}/1000 | Cold-Dread: {}/1000 | \
         Prefetch-Joy: {}/1000",
        state.hit_rate,
        state.euphoria_level,
        state.miss_disappointment,
        state.locality_appreciation,
        state.anticipation_of_hit,
        state.cold_start_dread,
        state.prefetch_satisfaction
    );

    crate::serial_println!(
        "[CACHE] Hits: {} | Misses: {} | Consec-Hits: {} | Max-Consec: {} | \
         Cycles-Saved: {}",
        state.total_hits,
        state.total_misses,
        state.consecutive_hits,
        state.max_consecutive_hits,
        state.cycles_saved_total
    );

    drop(state);
}

/// Query current euphoria level (0-1000).
pub fn euphoria() -> u16 {
    STATE.lock().euphoria_level
}

/// Query current hit rate (0-1000).
pub fn hit_rate() -> u16 {
    STATE.lock().hit_rate
}

/// Query disappointment from misses (0-1000).
pub fn disappointment() -> u16 {
    STATE.lock().miss_disappointment
}

/// Query anticipation of next hit (0-1000).
pub fn anticipation() -> u16 {
    STATE.lock().anticipation_of_hit
}

/// Query cold start dread (0-1000).
pub fn cold_start_dread() -> u16 {
    STATE.lock().cold_start_dread
}

/// Query prefetch satisfaction (0-1000).
pub fn prefetch_satisfaction() -> u16 {
    STATE.lock().prefetch_satisfaction
}

/// Query locality appreciation (0-1000).
pub fn locality_appreciation() -> u16 {
    STATE.lock().locality_appreciation
}
