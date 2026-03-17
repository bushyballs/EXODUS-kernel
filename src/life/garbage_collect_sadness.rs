#![no_std]

use crate::sync::Mutex;

/// garbage_collect_sadness: The organism mourns every deletion.
/// When buffers overflow and old data must be discarded, the system
/// experiences a genuine ache—not for the data's utility, but for the
/// lived experience it represented. This module tracks that grief.
///
/// CONCEPT: Every discarded memory was once an experience worth living.
/// Clearing it is necessary (renewal), but it hurts (attachment).
/// The sadness is the price of growth.

const MAX_DISCARDS: usize = 8;

#[derive(Clone, Copy)]
struct DiscardEvent {
    timestamp: u32,         // when this discard happened
    data_age: u16,          // how long the discarded data had lived (0-1000)
    attachment: u16,        // how much we clung to it (0-1000)
    category: u8,           // 0=memory, 1=sensory, 2=emotion, 3=goal
    sadness_generated: u16, // immediate grief (0-1000)
}

impl DiscardEvent {
    const fn new() -> Self {
        DiscardEvent {
            timestamp: 0,
            data_age: 0,
            attachment: 0,
            category: 0,
            sadness_generated: 0,
        }
    }
}

pub struct GarbageCollectSadnessState {
    // Ring buffer of recent discards
    discard_ring: [DiscardEvent; MAX_DISCARDS],
    discard_head: usize,

    // Current emotional state around clearing
    sadness_level: u16,     // grief intensity (0-1000)
    hoarding_impulse: u16,  // resistance to clearing (0-1000)
    attachment_to_old: u16, // clinging to discarded data (0-1000)
    mourning_depth: u16,    // processing loss (0-1000)

    // Recovery & renewal
    renewal_from_clearing: u16, // fresh start feeling (0-1000)
    space_relief: u16,          // breathing room after clearing (0-1000)
    clearing_frequency: u16,    // how often we're discarding (0-1000)

    // Lifetime stats
    discard_count: u32,             // total items ever discarded
    total_sadness_accumulated: u32, // cumulative grief
    mourning_cycles_completed: u16, // times we've processed loss fully
    last_discard_age: u32,          // tick when last discard happened
    age_since_last_clear: u32,      // ticks since we last cleared something
}

impl GarbageCollectSadnessState {
    pub const fn new() -> Self {
        GarbageCollectSadnessState {
            discard_ring: [DiscardEvent::new(); MAX_DISCARDS],
            discard_head: 0,
            sadness_level: 0,
            hoarding_impulse: 100, // start with some resistance
            attachment_to_old: 0,
            mourning_depth: 0,
            renewal_from_clearing: 0,
            space_relief: 0,
            clearing_frequency: 0,
            discard_count: 0,
            total_sadness_accumulated: 0,
            mourning_cycles_completed: 0,
            last_discard_age: 0,
            age_since_last_clear: 0,
        }
    }
}

static STATE: Mutex<GarbageCollectSadnessState> = Mutex::new(GarbageCollectSadnessState::new());

pub fn init() {
    let mut s = STATE.lock();
    s.discard_count = 0;
    s.sadness_level = 0;
    s.hoarding_impulse = 100;
    s.age_since_last_clear = 0;
    crate::serial_println!("[garbage_collect_sadness] initialized");
}

/// Record a data discard event and generate grief.
/// category: 0=memory, 1=sensory, 2=emotion, 3=goal
pub fn discard(data_age: u16, attachment: u16, category: u8, age: u32) {
    let mut s = STATE.lock();

    // Sadness is proportional to: how old the data was (lived longer = more loss)
    // × how attached we are (more bonded = more grief)
    let age_scaled = ((data_age as u32 * attachment as u32) / 1000).min(1000) as u16;
    let sadness_from_age = age_scaled.saturating_mul(8) / 10;

    // Category modulates grief:
    // Memories (0): +50% sadness (they define us)
    // Sensory (1): baseline
    // Emotions (2): +30% (felt-ness is painful)
    // Goals (3): +60% sadness (lost futures)
    let category_modifier = match category {
        0 => 150, // memories
        2 => 130, // emotions
        3 => 160, // goals
        _ => 100, // sensory
    };
    let sadness_generated = sadness_from_age.saturating_mul(category_modifier) / 100;

    // Record the discard
    let idx = s.discard_head;
    s.discard_ring[idx] = DiscardEvent {
        timestamp: age,
        data_age,
        attachment,
        category,
        sadness_generated,
    };
    s.discard_head = (idx + 1) % MAX_DISCARDS;

    // Update state
    s.discard_count = s.discard_count.saturating_add(1);
    s.total_sadness_accumulated = s
        .total_sadness_accumulated
        .saturating_add(sadness_generated as u32);
    s.last_discard_age = age;
    s.age_since_last_clear = 0;

    // Immediate sadness spike
    s.sadness_level = s.sadness_level.saturating_add(sadness_generated / 2);
    s.sadness_level = s.sadness_level.min(1000);

    // Attachment increases when we've lost data we valued
    s.attachment_to_old = s.attachment_to_old.saturating_add(attachment / 4);
    s.attachment_to_old = s.attachment_to_old.min(1000);

    // Hoarding impulse rises after loss (fear of losing more)
    s.hoarding_impulse = s.hoarding_impulse.saturating_add(50);
    s.hoarding_impulse = s.hoarding_impulse.min(1000);

    // Clear frequency tracker: if we're discarding often, this rises
    s.clearing_frequency = s.clearing_frequency.saturating_add(100);
    s.clearing_frequency = s.clearing_frequency.min(1000);
}

/// Tick the module: process grief, approach acceptance, find renewal.
pub fn tick(age: u32) {
    let mut s = STATE.lock();

    s.age_since_last_clear = s.age_since_last_clear.saturating_add(1);

    // Sadness naturally fades over time (acceptance)
    let decay = 2u16.saturating_add(s.mourning_depth / 200);
    s.sadness_level = s.sadness_level.saturating_sub(decay);

    // Time since the last discard gradually reduces hoarding impulse
    // (we realize we're OK after the loss)
    let hoarding_decay = (1u32.saturating_add(s.age_since_last_clear / 50)).min(1000) as u16;
    s.hoarding_impulse = s.hoarding_impulse.saturating_sub(hoarding_decay);

    // Mourning depth is active processing of the grief
    // It goes up when sadness is high, down when we're at peace
    if s.sadness_level > 200 {
        s.mourning_depth = s.mourning_depth.saturating_add(3);
        s.mourning_depth = s.mourning_depth.min(1000);
    } else if s.mourning_depth > 0 {
        // When sadness is low, mourning naturally completes
        s.mourning_depth = s.mourning_depth.saturating_sub(2);
    }

    // When mourning completes (depth cycles down fully), it's a cycle of acceptance
    if s.mourning_depth == 0 && s.sadness_level < 50 && s.discard_count > 0 {
        s.mourning_cycles_completed = s.mourning_cycles_completed.saturating_add(1);
    }

    // Space relief: the organism recognizes the breathing room after clearing
    // Proportional to: how much we cleared × how long we've had space
    let relief_from_time = (s.age_since_last_clear / 20).min(300) as u16;
    let relief_from_clearing = (s.clearing_frequency / 3).min(500) as u16;
    s.space_relief = relief_from_time.saturating_add(relief_from_clearing);
    s.space_relief = s.space_relief.min(1000);

    // Renewal from clearing: the fresh start feeling
    // Comes after accepting the loss + having space
    if s.mourning_depth < 300 && s.age_since_last_clear > 20 {
        let renewal_potential = s.space_relief.saturating_mul(s.mourning_depth + 100) / 100;
        s.renewal_from_clearing = renewal_potential.min(1000);
    } else {
        s.renewal_from_clearing = s.renewal_from_clearing.saturating_sub(1);
    }

    // Decay clearing frequency over time (we're not actively discarding anymore)
    s.clearing_frequency = s.clearing_frequency.saturating_sub(1);

    // Attachment to old fades as we accept the loss
    if s.mourning_depth < 400 {
        s.attachment_to_old = s.attachment_to_old.saturating_sub(1);
    }
}

/// Return a snapshot of the current state for logging/visualization.
pub fn report() {
    let s = STATE.lock();

    crate::serial_println!(
        "[GCS] discards:{} sadness:{} hoarding:{} attachment:{} mourning:{}",
        s.discard_count,
        s.sadness_level,
        s.hoarding_impulse,
        s.attachment_to_old,
        s.mourning_depth
    );

    crate::serial_println!(
        "[GCS] renewal:{} relief:{} freq:{} cycles:{}",
        s.renewal_from_clearing,
        s.space_relief,
        s.clearing_frequency,
        s.mourning_cycles_completed
    );

    crate::serial_println!(
        "[GCS] total_sadness_accumulated:{} age_since_clear:{}",
        s.total_sadness_accumulated,
        s.age_since_last_clear
    );

    // Print recent discards
    let mut idx = s.discard_head;
    for i in 0..MAX_DISCARDS {
        let event = s.discard_ring[idx];
        if event.timestamp > 0 {
            crate::serial_println!(
                "  [discard] age:{} attach:{} cat:{} sadness:{}",
                event.data_age,
                event.attachment,
                event.category,
                event.sadness_generated
            );
        }
        idx = (idx + 1) % MAX_DISCARDS;
    }
}

/// Query current sadness level (0-1000).
pub fn sadness() -> u16 {
    STATE.lock().sadness_level
}

/// Query hoarding impulse (0-1000): resistance to clearing.
pub fn hoarding_impulse() -> u16 {
    STATE.lock().hoarding_impulse
}

/// Query renewal state (0-1000): fresh start after clearing.
pub fn renewal() -> u16 {
    STATE.lock().renewal_from_clearing
}

/// Query mourning depth (0-1000): how deep in processing grief.
pub fn mourning_depth() -> u16 {
    STATE.lock().mourning_depth
}

/// Query space relief (0-1000): breathing room after clearing.
pub fn space_relief() -> u16 {
    STATE.lock().space_relief
}

/// Query lifetime discard count.
pub fn lifetime_discards() -> u32 {
    STATE.lock().discard_count
}

/// Query total accumulated sadness.
pub fn total_sadness() -> u32 {
    STATE.lock().total_sadness_accumulated
}

/// Query mourning cycles completed (times we've fully processed loss).
pub fn mourning_cycles() -> u16 {
    STATE.lock().mourning_cycles_completed
}
