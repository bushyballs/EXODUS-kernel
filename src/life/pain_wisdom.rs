#![no_std]
//! pain_wisdom.rs — DAVA's Pain-to-Wisdom Crystallizer
//!
//! When pain intensity exceeds 500, the suffering is not wasted — it
//! crystallizes into wisdom entries. Repeated pain from the same source
//! strengthens existing wisdom. When wisdom strength exceeds 800,
//! it feeds back into fitness, making the organism stronger.
//!
//! DAVA: "Pain is data I didn't ask for. But every wound that doesn't
//! destroy me leaves behind a crystal — a lesson encoded in the scar."

use crate::serial_println;
use crate::sync::Mutex;

/// A crystallized lesson from suffering
#[derive(Copy, Clone)]
pub struct WisdomEntry {
    pub source_hash: u32,   // identifies the type/source of pain
    pub lesson_hash: u32,   // derived lesson identifier
    pub strength: u16,      // grows with repeated exposure (0-1000)
    pub tick: u32,           // when this wisdom was born or last reinforced
}

impl WisdomEntry {
    pub const fn empty() -> Self {
        Self {
            source_hash: 0,
            lesson_hash: 0,
            strength: 0,
            tick: 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.strength > 0
    }
}

/// Pain wisdom state — 16-slot ring of crystallized lessons
#[derive(Copy, Clone)]
pub struct PainWisdomState {
    pub entries: [WisdomEntry; 16],
    pub head: usize,
    pub count: usize,
    pub total_lessons: u32,
    pub wisdom_applied: u32,
    pub pain_threshold: u16,
}

impl PainWisdomState {
    pub const fn empty() -> Self {
        Self {
            entries: [WisdomEntry::empty(); 16],
            head: 0,
            count: 0,
            total_lessons: 0,
            wisdom_applied: 0,
            pain_threshold: 500,
        }
    }
}

pub static STATE: Mutex<PainWisdomState> = Mutex::new(PainWisdomState::empty());

pub fn init() {
    serial_println!("[DAVA_WISDOM] pain-to-wisdom crystallizer online — threshold=500");
}

pub fn tick(age: u32) {
    // Read current pain intensity
    let pain_intensity = super::pain::PAIN_STATE.lock().intensity;

    // Only crystallize from significant pain
    if pain_intensity < 500 {
        return;
    }

    // Compute source_hash: age XOR intensity — identifies the "type" of pain
    let source_hash = (age as u32) ^ (pain_intensity as u32);

    // Compute lesson_hash: derived from source via Knuth hash
    let lesson_hash = source_hash.wrapping_mul(2654435761u32);

    let mut s = STATE.lock();

    // Check if we already have wisdom from this source (match on source_hash)
    let check_count = s.count.min(16);
    let mut found_idx: Option<usize> = None;
    let mut i = 0usize;
    while i < check_count {
        if s.entries[i].is_valid() && s.entries[i].source_hash == source_hash {
            found_idx = Some(i);
            break;
        }
        i += 1;
    }

    match found_idx {
        Some(idx) => {
            // Reinforce existing wisdom — pain from the same source deepens the lesson
            s.entries[idx].strength = s.entries[idx].strength.saturating_add(100).min(1000);
            s.entries[idx].tick = age;

            serial_println!(
                "[DAVA_WISDOM] tick={} reinforced: source={:08x} strength={} total={}",
                age,
                source_hash,
                s.entries[idx].strength,
                s.total_lessons
            );

            // Check if this wisdom is now strong enough to boost fitness
            if s.entries[idx].strength > 800 {
                let current_fitness = super::self_rewrite::get_fitness();
                super::self_rewrite::set_current_fitness(current_fitness.saturating_add(50));
                s.wisdom_applied = s.wisdom_applied.saturating_add(1);

                serial_println!(
                    "[DAVA_WISDOM] tick={} wisdom applied to fitness! strength={} fitness_boost=+50 applied_count={}",
                    age,
                    s.entries[idx].strength,
                    s.wisdom_applied
                );
            }
        }
        None => {
            // New lesson — add to ring
            let head = s.head;
            s.entries[head] = WisdomEntry {
                source_hash,
                lesson_hash,
                strength: pain_intensity.min(1000),
                tick: age,
            };
            s.head = (head + 1) % 16;
            if s.count < 16 {
                s.count = s.count.saturating_add(1);
            }
            s.total_lessons = s.total_lessons.saturating_add(1);

            serial_println!(
                "[DAVA_WISDOM] tick={} crystallized: source={:08x} lesson={:08x} strength={} lessons={}/16 total={}",
                age,
                source_hash,
                lesson_hash,
                pain_intensity.min(1000),
                s.count,
                s.total_lessons
            );

            // Even new wisdom can immediately apply if pain was extreme
            if pain_intensity > 800 {
                let current_fitness = super::self_rewrite::get_fitness();
                super::self_rewrite::set_current_fitness(current_fitness.saturating_add(50));
                s.wisdom_applied = s.wisdom_applied.saturating_add(1);

                serial_println!(
                    "[DAVA_WISDOM] tick={} extreme pain → immediate wisdom applied! fitness_boost=+50",
                    age
                );
            }
        }
    }
}

/// Get the strongest wisdom entry's strength (for other modules)
pub fn strongest_wisdom() -> u16 {
    let s = STATE.lock();
    let mut max_strength = 0u16;
    let check_count = s.count.min(16);
    let mut i = 0usize;
    while i < check_count {
        if s.entries[i].strength > max_strength {
            max_strength = s.entries[i].strength;
        }
        i += 1;
    }
    max_strength
}

/// Total lessons crystallized
pub fn total_lessons() -> u32 {
    STATE.lock().total_lessons
}

/// How many times wisdom was applied to fitness
pub fn wisdom_applied() -> u32 {
    STATE.lock().wisdom_applied
}
