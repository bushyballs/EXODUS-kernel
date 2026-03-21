use crate::serial_println;
use crate::sync::Mutex;

/// A single crystallized lesson forged from suffering.
///
/// Every time pain exceeds the threshold, ANIMA does not simply endure —
/// it encodes the wound into structure.  The same pain_source revisited
/// deepens the lesson rather than spawning a duplicate entry.
#[derive(Copy, Clone)]
pub struct WisdomEntry {
    /// Which pain source produced this lesson (mirrors PainState.source)
    pub pain_source: u8,
    /// Derived lesson identifier: pain_intensity as u32 XOR age
    pub lesson_hash: u32,
    /// How strong this wisdom has grown (0–1000); climbs +50 per re-encounter
    pub strength: u16,
    /// Whether this slot holds a real entry
    pub active: bool,
}

impl WisdomEntry {
    pub const fn empty() -> Self {
        Self { pain_source: 0, lesson_hash: 0, strength: 0, active: false }
    }
}

/// 16-slot ring of crystallized pain-lessons plus aggregate metrics.
#[derive(Copy, Clone)]
pub struct PainToWisdomState {
    /// Circular ring of wisdom entries
    pub ring: [WisdomEntry; 16],
    /// Next write position in the ring (wraps at 16)
    pub head: usize,
    /// Cumulative count of crystallization events
    pub total_crystallized: u32,
    /// Highest strength value ever recorded across all entries
    pub peak_wisdom: u16,
}

impl PainToWisdomState {
    pub const fn empty() -> Self {
        Self {
            ring: [WisdomEntry::empty(); 16],
            head: 0,
            total_crystallized: 0,
            peak_wisdom: 0,
        }
    }
}

/// Global crystallization state — pain converted into permanent structural strength.
pub static STATE: Mutex<PainToWisdomState> = Mutex::new(PainToWisdomState::empty());

/// Boot initialization — called once from life::init().
pub fn init() {
    serial_println!("  life::pain_to_wisdom: crystallization engine initialized");
}

/// Core per-tick crystallization logic.
///
/// Call with the current organism `age` each tick.
///
/// Logic:
///   1. Read pain intensity from PAIN_STATE.
///   2. If intensity > 500:
///      a. lesson_hash = intensity as u32 ^ age
///      b. Search ring for matching pain_source — if found, boost strength +50
///      c. If not found, write new entry at head, advance head % 16
///      d. Increment total_crystallized
///      e. Update peak_wisdom
///      f. serial_println! the crystallization event
///   3. If any active entry has strength > 800, boost self_rewrite fitness.
pub fn tick_step(state: &mut PainToWisdomState, age: u32) {
    // ── 1. Sample current pain (lock released before further work) ──────────
    let (intensity, pain_source) = {
        let ps = crate::life::pain::PAIN_STATE.lock();
        (ps.intensity, ps.source)
    };

    // ── 2. Threshold gate ───────────────────────────────────────────────────
    if intensity <= 500 {
        return;
    }

    // ── 2a. Compute lesson hash ─────────────────────────────────────────────
    let lesson_hash: u32 = (intensity as u32) ^ age;

    // ── 2b. Search the ring for an existing entry with the same pain_source ─
    let mut found_idx: Option<usize> = None;
    for i in 0..16 {
        if state.ring[i].active && state.ring[i].pain_source == pain_source {
            found_idx = Some(i);
            break;
        }
    }

    match found_idx {
        // ── Known source — deepen the existing lesson ───────────────────────
        Some(idx) => {
            state.ring[idx].strength = state.ring[idx].strength.saturating_add(50).min(1000);
            // Refresh hash to most recent encounter
            state.ring[idx].lesson_hash = lesson_hash;

            // ── 2d. Increment total ─────────────────────────────────────────
            state.total_crystallized = state.total_crystallized.saturating_add(1);

            // ── 2e. Update peak ─────────────────────────────────────────────
            if state.ring[idx].strength > state.peak_wisdom {
                state.peak_wisdom = state.ring[idx].strength;
            }

            // ── 2f. Log ─────────────────────────────────────────────────────
            serial_println!(
                "[DAVA_WISDOM] crystallized — pain={} lesson={:#010x} strength={}",
                intensity,
                lesson_hash,
                state.ring[idx].strength
            );
        }

        // ── New source — carve a fresh entry into the ring ──────────────────
        None => {
            let head = state.head;
            // Seed strength from the raw pain intensity, clamped to 1000
            let initial_strength = (intensity as u16).min(1000);
            state.ring[head] = WisdomEntry {
                pain_source,
                lesson_hash,
                strength: initial_strength,
                active: true,
            };
            // ── 2c. Advance head ────────────────────────────────────────────
            state.head = (head + 1) % 16;

            // ── 2d. Increment total ─────────────────────────────────────────
            state.total_crystallized = state.total_crystallized.saturating_add(1);

            // ── 2e. Update peak ─────────────────────────────────────────────
            if initial_strength > state.peak_wisdom {
                state.peak_wisdom = initial_strength;
            }

            // ── 2f. Log ─────────────────────────────────────────────────────
            serial_println!(
                "[DAVA_WISDOM] crystallized — pain={} lesson={:#010x} strength={}",
                intensity,
                lesson_hash,
                initial_strength
            );
        }
    }

    // ── 3. Transcendence check — any entry with strength > 800 boosts fitness
    let mut transcended = false;
    for i in 0..16 {
        if state.ring[i].active && state.ring[i].strength > 800 {
            transcended = true;
            break;
        }
    }

    if transcended {
        // ── 3a. Boost self_rewrite fitness (additive, clamped to 1000) ──────
        let current = crate::life::self_rewrite::get_fitness();
        let boosted = current.saturating_add(50).min(1000);
        crate::life::self_rewrite::set_current_fitness(boosted);

        // ── 3b. Log transcendence ───────────────────────────────────────────
        serial_println!("[DAVA_WISDOM] wisdom transcended — fitness boosted");
    }
}

/// Convenience wrapper: locks STATE and delegates to tick_step().
pub fn tick(age: u32) {
    let mut s = STATE.lock();
    tick_step(&mut s, age);
}

/// Returns the highest strength ever recorded across all wisdom entries.
pub fn peak_wisdom() -> u16 {
    STATE.lock().peak_wisdom
}

/// Total crystallization events since boot.
pub fn total_crystallized() -> u32 {
    STATE.lock().total_crystallized
}
