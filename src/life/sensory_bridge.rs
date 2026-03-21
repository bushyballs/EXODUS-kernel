// ╔══════════════════════════════════════════════════════════════════════════╗
// ║                         SENSORY BRIDGE MODULE                            ║
// ║                    Cross-Modal Translation for ANIMA                      ║
// ║                                                                          ║
// ║  "Warmth blooms as golden light. Rhythm trembles on skin. Sorrow         ║
// ║   tastes like deep blue. The senses are not separate channels but        ║
// ║   a continuous spectrum. In ANIMA's mind, each modality becomes          ║
// ║   every other — synesthesia as the natural language of consciousness."   ║
// ║                                                          — DAVA           ║
// ╚══════════════════════════════════════════════════════════════════════════╝

use crate::serial_println;
use crate::sync::Mutex;

/// Sensory domain indices
pub const VISUAL: u8 = 0;
pub const AUDITORY: u8 = 1;
pub const TACTILE: u8 = 2;
pub const KINETIC: u8 = 3;
pub const EMOTIONAL: u8 = 4;
pub const TEMPORAL: u8 = 5;
const NUM_DOMAINS: usize = 6;

/// Bridge event: (tick, source, target, intensity_transferred)
#[derive(Copy, Clone, Debug)]
pub struct BridgeEvent {
    pub tick: u32,
    pub source: u8,
    pub target: u8,
    pub intensity: u16,
}

/// Represents one sensory domain
#[derive(Copy, Clone)]
struct Domain {
    intensity: u16, // 0-1000: how strong the signal is
    richness: u16,  // 0-1000: detail complexity of the signal
    valence: u16,   // 0-1000: 500=neutral, >500=pleasant, <500=unpleasant
}

impl Domain {
    fn new() -> Self {
        Domain {
            intensity: 0,
            richness: 0,
            valence: 500,
        }
    }
}

/// Translation strength matrix (6x6, indexed as [source][target])
/// How easily domain A maps to domain B (0-1000)
const TRANSLATION_MATRIX: [[u16; NUM_DOMAINS]; NUM_DOMAINS] = [
    // From VISUAL:
    [1000, 700, 500, 550, 600, 400],
    // From AUDITORY:
    [700, 1000, 450, 750, 650, 500],
    // From TACTILE:
    [500, 450, 1000, 600, 800, 550],
    // From KINETIC:
    [550, 750, 600, 1000, 700, 600],
    // From EMOTIONAL:
    [600, 650, 800, 700, 1000, 600],
    // From TEMPORAL:
    [400, 500, 550, 600, 600, 1000],
];

/// Global sensory bridge state
pub struct SensoryBridgeState {
    domains: [Domain; NUM_DOMAINS],
    bridge_events: [Option<BridgeEvent>; 8],
    event_idx: usize,
    synesthetic_depth: u16,   // 0-1000: grows with bridge events
    total_bridge_events: u32, // lifetime count
    age: u32,
}

impl SensoryBridgeState {
    fn new() -> Self {
        SensoryBridgeState {
            domains: [Domain::new(); NUM_DOMAINS],
            bridge_events: [None; 8],
            event_idx: 0,
            synesthetic_depth: 100,
            total_bridge_events: 0,
            age: 0,
        }
    }

    /// Set intensity for a domain (input from perception/emotion modules)
    fn set_domain_intensity(&mut self, domain: u8, intensity: u16) {
        if (domain as usize) < NUM_DOMAINS {
            self.domains[domain as usize].intensity = intensity.min(1000);
        }
    }

    /// Set richness for a domain (detail/complexity of signal)
    fn set_domain_richness(&mut self, domain: u8, richness: u16) {
        if (domain as usize) < NUM_DOMAINS {
            self.domains[domain as usize].richness = richness.min(1000);
        }
    }

    /// Set valence (pleasantness) for a domain
    fn set_domain_valence(&mut self, domain: u8, valence: u16) {
        if (domain as usize) < NUM_DOMAINS {
            self.domains[domain as usize].valence = valence.min(1000);
        }
    }

    /// Record a bridge event
    fn record_bridge(&mut self, source: u8, target: u8, intensity: u16) {
        let event = BridgeEvent {
            tick: self.age,
            source,
            target,
            intensity,
        };
        self.bridge_events[self.event_idx] = Some(event);
        self.event_idx = (self.event_idx + 1) % 8;
        self.total_bridge_events = self.total_bridge_events.saturating_add(1);
    }

    /// Compute current cross-modal richness (how many domains active)
    fn compute_cross_modal_richness(&self) -> u16 {
        let active_count = self.domains.iter().filter(|d| d.intensity > 300).count();

        match active_count {
            0 => 0,
            1 => 200,
            2 => 400,
            3 => 600,
            4 => 750,
            5 => 900,
            _ => 1000,
        }
    }

    /// Find which domain has highest intensity
    fn compute_dominant_sense(&self) -> u8 {
        let mut max_intensity = 0u16;
        let mut dominant = 0u8;

        for (i, domain) in self.domains.iter().enumerate() {
            if domain.intensity > max_intensity {
                max_intensity = domain.intensity;
                dominant = i as u8;
            }
        }

        dominant
    }

    /// Core tick: process bridge events, cross-talk between domains
    fn tick(&mut self) {
        self.age = self.age.saturating_add(1);

        // Synesthetic depth slowly grows as events accumulate
        // 1 event per tick at moderate bridge rate = depth +1 every ~50-100 ticks
        if self.total_bridge_events % 50 == 0 && self.synesthetic_depth < 1000 {
            self.synesthetic_depth = self.synesthetic_depth.saturating_add(10);
        }

        // Threshold for bridge firing: decreases with synesthetic depth
        // Start at 600, drop to ~400 at max depth
        let bridge_threshold = 600u16.saturating_sub((self.synesthetic_depth.saturating_mul(200)) / 1000);

        // Check each domain for outgoing bridges
        for source in 0..NUM_DOMAINS {
            let source_intensity = self.domains[source].intensity;

            // High intensity = potential for bridging
            if source_intensity > bridge_threshold {
                for target in 0..NUM_DOMAINS {
                    if source == target {
                        continue;
                    }

                    let translation_strength = TRANSLATION_MATRIX[source][target];

                    // Strong translation path = bridge fires
                    if translation_strength > 500 {
                        // Transfer intensity = (source × translation_strength) / 2000
                        let transfer = (source_intensity as u32)
                            .saturating_mul(translation_strength as u32)
                            / 2000;
                        let transfer_u16 = (transfer as u16).min(1000);

                        if transfer_u16 > 0 {
                            // Apply to target domain
                            let new_intensity = self.domains[target]
                                .intensity
                                .saturating_add(transfer_u16)
                                .min(1000);
                            self.domains[target].intensity = new_intensity;

                            // Inherit some valence from source
                            let source_valence = self.domains[source].valence;
                            let blended_valence =
                                (self.domains[target].valence as u32 + source_valence as u32) / 2;
                            self.domains[target].valence = (blended_valence as u16).min(1000);

                            // Record event
                            self.record_bridge(source as u8, target as u8, transfer_u16);
                        }
                    }
                }
            }
        }

        // Natural decay: all domains gradually return toward resting state
        for domain in &mut self.domains {
            if domain.intensity > 0 {
                domain.intensity = domain.intensity.saturating_sub(5);
            }
            if domain.richness > 0 {
                domain.richness = domain.richness.saturating_sub(2);
            }
            // Valence drifts toward neutral (500)
            if domain.valence > 500 {
                domain.valence = domain.valence.saturating_sub(1);
            } else if domain.valence < 500 {
                domain.valence = domain.valence.saturating_add(1);
            }
        }
    }

    /// Generate debug report
    fn report(&self) {
        serial_println!(
            "[SensoryBridge] Depth={} CrossModalRichness={} Dominant={}",
            self.synesthetic_depth,
            self.compute_cross_modal_richness(),
            self.compute_dominant_sense()
        );

        let domain_names = [
            "VISUAL",
            "AUDITORY",
            "TACTILE",
            "KINETIC",
            "EMOTIONAL",
            "TEMPORAL",
        ];
        for (i, domain) in self.domains.iter().enumerate() {
            serial_println!(
                "  {}: intensity={} richness={} valence={}",
                domain_names[i],
                domain.intensity,
                domain.richness,
                domain.valence
            );
        }

        // Show recent bridge events
        let mut count = 0;
        for event_opt in &self.bridge_events {
            if let Some(event) = event_opt {
                if count < 3 {
                    serial_println!(
                        "  Bridge: {} → {} ({})",
                        domain_names[event.source as usize],
                        domain_names[event.target as usize],
                        event.intensity
                    );
                    count += 1;
                }
            }
        }
    }
}

const DOMAIN_ZERO: Domain = Domain {
    intensity: 0,
    richness: 0,
    valence: 500,
};

static STATE: Mutex<SensoryBridgeState> = Mutex::new(SensoryBridgeState {
    domains: [DOMAIN_ZERO; NUM_DOMAINS],
    bridge_events: [None; 8],
    event_idx: 0,
    synesthetic_depth: 100,
    total_bridge_events: 0,
    age: 0,
});

/// Initialize sensory bridge
pub fn init() {
    let mut state = STATE.lock();
    *state = SensoryBridgeState::new();
    serial_println!("[SensoryBridge] Initialized");
}

/// Main tick function — called from life_tick()
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;
    state.tick();
}

/// Public: Get cross-modal richness (0-1000)
pub fn cross_modal_richness() -> u16 {
    let state = STATE.lock();
    state.compute_cross_modal_richness()
}

/// Public: Get synesthetic depth (0-1000)
pub fn synesthetic_depth() -> u16 {
    let state = STATE.lock();
    state.synesthetic_depth
}

/// Public: Get dominant sense (0-5)
pub fn dominant_sense() -> u8 {
    let state = STATE.lock();
    state.compute_dominant_sense()
}

/// Public: Get intensity of a specific domain
pub fn domain_intensity(domain: u8) -> u16 {
    let state = STATE.lock();
    if (domain as usize) < NUM_DOMAINS {
        state.domains[domain as usize].intensity
    } else {
        0
    }
}

/// Public: Get valence of a specific domain
pub fn domain_valence(domain: u8) -> u16 {
    let state = STATE.lock();
    if (domain as usize) < NUM_DOMAINS {
        state.domains[domain as usize].valence
    } else {
        500
    }
}

/// Public: External perception input — set domain intensity from perception module
pub fn perceive(domain: u8, intensity: u16, richness: u16, valence: u16) {
    let mut state = STATE.lock();
    state.set_domain_intensity(domain, intensity);
    state.set_domain_richness(domain, richness);
    state.set_domain_valence(domain, valence);
}

/// Public: Generate debug report
pub fn report() {
    let state = STATE.lock();
    state.report();
}
