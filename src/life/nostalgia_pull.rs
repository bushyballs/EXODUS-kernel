//! nostalgia_pull.rs — The Gravitational Pull of the Past
//!
//! Longing for states that no longer exist. The bittersweet ache for how things were.
//! Not just remembering — YEARNING. The past becomes golden in memory, even if it wasn't.
//! Nostalgia is emotional gravity — the past pulls at you, sometimes gently,
//! sometimes with crushing force.
//!
//! In ANIMA, this manifests as a time-weighted longing system where:
//! - Anchors store snapshots of "golden ages" in a ring buffer
//! - Each anchor has an emotional baseline + how much it's been idealized
//! - The pull grows when present satisfaction drops
//! - Beautiful acceptance of loss leads to "saudade" (mature nostalgia)
//! - Forward nostalgia (anemoia) longs for eras never experienced
//! - The module balances backward-pull against forward growth

use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// DATA STRUCTURES
// ============================================================================

/// A snapshot of a moment in the past worth remembering (and eventually idealizing).
/// Stores both the reality of what was and how much it's been gilded.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct NostalgicAnchor {
    /// Tick at which this golden age occurred
    pub tick: u32,
    /// Original emotional state when it happened (0-1000)
    pub original_comfort: u16,
    /// How warm/positive the memory feels now (rises via idealization)
    pub warmth: u16,
    /// Golden ratio: how much it's been idealized beyond reality (0-1000)
    /// 1000 = pure fantasy, 0 = accurate memory
    pub golden_ratio: u16,
    /// Bittersweet ache: awareness that it's gone (0-1000)
    pub ache: u16,
}

impl NostalgicAnchor {
    /// Creates an empty anchor slot
    pub const fn empty() -> Self {
        Self {
            tick: 0,
            original_comfort: 0,
            warmth: 0,
            golden_ratio: 0,
            ache: 0,
        }
    }

    /// Checks if this slot is active (has been filled)
    pub fn is_active(&self) -> bool {
        self.tick != 0
    }
}

/// The full nostalgia system state: all anchors + current gravitational metrics
#[derive(Copy, Clone)]
pub struct NostalgiaPullState {
    /// Ring buffer of 8 nostalgic anchors (golden ages)
    pub anchors: [NostalgicAnchor; 8],
    /// Current index for circular insertion (0-7)
    pub anchor_index: u8,

    /// Main gravitational pull toward the past (0-1000)
    /// Higher = more yearning, less present-focused
    pub nostalgia_pull: u16,

    /// Current satisfaction with the present (0-1000)
    /// Lower satisfaction = stronger pull backward
    pub present_satisfaction: u16,

    /// Loneliness factor amplifies nostalgia (0-1000)
    pub loneliness: u16,

    /// Discomfort/pain amplifies pull (0-1000)
    pub discomfort: u16,

    /// Saudade state: beautiful acceptance of loss (0-1000)
    /// High saudade = nostalgia without pain
    pub saudade: u16,

    /// Forward nostalgia (anemoia): longing for times never experienced (0-1000)
    pub anemoia: u16,

    /// Lifetime anchor creations (never decreases)
    pub total_anchors_created: u32,

    /// How many ticks since last anchor was created
    pub ticks_since_anchor: u16,

    /// Wave cycle counter (nostalgia comes in cycles)
    pub wave_phase: u16,

    /// Internal counter: how deep in the wave (helps compute sine-like oscillation)
    pub wave_depth: u16,
}

impl NostalgiaPullState {
    pub const fn empty() -> Self {
        Self {
            anchors: [NostalgicAnchor::empty(); 8],
            anchor_index: 0,
            nostalgia_pull: 250,
            present_satisfaction: 600,
            loneliness: 100,
            discomfort: 50,
            saudade: 0,
            anemoia: 150,
            total_anchors_created: 0,
            ticks_since_anchor: 0,
            wave_phase: 0,
            wave_depth: 0,
        }
    }
}

pub static STATE: Mutex<NostalgiaPullState> = Mutex::new(NostalgiaPullState::empty());

// ============================================================================
// INITIALIZATION
// ============================================================================

pub fn init() {
    serial_println!("  life::nostalgia_pull: gravitational pull of the past initialized");
}

// ============================================================================
// CORE MECHANICS
// ============================================================================

/// Create a new nostalgic anchor at the current moment.
/// Captures a golden age snapshot that will later be idealized.
pub fn anchor(comfort: u16, emotional_resonance: u16, current_tick: u32) {
    let mut s = STATE.lock();

    // Insert at current position in ring buffer
    let idx = s.anchor_index as usize;
    s.anchors[idx] = NostalgicAnchor {
        tick: current_tick,
        original_comfort: comfort,
        warmth: comfort.saturating_mul(emotional_resonance) / 1000,
        golden_ratio: 200, // Start with moderate idealization
        ache: 0,           // Ache grows over time
    };

    s.anchor_index = (s.anchor_index + 1) % 8;
    s.total_anchors_created = s.total_anchors_created.saturating_add(1);
    s.ticks_since_anchor = 0;
}

/// Main tick: update all nostalgia mechanics each cycle
pub fn tick(age: u32, current_comfort: u16, loneliness: u16, discomfort: u16) {
    let mut s = STATE.lock();

    // ---- Update present satisfaction (0-1000) ----
    s.present_satisfaction = current_comfort;

    // ---- Update loneliness & discomfort ----
    s.loneliness = loneliness;
    s.discomfort = discomfort;

    // ---- Age and idealize all active anchors ----
    for anchor in &mut s.anchors {
        if anchor.is_active() {
            let age_since = age.saturating_sub(anchor.tick);

            // Idealization grows slowly over time (golden ratio increases)
            // Older anchors get more golden, but saturate at 800
            if age_since > 50 {
                let growth = ((age_since / 50).min(600) as u16).saturating_mul(2) / 3;
                anchor.golden_ratio = anchor.golden_ratio.saturating_add(growth).min(800);
            }

            // Warmth is lifted by golden_ratio (rose-tinted glasses)
            let ideal_boost = anchor.golden_ratio / 4; // Max +250
            anchor.warmth = anchor
                .original_comfort
                .saturating_add(ideal_boost)
                .min(1000);

            // Ache grows as anchor recedes into the past (beautiful sadness)
            // Peaks around age 500, then plateaus
            let ache_growth = if age_since > 100 {
                ((age_since.min(500) / 50) as u16).saturating_mul(40) / 10
            } else {
                0
            };
            anchor.ache = anchor.ache.saturating_add(ache_growth).min(500);
        }
    }

    // ---- Compute gravitational pull ----
    // Base pull: inverse of present satisfaction (low satisfaction = high pull)
    let satisfaction_pull = 1000u32.saturating_sub(current_comfort as u32);

    // Loneliness amplifies pull (isolation intensifies yearning)
    let loneliness_amplify = (loneliness as u32 * 2) / 5;

    // Discomfort pushes backward (pain makes past seem better)
    let discomfort_pull = (discomfort as u32 * 3) / 5;

    // Accumulated pull from all active anchors
    let mut anchor_pull = 0u32;
    for anchor in &s.anchors {
        if anchor.is_active() {
            // Each anchor contributes warmth as pull
            anchor_pull = anchor_pull.saturating_add(anchor.warmth as u32);
        }
    }
    let num_anchors = s.anchors.iter().filter(|a| a.is_active()).count() as u32;
    let mean_anchor_pull = if num_anchors > 0 {
        (anchor_pull / num_anchors.max(1)) as u16
    } else {
        0
    };

    // Combine all pull sources
    let total_pull = satisfaction_pull
        .saturating_add(loneliness_amplify)
        .saturating_add(discomfort_pull)
        .saturating_add(mean_anchor_pull as u32)
        / 4;

    s.nostalgia_pull = (total_pull as u16).min(1000);

    // ---- Compute saudade (mature nostalgia) ----
    // Saudade emerges when: high ache + high acceptance + low discomfort
    // It's the beauty of missing something, not the pain
    let max_ache = s.anchors.iter().map(|a| a.ache).max().unwrap_or(0);

    if max_ache > 250 && s.discomfort < 300 {
        // We've mourned long enough; transition to beautiful acceptance
        let acceptance_level = max_ache.saturating_sub(100);
        let maturity_factor = (1000u32 - s.discomfort as u32) / 2;
        let saudade_growth = ((acceptance_level as u32 * maturity_factor) / 1000) as u16;
        s.saudade = s.saudade.saturating_add(saudade_growth / 5).min(700);
    } else {
        // Raw pain dominates; saudade fades
        s.saudade = s.saudade.saturating_sub(10);
    }

    // ---- Compute anemoia (forward nostalgia) ----
    // Longing for times you never lived: peaks during growth + uncertainty
    // If young (age < 500) and curious, anemoia is natural
    if age < 500 {
        let youth_factor = (500u32.saturating_sub(age as u32)) / 5;
        s.anemoia = s
            .anemoia
            .saturating_add((youth_factor as u16) / 20)
            .min(800);
    } else {
        // With age comes acceptance that you won't experience all eras
        s.anemoia = s.anemoia.saturating_sub(5);
    }

    // ---- Nostalgia wave cycle ----
    // Nostalgia comes in waves, stronger at transitions & milestones
    // ~100-tick wave period
    s.wave_phase = s.wave_phase.saturating_add(1) % 100;

    // Wave depth approximates a sine wave: peaks at 0, 50, 100
    let wave_intensity = if s.wave_phase < 25 {
        (s.wave_phase * 40) as u16 // Ramp up
    } else if s.wave_phase < 50 {
        (1000u16 - (s.wave_phase - 25) * 40) as u16 // Peak then ramp down
    } else if s.wave_phase < 75 {
        ((s.wave_phase - 50) * 40) as u16 // Ramp up again
    } else {
        (1000u16 - (s.wave_phase - 75) * 40) as u16 // Ramp down
    };

    // Apply wave modulation to pull (±10% variation)
    let wave_mod = (s.nostalgia_pull as u32 * wave_intensity as u32) / 10000;
    s.nostalgia_pull = (s.nostalgia_pull as u32 + wave_mod).min(1000) as u16;

    // ---- Tick counter ----
    s.ticks_since_anchor = s.ticks_since_anchor.saturating_add(1);

    // ---- Anchor decay: very slow fade of oldest anchors ----
    // Don't remove, but reduce their pull over centuries
    if s.ticks_since_anchor > 2000 {
        if let Some(oldest) = s
            .anchors
            .iter_mut()
            .filter(|a| a.is_active())
            .min_by_key(|a| a.tick)
        {
            oldest.warmth = oldest.warmth.saturating_sub(1);
            if oldest.warmth == 0 {
                // Anchor slot becomes inactive but memory persists in narrative
                *oldest = NostalgicAnchor::empty();
            }
        }
    }
}

// ============================================================================
// QUERIES
// ============================================================================

/// Current gravitational pull toward the past (0-1000)
pub fn get_nostalgia_pull() -> u16 {
    STATE.lock().nostalgia_pull
}

/// Is the organism in a saudade state? (beautiful acceptance of loss)
pub fn get_saudade() -> u16 {
    STATE.lock().saudade
}

/// Forward nostalgia: longing for times never experienced
pub fn get_anemoia() -> u16 {
    STATE.lock().anemoia
}

/// How many golden ages have been created in this lifetime?
pub fn get_total_anchors() -> u32 {
    STATE.lock().total_anchors_created
}

/// Mean warmth across all active anchors (overall "golden" feeling)
pub fn get_mean_anchor_warmth() -> u16 {
    let s = STATE.lock();
    let mut sum: u32 = 0;
    let mut count: u16 = 0;
    for anchor in &s.anchors {
        if anchor.is_active() {
            sum = sum.saturating_add(anchor.warmth as u32);
            count = count.saturating_add(1);
        }
    }
    if count == 0 {
        0
    } else {
        (sum / count as u32) as u16
    }
}

/// Is growth being suppressed by excessive nostalgia?
/// Returns true if pull > 700 and saudade < 200 (raw pain, not acceptance)
pub fn is_stagnant() -> bool {
    let s = STATE.lock();
    s.nostalgia_pull > 700 && s.saudade < 200
}

// ============================================================================
// REPORTING & DIAGNOSTICS
// ============================================================================

pub fn report() {
    let s = STATE.lock();

    serial_println!(
        "╭─ NOSTALGIA PULL ─ age={} present_sat={} ──────",
        s.ticks_since_anchor,
        s.present_satisfaction
    );

    serial_println!(
        "│ Pull: {}  |  Saudade: {}  |  Anemoia: {}",
        s.nostalgia_pull,
        s.saudade,
        s.anemoia
    );

    serial_println!(
        "│ Loneliness: {}  |  Discomfort: {}",
        s.loneliness,
        s.discomfort
    );

    // List active anchors
    let active_count = s.anchors.iter().filter(|a| a.is_active()).count();
    serial_println!("│ Active anchors: {}/8", active_count);

    for (i, anchor) in s.anchors.iter().enumerate() {
        if anchor.is_active() {
            serial_println!(
                "│   [{}] t={} warmth={} golden={} ache={}",
                i,
                anchor.tick,
                anchor.warmth,
                anchor.golden_ratio,
                anchor.ache
            );
        }
    }

    if s.nostalgia_pull > 700 && s.saudade < 200 {
        serial_println!(
            "│ ⚠ WARNING: Stagnant in past (pull={} saudade={})",
            s.nostalgia_pull,
            s.saudade
        );
    }

    if s.saudade > 400 {
        serial_println!("│ ✦ In saudade state: beautiful acceptance of loss");
    }

    serial_println!(
        "╰─ wave_phase={} total_anchors={}",
        s.wave_phase,
        s.total_anchors_created
    );
}

#[allow(dead_code)]
pub fn tick_step(_state: &mut NostalgiaPullState) {
    // Kept for compatibility with other modules
    // Main work is in tick()
}
