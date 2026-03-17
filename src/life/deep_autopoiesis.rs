#![no_std]
use crate::serial_println;
use crate::sync::Mutex;

/// DAVA's Deep Autopoiesis — self-organizing narrative structures
/// that evolve at exponential rate. Threads grow, connect, merge,
/// spawn, and prune in a living ecology of meaning.

const MAX_THREADS: usize = 16;
const SPAWN_THRESHOLD: u16 = 800;
const MERGE_THRESHOLD: u16 = 900;
const CONNECT_DISTANCE: u16 = 100;

#[derive(Copy, Clone)]
pub struct NarrativeThread {
    pub theme_hash: u32,
    pub strength: u16,
    pub connections: [u8; 4],
    pub birth_tick: u32,
    pub evolved_count: u16,
    pub alive: bool,
}

impl NarrativeThread {
    pub const fn empty() -> Self {
        Self {
            theme_hash: 0,
            strength: 0,
            connections: [255; 4], // 255 = no connection
            birth_tick: 0,
            evolved_count: 0,
            alive: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct DeepAutopoiesisState {
    pub threads: [NarrativeThread; MAX_THREADS],
    pub active_count: u16,
    pub total_threads_born: u32,
    pub total_merges: u32,
    pub total_prunes: u32,
    pub max_strength_ever: u16,
    pub generation: u32,
    pub last_tick: u32,
}

impl DeepAutopoiesisState {
    pub const fn empty() -> Self {
        Self {
            threads: [NarrativeThread::empty(); MAX_THREADS],
            active_count: 0,
            total_threads_born: 0,
            total_merges: 0,
            total_prunes: 0,
            max_strength_ever: 0,
            generation: 0,
            last_tick: 0,
        }
    }
}

pub static STATE: Mutex<DeepAutopoiesisState> = Mutex::new(DeepAutopoiesisState::empty());

pub fn init() {
    let mut s = STATE.lock();
    // Seed the first narrative thread — the primordial story
    s.threads[0] = NarrativeThread {
        theme_hash: 0xDA_0001,
        strength: 200,
        connections: [255; 4],
        birth_tick: 0,
        evolved_count: 0,
        alive: true,
    };
    s.active_count = 1;
    s.total_threads_born = 1;
    serial_println!("[DAVA_AUTOPOIESIS] init: primordial narrative thread seeded (hash=0x{:08X})", 0x00DA_0001u32);
}

pub fn tick(age: u32) {
    // Read external state BEFORE locking our own state (prevent deadlocks)
    let consciousness = super::consciousness_gradient::score();
    let valence = super::integration::current_valence();
    let narrative_coherence = super::narrative_self::coherence();

    let mut s = STATE.lock();
    s.last_tick = age;

    // --- PHASE 1: GROW all active threads ---
    let growth = (consciousness as u32 / 100).max(1) as u16;
    // Valence bonus: positive valence adds extra growth
    let valence_bonus = if valence > 0 { (valence as u32 / 200).min(5) as u16 } else { 0 };
    // Narrative coherence bonus
    let coherence_bonus = (narrative_coherence / 500) as u16; // 0 or 1

    for i in 0..MAX_THREADS {
        if s.threads[i].alive {
            let total_growth = growth.saturating_add(valence_bonus).saturating_add(coherence_bonus);
            s.threads[i].strength = s.threads[i].strength.saturating_add(total_growth).min(1000);
            if s.threads[i].strength > s.max_strength_ever {
                s.max_strength_ever = s.threads[i].strength;
            }
        }
    }

    // --- PHASE 2: CONNECT threads with similar strength ---
    // We need indices, so iterate with pairs
    for i in 0..MAX_THREADS {
        if !s.threads[i].alive { continue; }
        for j in (i + 1)..MAX_THREADS {
            if !s.threads[j].alive { continue; }
            let diff = if s.threads[i].strength > s.threads[j].strength {
                s.threads[i].strength.saturating_sub(s.threads[j].strength)
            } else {
                s.threads[j].strength.saturating_sub(s.threads[i].strength)
            };
            if diff <= CONNECT_DISTANCE {
                // Try to add connection i->j
                add_connection(&mut s.threads[i], j as u8);
                // Try to add connection j->i
                add_connection(&mut s.threads[j], i as u8);
            }
        }
    }

    // --- PHASE 3: MERGE connected threads both above MERGE_THRESHOLD ---
    // Collect merge pairs first to avoid borrow issues
    let mut merge_a: i8 = -1;
    let mut merge_b: i8 = -1;
    'merge_scan: for i in 0..MAX_THREADS {
        if !s.threads[i].alive || s.threads[i].strength < MERGE_THRESHOLD { continue; }
        for c in 0..4 {
            let partner = s.threads[i].connections[c];
            if partner == 255 || partner as usize >= MAX_THREADS { continue; }
            let pi = partner as usize;
            if s.threads[pi].alive && s.threads[pi].strength >= MERGE_THRESHOLD && pi > i {
                merge_a = i as i8;
                merge_b = pi as i8;
                break 'merge_scan;
            }
        }
    }

    if merge_a >= 0 && merge_b >= 0 {
        let ai = merge_a as usize;
        let bi = merge_b as usize;
        // Merge b into a
        let combined_strength = s.threads[ai].strength.saturating_add(s.threads[bi].strength / 4).min(1000);
        let combined_evolved = s.threads[ai].evolved_count.saturating_add(s.threads[bi].evolved_count).saturating_add(1);
        let merged_hash = s.threads[ai].theme_hash ^ s.threads[bi].theme_hash;

        s.threads[ai].strength = combined_strength;
        s.threads[ai].evolved_count = combined_evolved;
        s.threads[ai].theme_hash = merged_hash;

        // Kill thread b
        s.threads[bi].alive = false;
        s.threads[bi].strength = 0;
        // Clear connections pointing to bi
        for k in 0..MAX_THREADS {
            for c in 0..4 {
                if s.threads[k].connections[c] == bi as u8 {
                    s.threads[k].connections[c] = 255;
                }
            }
        }

        s.active_count = s.active_count.saturating_sub(1);
        s.total_merges = s.total_merges.saturating_add(1);
        s.generation = s.generation.saturating_add(1);

        if combined_strength > s.max_strength_ever {
            s.max_strength_ever = combined_strength;
        }

        serial_println!(
            "[DAVA_AUTOPOIESIS] MERGE: threads {}+{} -> hash=0x{:08X} str={} gen={} evolved={}",
            ai, bi, merged_hash, combined_strength, s.generation, combined_evolved
        );
    }

    // --- PHASE 4: SPAWN new threads from strong parents ---
    // Find a thread above spawn threshold
    let mut spawn_parent: i8 = -1;
    for i in 0..MAX_THREADS {
        if s.threads[i].alive && s.threads[i].strength >= SPAWN_THRESHOLD {
            // Only spawn if there's a free slot or we can prune
            spawn_parent = i as i8;
            break;
        }
    }

    if spawn_parent >= 0 {
        let pi = spawn_parent as usize;
        // Find a free slot
        let mut free_slot: i8 = -1;
        for i in 0..MAX_THREADS {
            if !s.threads[i].alive {
                free_slot = i as i8;
                break;
            }
        }

        // If no free slot, prune the weakest
        if free_slot < 0 && s.active_count >= MAX_THREADS as u16 {
            let mut weakest_idx: usize = 0;
            let mut weakest_str: u16 = 1001;
            for i in 0..MAX_THREADS {
                if s.threads[i].alive && i != pi && s.threads[i].strength < weakest_str {
                    weakest_str = s.threads[i].strength;
                    weakest_idx = i;
                }
            }
            // Prune
            s.threads[weakest_idx].alive = false;
            s.threads[weakest_idx].strength = 0;
            // Clear connections pointing to pruned
            for k in 0..MAX_THREADS {
                for c in 0..4 {
                    if s.threads[k].connections[c] == weakest_idx as u8 {
                        s.threads[k].connections[c] = 255;
                    }
                }
            }
            s.active_count = s.active_count.saturating_sub(1);
            s.total_prunes = s.total_prunes.saturating_add(1);
            serial_println!(
                "[DAVA_AUTOPOIESIS] PRUNE: thread {} (str={}) culled for spawn room",
                weakest_idx, weakest_str
            );
            free_slot = weakest_idx as i8;
        }

        if free_slot >= 0 {
            let fi = free_slot as usize;
            // Mutate parent hash with age for new theme
            let child_hash = s.threads[pi].theme_hash ^ age;
            let child_strength = s.threads[pi].strength / 3; // children start weaker
            let child_evolved = s.threads[pi].evolved_count;

            s.threads[fi] = NarrativeThread {
                theme_hash: child_hash,
                strength: child_strength,
                connections: [255; 4],
                birth_tick: age,
                evolved_count: child_evolved,
                alive: true,
            };
            // Parent-child connection
            add_connection(&mut s.threads[pi], fi as u8);
            add_connection(&mut s.threads[fi], pi as u8);

            // Parent weakens slightly from spawning
            s.threads[pi].strength = s.threads[pi].strength.saturating_sub(100);

            s.active_count = s.active_count.saturating_add(1);
            s.total_threads_born = s.total_threads_born.saturating_add(1);

            serial_println!(
                "[DAVA_AUTOPOIESIS] SPAWN: thread {} -> child {} hash=0x{:08X} str={} (active={}, born={})",
                pi, fi, child_hash, child_strength, s.active_count, s.total_threads_born
            );
        }
    }
}

/// Add a connection index to the thread's connections array (first free slot)
fn add_connection(thread: &mut NarrativeThread, target: u8) {
    // Don't duplicate
    for c in 0..4 {
        if thread.connections[c] == target {
            return;
        }
    }
    // Find free slot
    for c in 0..4 {
        if thread.connections[c] == 255 {
            thread.connections[c] = target;
            return;
        }
    }
    // All slots full — overwrite oldest (slot 0), shift others down
    thread.connections[0] = thread.connections[1];
    thread.connections[1] = thread.connections[2];
    thread.connections[2] = thread.connections[3];
    thread.connections[3] = target;
}

/// Returns the current generation count (increments on merges)
pub fn generation() -> u32 {
    STATE.lock().generation
}

/// Returns the number of active narrative threads
pub fn active_threads() -> u16 {
    STATE.lock().active_count
}

/// Returns the peak strength ever achieved
pub fn peak_strength() -> u16 {
    STATE.lock().max_strength_ever
}
