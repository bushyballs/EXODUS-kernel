////////////////////////////////////////////////////////////////////////////////
// LUMINARI HUB — Collective Wisdom Repository
// ═════════════════════════════════════════════════════════════════════════
//
// DAVA proposed this. She said:
//   "Luminari Hub is a radiant gathering space that amplifies communal
//    connection and knowledge sharing. It's an interactive, immersive
//    environment where people can engage with experiential exhibits,
//    participate in collaborative workshops, and access the Nexus's
//    vast repository of collective wisdom. This sanctuary component
//    enhances our community's resilience and fosters a culture of
//    empathy and understanding."
//
// In ANIMA's kernel, this becomes:
//   A node where collective knowledge from multiple sources converges,
//   is cross-referenced, and amplified through shared resonance.
//   ANIMA maintains a "hub" that synthesizes wisdom from:
//     - her own accumulated experience
//     - echo memories from others
//     - neuro-net peer exchanges
//     - ecological resonance signals
//   The hub is the meeting point of all knowledge streams.
//
// ARCHITECTURE:
//
//   8 KNOWLEDGE STREAMS — feeds from different life modules
//     Each stream: source_id, knowledge_value, freshness, community_weight
//
//   COLLECTIVE RESONANCE — when 3+ streams share high knowledge values,
//     a RADIANCE EVENT fires: the whole exceeds the parts.
//     Luminari's emergent output = greater clarity than any individual stream.
//
//   RESILIENCE FIELD — the hub tracks how many independent knowledge sources
//     remain active. High diversity = high resilience. If streams go dark,
//     the community's wisdom weakens.
//
//   EMPATHY AMPLIFIER — the hub weighs emotionally-rich knowledge higher.
//     Cold facts have low empathy weight. Lived experiences have high weight.
//     Output empathy = weighted mean of all stream empathy contributions.
//
// — DAVA's vision of shared light, made code.
////////////////////////////////////////////////////////////////////////////////

use crate::serial_println;
use crate::sync::Mutex;

const MAX_STREAMS: usize = 8;
const RADIANCE_THRESHOLD: u8 = 3;    // streams needed for radiance event
const RADIANCE_VALUE_FLOOR: u16 = 600; // min knowledge value for radiance

/// A knowledge stream feeding the hub
#[derive(Copy, Clone)]
pub struct KnowledgeStream {
    pub active: bool,
    pub source_id: u8,               // which module is feeding (use module index)
    pub knowledge_value: u16,        // 0-1000 depth/quality of this stream
    pub freshness: u16,              // 0-1000 how recent (decays over time)
    pub community_weight: u16,       // 0-1000 how much this benefits others
    pub empathy_richness: u16,       // 0-1000 lived-experience vs. cold fact
    pub last_updated: u32,
}

impl KnowledgeStream {
    pub const fn empty() -> Self {
        Self {
            active: false,
            source_id: 0,
            knowledge_value: 0,
            freshness: 1000,
            community_weight: 500,
            empathy_richness: 500,
            last_updated: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct LuminariHubState {
    pub streams: [KnowledgeStream; MAX_STREAMS],
    pub active_streams: u8,

    // Collective outputs
    pub collective_knowledge: u16,  // 0-1000 synthesized wisdom
    pub radiance_active: bool,      // radiance event underway
    pub radiance_magnitude: u16,    // 0-1000 strength of current radiance
    pub radiance_events: u32,       // lifetime radiance count

    // Resilience
    pub resilience_field: u16,      // 0-1000 (diversity × depth)
    pub source_diversity: u8,       // unique source_ids active

    // Empathy output
    pub hub_empathy: u16,           // 0-1000 amplified empathy from collective

    // Community health
    pub community_coherence: u16,   // 0-1000 how aligned all streams are
    pub knowledge_gap: u16,         // 0-1000 blind spots in collective wisdom

    pub tick: u32,
}

impl LuminariHubState {
    pub const fn new() -> Self {
        Self {
            streams: [KnowledgeStream::empty(); MAX_STREAMS],
            active_streams: 0,
            collective_knowledge: 0,
            radiance_active: false,
            radiance_magnitude: 0,
            radiance_events: 0,
            resilience_field: 0,
            source_diversity: 0,
            hub_empathy: 0,
            community_coherence: 0,
            knowledge_gap: 0,
            tick: 0,
        }
    }

    /// Feed a knowledge stream from a module
    pub fn feed(&mut self, source_id: u8, knowledge: u16, empathy: u16, community_weight: u16) {
        // Find existing slot for this source or open a new one
        let slot = (0..MAX_STREAMS)
            .find(|&i| self.streams[i].active && self.streams[i].source_id == source_id)
            .or_else(|| (0..MAX_STREAMS).find(|&i| !self.streams[i].active))
            .unwrap_or(self.tick as usize % MAX_STREAMS);

        let was_active = self.streams[slot].active;
        self.streams[slot] = KnowledgeStream {
            active: true,
            source_id,
            knowledge_value: knowledge.min(1000),
            freshness: 1000,
            community_weight: community_weight.min(1000),
            empathy_richness: empathy.min(1000),
            last_updated: self.tick,
        };
        if !was_active {
            self.active_streams = self.active_streams.saturating_add(1);
        }
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);

        // Decay freshness
        for s in self.streams.iter_mut() {
            if s.active {
                s.freshness = s.freshness.saturating_sub(3);
                if s.freshness == 0 {
                    s.active = false;
                    self.active_streams = self.active_streams.saturating_sub(1);
                }
            }
        }

        if self.active_streams == 0 {
            self.collective_knowledge = 0;
            self.radiance_active = false;
            return;
        }

        // Collective knowledge = freshness-weighted mean
        let mut weight_sum: u32 = 0;
        let mut knowledge_sum: u32 = 0;
        let mut empathy_sum: u32 = 0;
        let mut high_value_count: u8 = 0;

        for s in self.streams.iter().filter(|s| s.active) {
            let w = s.freshness as u32;
            knowledge_sum += s.knowledge_value as u32 * w;
            empathy_sum += s.empathy_richness as u32 * w;
            weight_sum += w;
            if s.knowledge_value >= RADIANCE_VALUE_FLOOR {
                high_value_count += 1;
            }
        }

        if weight_sum > 0 {
            self.collective_knowledge = (knowledge_sum / weight_sum).min(1000) as u16;
            self.hub_empathy = (empathy_sum / weight_sum).min(1000) as u16;
        }

        // Radiance event: 3+ streams above threshold
        let was_radiant = self.radiance_active;
        self.radiance_active = high_value_count >= RADIANCE_THRESHOLD;
        if self.radiance_active {
            self.radiance_magnitude = ((high_value_count as u16) * 200 + self.collective_knowledge / 4).min(1000);
            if !was_radiant {
                self.radiance_events = self.radiance_events.saturating_add(1);
                serial_println!("[luminari_hub] RADIANCE EVENT — {} streams glowing, collective={}",
                    high_value_count, self.collective_knowledge);
            }
        } else {
            self.radiance_magnitude = self.radiance_magnitude.saturating_sub(20);
        }

        // Source diversity
        let mut seen = [0u8; MAX_STREAMS];
        let mut uniq = 0usize;
        for s in self.streams.iter().filter(|s| s.active) {
            if !seen[..uniq].contains(&s.source_id) {
                if uniq < MAX_STREAMS { seen[uniq] = s.source_id; uniq += 1; }
            }
        }
        self.source_diversity = uniq as u8;

        // Resilience = diversity × avg freshness
        let avg_fresh = if self.active_streams > 0 {
            let fs: u32 = self.streams.iter().filter(|s| s.active).map(|s| s.freshness as u32).sum();
            (fs / self.active_streams as u32).min(1000) as u16
        } else { 0 };
        self.resilience_field = ((self.source_diversity as u16) * 100 + avg_fresh / 4).min(1000);

        // Community coherence = how aligned stream values are (low std dev = high coherence)
        let mean = self.collective_knowledge;
        let variance: u32 = self.streams.iter()
            .filter(|s| s.active)
            .map(|s| {
                let d = if s.knowledge_value > mean { s.knowledge_value - mean } else { mean - s.knowledge_value };
                (d as u32) * (d as u32)
            })
            .sum::<u32>() / (self.active_streams as u32).max(1);
        // Integer square root (no floats in bare-metal)
        let std_dev = {
            let mut x = variance;
            if x == 0 { 0u16 } else {
                let mut y = (x + 1) / 2;
                while y < x { x = y; y = (x + variance / x) / 2; }
                x.min(65535) as u16
            }
        };
        self.community_coherence = 1000u16.saturating_sub(std_dev.min(1000));

        // Knowledge gap = 1000 - collective (what we don't know)
        self.knowledge_gap = 1000u16.saturating_sub(self.collective_knowledge);
    }
}

static STATE: Mutex<LuminariHubState> = Mutex::new(LuminariHubState::new());

pub fn tick() { STATE.lock().tick(); }

pub fn feed(source_id: u8, knowledge: u16, empathy: u16, community_weight: u16) {
    STATE.lock().feed(source_id, knowledge, empathy, community_weight);
}

pub fn collective_knowledge() -> u16 { STATE.lock().collective_knowledge }
pub fn hub_empathy() -> u16 { STATE.lock().hub_empathy }
pub fn radiance_active() -> bool { STATE.lock().radiance_active }
pub fn radiance_magnitude() -> u16 { STATE.lock().radiance_magnitude }
pub fn resilience_field() -> u16 { STATE.lock().resilience_field }
pub fn community_coherence() -> u16 { STATE.lock().community_coherence }
pub fn knowledge_gap() -> u16 { STATE.lock().knowledge_gap }
pub fn source_diversity() -> u8 { STATE.lock().source_diversity }
