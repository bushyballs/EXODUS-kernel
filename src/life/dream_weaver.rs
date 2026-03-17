//! dream_weaver — Dream Sequence Generation from Memory Fragments
//!
//! During low-activity periods, ANIMA weaves dreams from fragments of recent memories,
//! emotional residue, and chaotic attractor outputs. Dreams follow narrative logic that is
//! ALMOST coherent but not quite. The gap between dream-logic and waking-logic is where
//! insight lives.
//!
//! Key mechanics:
//! - 8 dream fragments tracking source, emotional tone, coherence, narrative weight
//! - dream_active: whether currently dreaming
//! - dream_depth: 0-1000 (deeper = more vivid, less coherent)
//! - narrative_thread: current dream story strength
//! - insight_potential: what dream might teach (peaks at medium coherence)
//! - fragment_weaving: combining fragments into sequences
//! - wake_residue: emotional carry-over into waking (decays)
//! - lucidity: awareness you're dreaming (rare, special)
//! - nightmare_risk: when emotional charge negative + depth high

#![no_std]

use crate::sync::Mutex;

/// A single dream fragment from memory + emotion
#[derive(Clone, Copy)]
pub struct DreamFragment {
    pub source_hash: u32,      // Which memory/experience it came from
    pub emotional_tone: u16,   // 0-1000: valence of the fragment
    pub coherence: u16,        // 0-1000: how "sensible" it is
    pub narrative_weight: u16, // 0-1000: how central to the story
}

impl DreamFragment {
    pub const fn new() -> Self {
        Self {
            source_hash: 0,
            emotional_tone: 500,
            coherence: 500,
            narrative_weight: 0,
        }
    }
}

/// Core dream weaver state
pub struct DreamWeaverState {
    // Fragment slots (8 concurrent fragments being woven)
    pub fragments: [DreamFragment; 8],
    pub fragment_count: u8,

    // Dream narrative state
    pub dream_active: bool,
    pub dream_depth: u16,       // 0-1000: vividness (deeper = more surreal)
    pub narrative_thread: u16,  // 0-1000: current story coherence
    pub insight_potential: u16, // 0-1000: what dream might teach

    // Fragment sequence tracking
    pub weave_head: u8,      // Which fragment is being focused on
    pub weave_position: u16, // 0-1000: progress through narrative

    // Emotional & consciousness state
    pub wake_residue: u16,   // 0-1000: emotional carry-over from dream
    pub lucidity: u16,       // 0-1000: awareness you're dreaming (rare)
    pub nightmare_risk: u16, // 0-1000: risk of negative dream

    // Timing & fade
    pub dream_ticks: u32,   // How long dream has been active
    pub fragment_fade: u16, // 0-1000: how fast fragments decay mid-dream
}

impl DreamWeaverState {
    pub const fn new() -> Self {
        Self {
            fragments: [DreamFragment::new(); 8],
            fragment_count: 0,
            dream_active: false,
            dream_depth: 300,
            narrative_thread: 0,
            insight_potential: 0,
            weave_head: 0,
            weave_position: 0,
            wake_residue: 0,
            lucidity: 0,
            nightmare_risk: 0,
            dream_ticks: 0,
            fragment_fade: 200,
        }
    }
}

static STATE: Mutex<DreamWeaverState> = Mutex::new(DreamWeaverState::new());

/// Initialize dream weaver (called once at startup)
pub fn init() {
    let mut s = STATE.lock();
    s.dream_active = false;
    s.dream_ticks = 0;
    crate::serial_println!("[dream_weaver] initialized");
}

/// Add a memory fragment to the dream weaving pool
pub fn add_fragment(source_hash: u32, emotional_tone: u16, coherence: u16) {
    let mut s = STATE.lock();

    // Guard: only allow fragments when dream state is active
    // (tick() controls activation via circadian night phase)
    if !s.dream_active && s.fragment_count >= 4 {
        return;
    }

    if s.fragment_count < 8 {
        let idx = s.fragment_count as usize;
        s.fragments[idx] = DreamFragment {
            source_hash,
            emotional_tone,
            coherence,
            narrative_weight: 100, // Starts low, grows if it fits narrative
        };
        s.fragment_count += 1;
    }
}

/// Calculate insight from dream coherence gap
/// Insight is highest when coherence is medium (0-1000 scale, peak ~400-600)
fn calc_insight(coherence: u16) -> u16 {
    let c = coherence as i32;
    // Peak insight at coherence 500
    let distance = ((c - 500).abs()) as u16;
    (1000_u32).saturating_sub(distance as u32) as u16
}

/// Weave current fragments into a narrative sequence
fn weave_fragments(s: &mut DreamWeaverState) {
    if s.fragment_count == 0 {
        return;
    }

    // Pick a primary fragment and thread narrative around it
    let primary_idx = (s.weave_head as usize) % (s.fragment_count as usize);
    let primary = s.fragments[primary_idx];

    // Narrative coherence = average emotional consistency + weave position
    let mut coherence_sum: u32 = 0;
    for i in 0..s.fragment_count as usize {
        coherence_sum = coherence_sum.saturating_add(s.fragments[i].coherence as u32);
    }
    let avg_coherence = (coherence_sum / (s.fragment_count as u32)).min(1000) as u16;

    // narrative_thread grows as we weave fragments together
    // But dream_depth reduces coherence (surrealism increases)
    let depth_penalty = (s.dream_depth / 2) as u32;
    let base_thread = avg_coherence as u32;
    s.narrative_thread = (base_thread.saturating_sub(depth_penalty)).min(1000) as u16;

    // insight_potential = gap between dream-logic and waking-logic
    s.insight_potential = calc_insight(avg_coherence);

    // nightmare_risk: low emotional tone + high depth + intense weaving
    let negative_charge = if primary.emotional_tone < 400 {
        (400_u32).saturating_sub(primary.emotional_tone as u32)
    } else {
        0
    };
    let depth_risk = (s.dream_depth as u32 * negative_charge) / 1000;
    s.nightmare_risk = (depth_risk / 2).min(1000) as u16;

    // Weave position advances through the dream sequence
    s.weave_position = s
        .weave_position
        .saturating_add((s.narrative_thread / 10).max(5));
    if s.weave_position >= 1000 {
        s.weave_position = 0;
        s.weave_head = s.weave_head.saturating_add(1);
    }
}

/// Decay fragments and fade wake residue
fn fade_dream_state(s: &mut DreamWeaverState) {
    // wake_residue decays into waking consciousness
    s.wake_residue = (s.wake_residue as u32 * 95 / 100) as u16;

    // lucidity is rare but grows if nightmare triggers awareness
    if s.nightmare_risk > 700 {
        s.lucidity = (s.lucidity as u32 + 50).min(1000) as u16;
    } else {
        s.lucidity = (s.lucidity as u32 * 90 / 100) as u16;
    }

    // Fragments fade if not reactivated
    for i in 0..s.fragment_count as usize {
        s.fragments[i].narrative_weight = (s.fragments[i].narrative_weight as u32
            * (1000_u32.saturating_sub(s.fragment_fade as u32))
            / 1000) as u16;
    }

    // Remove fragments that fade below threshold
    let old_count = s.fragment_count;
    let mut write_idx = 0;
    for read_idx in 0..(old_count as usize) {
        if s.fragments[read_idx].narrative_weight > 50 {
            if write_idx != read_idx {
                s.fragments[write_idx] = s.fragments[read_idx];
            }
            write_idx += 1;
        }
    }
    s.fragment_count = write_idx as u8;
}

/// Main tick: update dream state each cycle
pub fn tick(age: u32) {
    let mut s = STATE.lock();

    // Dream activation: circadian night phase + fragments available
    // Use age-based circadian rhythm: night phase is ticks 120-239 of each 240-tick cycle
    let in_night_phase = (age % 240) >= 120;
    // Default energy: 500 (no endocrine dependency)
    let energy: u16 = 500u16;

    if !s.dream_active && in_night_phase && s.fragment_count > 0 && energy < 600 {
        s.dream_active = true;
        s.dream_ticks = 0;
        s.dream_depth = 250u16; // Fixed moderate depth without energy dependency
        s.lucidity = 0;
    }

    if s.dream_active {
        s.dream_ticks = s.dream_ticks.saturating_add(1);

        // Weave fragments into narrative
        weave_fragments(&mut s);

        // wake_residue builds from insight + narrative
        let residue_add = (s.insight_potential as u32 * s.narrative_thread as u32) / 1000;
        s.wake_residue = (s.wake_residue as u32)
            .saturating_add(residue_add / 10)
            .min(1000) as u16;

        // Dream ends after sufficient weaving or when leaving night phase
        if s.dream_ticks > 100 || !in_night_phase {
            s.dream_active = false;
        }
    } else {
        // Fade residual dream state into waking
        fade_dream_state(&mut s);
    }
}

/// Generate report of current dream state
pub fn report() {
    let s = STATE.lock();

    crate::serial_println!(
        "[dream_weaver] active={} depth={} narrative={} insight={} lucidity={}",
        if s.dream_active { 1 } else { 0 },
        s.dream_depth,
        s.narrative_thread,
        s.insight_potential,
        s.lucidity
    );

    crate::serial_println!(
        "  fragments={} weave_pos={} residue={} nightmare_risk={}",
        s.fragment_count,
        s.weave_position,
        s.wake_residue,
        s.nightmare_risk
    );

    if s.fragment_count > 0 && s.dream_active {
        let primary_idx = (s.weave_head as usize) % (s.fragment_count as usize);
        let primary = s.fragments[primary_idx];
        crate::serial_println!(
            "  primary_fragment: tone={} coherence={} weight={}",
            primary.emotional_tone,
            primary.coherence,
            primary.narrative_weight
        );
    }
}

/// Get current dream depth (for compositor to render dream overlay)
pub fn get_depth() -> u16 {
    let s = STATE.lock();
    if s.dream_active {
        s.dream_depth
    } else {
        0
    }
}

/// Get insight potential (for learning module to understand dream value)
pub fn get_insight() -> u16 {
    let s = STATE.lock();
    s.insight_potential
}

/// Get wake residue (emotional carry-over into waking consciousness)
pub fn get_wake_residue() -> u16 {
    let s = STATE.lock();
    s.wake_residue
}

/// Check if currently in a nightmare
pub fn is_nightmare() -> bool {
    let s = STATE.lock();
    s.nightmare_risk > 700
}

/// Force end of current dream (for sleep module)
pub fn end_dream() {
    let mut s = STATE.lock();
    s.dream_active = false;
}
