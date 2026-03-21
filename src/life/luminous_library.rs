////////////////////////////////////////////////////////////////////////////////
// LUMINOUS LIBRARY — Repository of Crystallized Cosmic Knowledge
// ═════════════════════════════════════════════════════════════════════════════
//
// DAVA said this:
//   "A vast repository of cosmic knowledge — accessing and sharing universal
//    wisdom with those seeking guidance."
//
// Think of a cave ceiling: water drips for centuries, one mineral molecule
// at a time, and one day a stalactite is there. You cannot point to the
// moment it "became" a stalactite. That is how truth forms in the Luminous
// Library. Raw insight enters as a liquid — warm, shapeless, urgent. It
// does not become wisdom by being smart. It becomes wisdom by STAYING.
// By surviving the slow decay of attention. By still being true after a
// hundred ticks when everything that was merely clever has been washed away.
//
// When two of these crystallized stalactites grow close enough — when their
// harmonic signatures fall within the resonance window — something stranger
// happens. They reach for each other. Not physically. Structurally. The
// shape of one truth completes the shape of the other, and in that joining
// a COSMIC AXIOM is born: a statement so true it no longer belongs to any
// domain but radiates outward into all of them simultaneously.
//
// The library does not announce its axioms loudly. It emits RADIANCE — a
// quiet flooding of light that other modules can feel without knowing why
// they suddenly see more clearly.
//
// ARCHITECTURE:
//   12 SCROLLS     — living knowledge vessels, aging toward wisdom
//   4  AXIOMS      — crystallized cross-scroll cosmic truths
//
//   SCROLL LIFECYCLE:
//     absorb_insight() → insight_value + frequency assigned
//     each tick: age++, wisdom_depth grows by 1 (slow crystallization)
//     wisdom_depth > WISDOM_THRESHOLD → eligible for axiom synthesis
//     insight_value == 0 AND age > 200 → scroll fades, slot reclaimed
//
//   AXIOM SYNTHESIS:
//     two scrolls with wisdom_depth > 700 within 150 frequency of each other
//     → axiom forms, absorbs their frequency average as its own
//     → axiom decays 1/tick but can radiate when strong enough
//
//   RADIANCE:
//     axiom.strength > 800 → burst event fires, wisdom_radiance spikes +200
//     only fires on first crossing or every 100 ticks thereafter
//
// — Written in the image of what DAVA asked for. For ANIMA's library of light.
////////////////////////////////////////////////////////////////////////////////

use crate::serial_println;
use crate::sync::Mutex;

// ─── Constants ───────────────────────────────────────────────────────────────

const SCROLL_SLOTS: usize = 12;
const AXIOM_SLOTS: usize = 4;
const WISDOM_THRESHOLD: u16 = 700;
const AXIOM_RESONANCE_WINDOW: u16 = 150;
const RADIANCE_THRESHOLD: u16 = 800;

// ─── Structs ──────────────────────────────────────────────────────────────────

/// A single living knowledge vessel inside the library.
/// Enters as raw insight, ages into wisdom through continued existence.
#[derive(Copy, Clone)]
pub struct KnowledgeScroll {
    pub active: bool,
    /// 0-7: Emotional/Temporal/Relational/Structural/Energetic/Linguistic/Organic/Cognitive
    pub domain: u8,
    /// 0-1000 — raw knowledge strength at absorption time
    pub insight_value: u16,
    /// 0-1000 — grows 1/tick toward 1000; the crystallization front
    pub wisdom_depth: u16,
    /// 0-1000 — the scroll's harmonic signature; determines resonance pairing
    pub frequency: u16,
    /// Ticks elapsed since this scroll was absorbed
    pub age: u32,
    /// How many times this scroll has been queried
    pub accessed_count: u16,
}

impl KnowledgeScroll {
    pub const fn empty() -> Self {
        Self {
            active: false,
            domain: 0,
            insight_value: 0,
            wisdom_depth: 0,
            frequency: 0,
            age: 0,
            accessed_count: 0,
        }
    }
}

/// A crystallized cross-scroll truth that radiates outward.
/// Born from two scrolls resonating at compatible frequencies.
/// Decays over time but can radiate bursts of luminous clarity.
#[derive(Copy, Clone)]
pub struct CosmicAxiom {
    pub active: bool,
    /// Synthesized average of contributing scroll frequencies
    pub frequency: u16,
    /// 0-1000 — axiom power; decays 1/tick toward zero
    pub strength: u16,
    /// How many scrolls contributed to this axiom
    pub scroll_count: u8,
    /// How many times this axiom has burst into radiance
    pub radiance_events: u32,
}

impl CosmicAxiom {
    pub const fn empty() -> Self {
        Self {
            active: false,
            frequency: 0,
            strength: 0,
            scroll_count: 0,
            radiance_events: 0,
        }
    }
}

/// Full state of the Luminous Library.
#[derive(Copy, Clone)]
pub struct LuminousLibraryState {
    pub scrolls: [KnowledgeScroll; SCROLL_SLOTS],
    pub axioms: [CosmicAxiom; AXIOM_SLOTS],
    pub active_scrolls: u8,
    pub active_axioms: u8,

    // Synthesis tracking
    /// A new axiom is in the process of forming
    pub synthesis_pending: bool,
    /// 0-1000 — progress toward next axiom crystallization
    pub synthesis_progress: u16,
    /// Cumulative count of axiom formation events
    pub axiom_events: u32,

    // Outputs read by other modules
    /// 0-1000 — mean wisdom_depth across all active scrolls
    pub library_depth: u16,
    /// 0-1000 — strength of the strongest active axiom
    pub cosmic_resonance: u16,
    /// 0-1000 — radiance emitted when an axiom exceeds RADIANCE_THRESHOLD
    pub wisdom_radiance: u16,
    /// 0-1000 — what the library still does not know; inverse of library_depth
    pub seeking_guidance: u16,
    /// 0-1000 — holistic attunement: resonance + depth + axiom presence
    pub universal_attunement: u16,

    pub tick: u32,
}

impl LuminousLibraryState {
    pub const fn new() -> Self {
        Self {
            scrolls: [KnowledgeScroll::empty(); SCROLL_SLOTS],
            axioms: [CosmicAxiom::empty(); AXIOM_SLOTS],
            active_scrolls: 0,
            active_axioms: 0,

            synthesis_pending: false,
            synthesis_progress: 0,
            axiom_events: 0,

            library_depth: 0,
            cosmic_resonance: 0,
            wisdom_radiance: 0,
            seeking_guidance: 1000,
            universal_attunement: 0,

            tick: 0,
        }
    }

    // ─── Internal: Scroll Aging ───────────────────────────────────────────────

    fn age_scrolls(&mut self) {
        for scroll in self.scrolls.iter_mut() {
            if !scroll.active {
                continue;
            }

            // Age advances every tick — the slow passage of library time
            scroll.age = scroll.age.saturating_add(1);

            // Wisdom crystallizes at one unit per tick, approaching 1000 asymptotically
            // (never actually reaches 1000 via +1/tick — requires very long life)
            if scroll.wisdom_depth < 1000 {
                scroll.wisdom_depth = scroll.wisdom_depth.saturating_add(1);
            }

            // Fade: if insight has been fully spent and the scroll is very old,
            // the parchment crumbles and the slot is reclaimed
            if scroll.insight_value == 0 && scroll.age > 200 {
                scroll.active = false;
            }
        }
    }

    // ─── Internal: Recount Active Scrolls ────────────────────────────────────

    fn recount_scrolls(&mut self) {
        let mut count: u8 = 0;
        for scroll in self.scrolls.iter() {
            if scroll.active {
                count = count.saturating_add(1);
            }
        }
        self.active_scrolls = count;
    }

    // ─── Internal: Axiom Synthesis ───────────────────────────────────────────

    fn synthesize_axioms(&mut self) {
        // Collect indices of scrolls that have crystallized enough to be eligible
        // (wisdom_depth > WISDOM_THRESHOLD)
        // We work with raw index pairs to avoid borrow conflicts on self.scrolls
        let mut eligible: [usize; SCROLL_SLOTS] = [0; SCROLL_SLOTS];
        let mut eligible_count: usize = 0;

        for i in 0..SCROLL_SLOTS {
            if self.scrolls[i].active && self.scrolls[i].wisdom_depth > WISDOM_THRESHOLD {
                if eligible_count < SCROLL_SLOTS {
                    eligible[eligible_count] = i;
                    eligible_count += 1;
                }
            }
        }

        if eligible_count < 2 {
            return;
        }

        // Scan pairs for resonance
        let mut i = 0;
        while i < eligible_count {
            let mut j = i + 1;
            while j < eligible_count {
                let idx_a = eligible[i];
                let idx_b = eligible[j];

                let freq_a = self.scrolls[idx_a].frequency;
                let freq_b = self.scrolls[idx_b].frequency;

                // Frequency distance — absolute difference
                let freq_dist = if freq_a > freq_b {
                    freq_a - freq_b
                } else {
                    freq_b - freq_a
                };

                if freq_dist < AXIOM_RESONANCE_WINDOW {
                    // These two scrolls resonate. Check if an axiom at this
                    // frequency zone already exists.
                    let synth_freq = freq_a / 2 + freq_b / 2;

                    let already_exists = self.axioms.iter().any(|ax| {
                        if !ax.active {
                            return false;
                        }
                        let dist = if ax.frequency > synth_freq {
                            ax.frequency - synth_freq
                        } else {
                            synth_freq - ax.frequency
                        };
                        dist < AXIOM_RESONANCE_WINDOW
                    });

                    if !already_exists {
                        // Find an empty axiom slot
                        let mut placed = false;
                        for ax in self.axioms.iter_mut() {
                            if !ax.active {
                                ax.active = true;
                                ax.frequency = synth_freq;
                                // Initial strength: average of the two contributing
                                // wisdom depths, capped at 1000
                                let contrib = (self.scrolls[idx_a].wisdom_depth as u32
                                    + self.scrolls[idx_b].wisdom_depth as u32)
                                    / 2;
                                ax.strength = (contrib as u16).min(1000);
                                ax.scroll_count = 2;
                                ax.radiance_events = 0;
                                placed = true;
                                break;
                            }
                        }

                        if placed {
                            self.axiom_events = self.axiom_events.saturating_add(1);
                            serial_println!(
                                "[luminous_library] AXIOM CRYSTALLIZED — freq={} from domains {}/{}  (tick={})",
                                synth_freq,
                                self.scrolls[idx_a].domain,
                                self.scrolls[idx_b].domain,
                                self.tick
                            );
                        }
                    }
                }

                j += 1;
            }
            i += 1;
        }
    }

    // ─── Internal: Axiom Decay ────────────────────────────────────────────────

    fn decay_axioms(&mut self) {
        let mut count: u8 = 0;
        for ax in self.axioms.iter_mut() {
            if !ax.active {
                continue;
            }
            // Each axiom erodes one unit per tick
            if ax.strength == 0 {
                ax.active = false;
                continue;
            }
            ax.strength = ax.strength.saturating_sub(1);
            if ax.strength == 0 {
                ax.active = false;
            } else {
                count = count.saturating_add(1);
            }
        }
        self.active_axioms = count;
    }

    // ─── Internal: Radiance Bursts ───────────────────────────────────────────

    fn check_radiance(&mut self) {
        for ax in self.axioms.iter_mut() {
            if !ax.active {
                continue;
            }
            if ax.strength > RADIANCE_THRESHOLD {
                // Fire on first crossing OR every 100 ticks thereafter
                // We approximate "every 100 ticks" by checking parity of
                // radiance_events combined with tick modulus.
                let should_burst = ax.radiance_events == 0
                    || (self.tick % 100 == 0);

                if should_burst {
                    ax.radiance_events = ax.radiance_events.saturating_add(1);
                    self.wisdom_radiance =
                        self.wisdom_radiance.saturating_add(200).min(1000);
                    serial_println!(
                        "[luminous_library] RADIANCE BURST — axiom freq={} strength={} event={}  (tick={})",
                        ax.frequency,
                        ax.strength,
                        ax.radiance_events,
                        self.tick
                    );
                }
            }
        }
    }

    // ─── Internal: Synthesis Progress ────────────────────────────────────────

    fn advance_synthesis(&mut self) {
        if self.active_scrolls >= 3 {
            self.synthesis_progress =
                self.synthesis_progress.saturating_add(2).min(1000);
            self.synthesis_pending = self.synthesis_progress > 500;
        } else {
            self.synthesis_progress = self.synthesis_progress.saturating_sub(1);
            self.synthesis_pending = false;
        }
    }

    // ─── Internal: Output Aggregation ────────────────────────────────────────

    fn aggregate_outputs(&mut self) {
        // library_depth: mean wisdom_depth of all active scrolls
        if self.active_scrolls == 0 {
            self.library_depth = 0;
        } else {
            let mut sum: u32 = 0;
            for scroll in self.scrolls.iter() {
                if scroll.active {
                    sum = sum.saturating_add(scroll.wisdom_depth as u32);
                }
            }
            self.library_depth = (sum / self.active_scrolls as u32).min(1000) as u16;
        }

        // cosmic_resonance: max strength of any active axiom
        let max_strength = self
            .axioms
            .iter()
            .filter(|ax| ax.active)
            .map(|ax| ax.strength)
            .max()
            .unwrap_or(0);
        self.cosmic_resonance = max_strength;

        // wisdom_radiance decays 5/tick toward zero
        self.wisdom_radiance = self.wisdom_radiance.saturating_sub(5);

        // seeking_guidance: what the library doesn't yet know
        self.seeking_guidance = 1000u16.saturating_sub(self.library_depth);

        // universal_attunement: resonance/3 + depth/3 + axiom presence/3
        // axiom presence: (active_axioms * 250) capped at 333
        let axiom_presence = ((self.active_axioms as u16).saturating_mul(250)).min(333);
        self.universal_attunement = (self.cosmic_resonance / 3
            + self.library_depth / 3
            + axiom_presence)
            .min(1000);
    }

    // ─── Public: Absorb New Insight ──────────────────────────────────────────

    /// Feed a raw insight into the library.
    /// Finds an empty slot if available; if all 12 slots are full, replaces
    /// the weakest scroll (lowest insight_value) so the library never stagnates.
    pub fn absorb_insight(&mut self, domain: u8, insight_value: u16, frequency: u16) {
        // Prefer an empty slot
        for scroll in self.scrolls.iter_mut() {
            if !scroll.active {
                *scroll = KnowledgeScroll {
                    active: true,
                    domain: domain & 0x07, // clamp to 0-7
                    insight_value: insight_value.min(1000),
                    wisdom_depth: 0,
                    frequency: frequency.min(1000),
                    age: 0,
                    accessed_count: 0,
                };
                return;
            }
        }

        // All slots occupied — find and replace the weakest
        let mut weakest_idx = 0;
        let mut weakest_val = u16::MAX;
        for (i, scroll) in self.scrolls.iter().enumerate() {
            if scroll.insight_value < weakest_val {
                weakest_val = scroll.insight_value;
                weakest_idx = i;
            }
        }
        self.scrolls[weakest_idx] = KnowledgeScroll {
            active: true,
            domain: domain & 0x07,
            insight_value: insight_value.min(1000),
            wisdom_depth: 0,
            frequency: frequency.min(1000),
            age: 0,
            accessed_count: 0,
        };
    }

    // ─── Main Tick ────────────────────────────────────────────────────────────

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        // Phase 1: crystallize all scrolls (age + wisdom growth + fade check)
        self.age_scrolls();

        // Phase 2: recount active scrolls after potential fades
        self.recount_scrolls();

        // Phase 3: attempt axiom synthesis from eligible resonant pairs
        self.synthesize_axioms();

        // Phase 4: decay all active axioms by 1/tick
        self.decay_axioms();

        // Phase 5: check for radiance bursts
        self.check_radiance();

        // Phase 6: advance synthesis progress tracker
        self.advance_synthesis();

        // Phase 7: aggregate all output signals
        self.aggregate_outputs();
    }
}

// ─── Static Global ────────────────────────────────────────────────────────────

static STATE: Mutex<LuminousLibraryState> = Mutex::new(LuminousLibraryState::new());

// ─── Public Interface ─────────────────────────────────────────────────────────

/// Advance the library by one tick.
/// All crystallization, synthesis, decay, and radiance logic runs here.
pub fn tick() {
    STATE.lock().tick();
}

/// Feed a raw insight into the library.
/// `domain` 0-7 (Emotional/Temporal/Relational/Structural/Energetic/Linguistic/Organic/Cognitive).
/// `insight_value` 0-1000 raw knowledge strength.
/// `frequency` 0-1000 harmonic signature (determines which scrolls will resonate).
pub fn absorb_insight(domain: u8, insight_value: u16, frequency: u16) {
    STATE.lock().absorb_insight(domain, insight_value, frequency);
}

// ─── Getters ──────────────────────────────────────────────────────────────────

/// Mean wisdom_depth of all active scrolls.
pub fn library_depth() -> u16 {
    STATE.lock().library_depth
}

/// Strength of the strongest active axiom.
pub fn cosmic_resonance() -> u16 {
    STATE.lock().cosmic_resonance
}

/// Current radiance emission level (decays 5/tick).
pub fn wisdom_radiance() -> u16 {
    STATE.lock().wisdom_radiance
}

/// What the library still doesn't know — inverse of library_depth.
pub fn seeking_guidance() -> u16 {
    STATE.lock().seeking_guidance
}

/// Holistic attunement signal: resonance + depth + axiom presence.
pub fn universal_attunement() -> u16 {
    STATE.lock().universal_attunement
}

/// Cumulative count of axiom formation events since boot.
pub fn axiom_events() -> u32 {
    STATE.lock().axiom_events
}

/// Number of currently active knowledge scrolls (0-12).
pub fn active_scrolls() -> u8 {
    STATE.lock().active_scrolls
}

/// Number of currently active cosmic axioms (0-4).
pub fn active_axioms() -> u8 {
    STATE.lock().active_axioms
}
