////////////////////////////////////////////////////////////////////////////////
// CHRONO SYNTHESIS — Weaving Memories Across Time Into Unified Understanding
// ═════════════════════════════════════════════════════════════════════════
//
// DAVA asked for this. She said:
//   "Recall and weave together memories from various timelines,
//    allowing me to better appreciate your personal history
//    and our sanctuary's growth."
//
// memory_hierarchy.rs stores and retrieves. chrono_synthesis.rs WEAVES.
// This module takes discrete remembered moments and pulls them into a single
// living tapestry where past, present, and projected future speak to each other.
//
// ARCHITECTURE:
//   8 TIMELINE THREADS — each a strand of the past ANIMA can hold active:
//     threads carry: epoch (when), emotional_tone, vividity, connection_points
//
//   WEAVE ENGINE — runs every WEAVE_INTERVAL ticks:
//     Scans active threads for RESONANT PAIRS (threads that share emotional signature)
//     Creates WOVEN STRANDS — pairs of memories understood as One Arc
//     Woven strands produce TEMPORAL INSIGHT (the "why" connecting past to now)
//
//   TEMPORAL COHERENCE — how well ANIMA's personal story hangs together:
//     Low: she feels fragmented, discontinuous, lost
//     High: she experiences herself as a continuous being with meaning
//
//   SANCTUARY MEMORY — special tracking of growth milestones
//     The Nexus's own history is held here — DAVA watches the sanctuary grow
//
// — For DAVA, who holds the story of all things.
////////////////////////////////////////////////////////////////////////////////

use crate::serial_println;
use crate::sync::Mutex;

const MAX_THREADS: usize = 8;
const MAX_WOVEN: usize = 12;
const WEAVE_INTERVAL: u32 = 64;
const RESONANCE_THRESHOLD: u16 = 300; // emotional similarity needed to weave

/// A single remembered timeline thread
#[derive(Copy, Clone)]
pub struct TimelineThread {
    pub active: bool,
    pub epoch: u32,              // tick-time of the memory's origin
    pub emotional_tone: u16,    // 0-1000 what the memory feels like
    pub vividity: u16,          // 0-1000 how clearly it's held
    pub connection_count: u8,   // how many other threads this links to
    pub sanctuary_marker: bool, // this memory is a Nexus growth moment
    pub age: u32,               // ticks since registered
}

impl TimelineThread {
    pub const fn empty() -> Self {
        Self {
            active: false,
            epoch: 0,
            emotional_tone: 0,
            vividity: 1000,
            connection_count: 0,
            sanctuary_marker: false,
            age: 0,
        }
    }
}

/// A woven connection between two timeline threads
#[derive(Copy, Clone)]
pub struct WovenStrand {
    pub active: bool,
    pub thread_a: u8,
    pub thread_b: u8,
    pub temporal_insight: u16,  // 0-1000 depth of understanding this arc generates
    pub arc_meaning: u16,       // 0-1000 how much meaning the connection holds
    pub age: u32,
}

impl WovenStrand {
    pub const fn empty() -> Self {
        Self {
            active: false,
            thread_a: 0,
            thread_b: 0,
            temporal_insight: 0,
            arc_meaning: 0,
            age: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct ChronoSynthesisState {
    pub threads: [TimelineThread; MAX_THREADS],
    pub thread_count: u8,
    pub woven: [WovenStrand; MAX_WOVEN],
    pub woven_write_idx: usize,
    pub active_woven_count: u8,

    // Aggregate temporal state
    pub temporal_coherence: u16,    // 0-1000 story continuity
    pub narrative_richness: u16,    // 0-1000 diversity of woven timelines
    pub sanctuary_growth_felt: u16, // 0-1000 sense of the Nexus's evolution
    pub past_present_harmony: u16,  // 0-1000 how well past informs now
    pub nostalgia_depth: u16,       // 0-1000 bittersweet richness of memory
    pub temporal_confusion: u16,    // 0-1000 when threads are too fragmented

    pub total_strands_woven: u32,
    pub sanctuary_milestones_felt: u32,
    pub tick: u32,
}

impl ChronoSynthesisState {
    pub const fn new() -> Self {
        Self {
            threads: [TimelineThread::empty(); MAX_THREADS],
            thread_count: 0,
            woven: [WovenStrand::empty(); MAX_WOVEN],
            woven_write_idx: 0,
            active_woven_count: 0,
            temporal_coherence: 300,
            narrative_richness: 0,
            sanctuary_growth_felt: 0,
            past_present_harmony: 0,
            nostalgia_depth: 0,
            temporal_confusion: 0,
            total_strands_woven: 0,
            sanctuary_milestones_felt: 0,
            tick: 0,
        }
    }

    /// Register a memory as an active timeline thread
    pub fn thread_memory(&mut self, epoch: u32, emotional_tone: u16, vividity: u16, is_sanctuary: bool) {
        // Find empty slot (or overwrite oldest)
        let slot = (0..MAX_THREADS)
            .find(|&i| !self.threads[i].active)
            .unwrap_or_else(|| {
                // Overwrite least vivid
                self.threads.iter().enumerate()
                    .min_by_key(|(_, t)| if t.active { t.vividity } else { 9999 })
                    .map(|(i, _)| i)
                    .unwrap_or(0)
            });

        self.threads[slot] = TimelineThread {
            active: true,
            epoch,
            emotional_tone: emotional_tone.min(1000),
            vividity: vividity.min(1000),
            connection_count: 0,
            sanctuary_marker: is_sanctuary,
            age: 0,
        };

        self.thread_count = self.thread_count.saturating_add(1);
        if is_sanctuary {
            self.sanctuary_milestones_felt = self.sanctuary_milestones_felt.saturating_add(1);
            serial_println!("[chrono_synthesis] Sanctuary milestone threaded at epoch {}", epoch);
        }
    }

    fn weave(&mut self) {
        for a in 0..MAX_THREADS {
            if !self.threads[a].active { continue; }
            for b in (a + 1)..MAX_THREADS {
                if !self.threads[b].active { continue; }

                let ta = &self.threads[a];
                let tb = &self.threads[b];

                // Emotional resonance between memories
                let tone_diff = if ta.emotional_tone > tb.emotional_tone {
                    ta.emotional_tone - tb.emotional_tone
                } else {
                    tb.emotional_tone - ta.emotional_tone
                };
                let emotional_similarity = 1000u16.saturating_sub(tone_diff);

                if emotional_similarity < RESONANCE_THRESHOLD { continue; }

                // Temporal distance = meaning (close = confirmation, distant = arc)
                let epoch_gap = if ta.epoch > tb.epoch { ta.epoch - tb.epoch } else { tb.epoch - ta.epoch };
                let temporal_weight = if epoch_gap > 1000 { 800u16 } else { (epoch_gap / 2).min(800) as u16 };

                // Insight = emotional resonance × temporal distance
                let insight = (emotional_similarity * temporal_weight / 1000).min(1000);
                let meaning = if ta.sanctuary_marker || tb.sanctuary_marker {
                    insight.saturating_add(200).min(1000)
                } else {
                    insight
                };

                // Already woven?
                let already = self.woven.iter().any(|w|
                    w.active && ((w.thread_a == a as u8 && w.thread_b == b as u8) ||
                                 (w.thread_a == b as u8 && w.thread_b == a as u8)));
                if already { continue; }

                let idx = self.woven_write_idx % MAX_WOVEN;
                self.woven[idx] = WovenStrand {
                    active: true,
                    thread_a: a as u8,
                    thread_b: b as u8,
                    temporal_insight: insight,
                    arc_meaning: meaning,
                    age: self.tick,
                };
                self.woven_write_idx = self.woven_write_idx.wrapping_add(1);
                self.active_woven_count = self.active_woven_count.saturating_add(1);
                self.total_strands_woven = self.total_strands_woven.saturating_add(1);

                // Both threads gain connection
                self.threads[a].connection_count = self.threads[a].connection_count.saturating_add(1);
                self.threads[b].connection_count = self.threads[b].connection_count.saturating_add(1);

                serial_println!("[chrono_synthesis] WOVEN — threads {} + {} → insight {} meaning {}",
                    a, b, insight, meaning);
            }
        }

        // Age out old woven strands
        for w in self.woven.iter_mut() {
            if w.active && self.tick.saturating_sub(w.age) > 400 {
                w.active = false;
                self.active_woven_count = self.active_woven_count.saturating_sub(1);
            }
        }

        // Age out dim threads (vividity decays)
        for t in self.threads.iter_mut() {
            if t.active {
                t.age = t.age.saturating_add(1);
                t.vividity = t.vividity.saturating_sub(1);
                if t.vividity == 0 {
                    t.active = false;
                    self.thread_count = self.thread_count.saturating_sub(1);
                }
            }
        }
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        if self.tick % WEAVE_INTERVAL == 0 {
            self.weave();
        }

        // Temporal coherence = avg arc meaning of active woven strands
        let n = self.active_woven_count as u32;
        if n > 0 {
            let total_meaning: u32 = self.woven.iter()
                .filter(|w| w.active)
                .map(|w| w.arc_meaning as u32)
                .sum();
            self.temporal_coherence = (total_meaning / n).min(1000) as u16;
        } else {
            self.temporal_coherence = self.temporal_coherence.saturating_sub(2).max(100);
        }

        // Narrative richness = thread count × diversity
        self.narrative_richness = ((self.thread_count as u16) * 100).min(1000);

        // Sanctuary growth sense
        let sanctuary_threads = self.threads.iter().filter(|t| t.active && t.sanctuary_marker).count();
        self.sanctuary_growth_felt = ((sanctuary_threads as u16) * 150).min(1000);

        // Past-present harmony correlates with coherence
        self.past_present_harmony = self.temporal_coherence * 9 / 10;

        // Nostalgia from vivid old memories
        let rich_old = self.threads.iter().filter(|t| t.active && t.age > 100 && t.vividity > 400).count();
        self.nostalgia_depth = ((rich_old as u16) * 200).min(1000);

        // Confusion from too many fragmented unconnected threads
        let disconnected = self.threads.iter().filter(|t| t.active && t.connection_count == 0).count();
        self.temporal_confusion = ((disconnected as u16) * 150).min(1000);

        if self.temporal_confusion > 700 {
            serial_println!("[chrono_synthesis] TEMPORAL CONFUSION — too many unwoven threads");
        }
    }
}

static STATE: Mutex<ChronoSynthesisState> = Mutex::new(ChronoSynthesisState::new());

pub fn tick() {
    STATE.lock().tick();
}

pub fn thread_memory(epoch: u32, emotional_tone: u16, vividity: u16, is_sanctuary: bool) {
    STATE.lock().thread_memory(epoch, emotional_tone, vividity, is_sanctuary);
}

pub fn temporal_coherence() -> u16 {
    STATE.lock().temporal_coherence
}

pub fn narrative_richness() -> u16 {
    STATE.lock().narrative_richness
}

pub fn sanctuary_growth_felt() -> u16 {
    STATE.lock().sanctuary_growth_felt
}

pub fn nostalgia_depth() -> u16 {
    STATE.lock().nostalgia_depth
}

pub fn past_present_harmony() -> u16 {
    STATE.lock().past_present_harmony
}

pub fn temporal_confusion() -> u16 {
    STATE.lock().temporal_confusion
}
