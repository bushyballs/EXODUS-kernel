//! echoes_of_forgetting — The Relief and Grief of Not Remembering
//!
//! Forgetting is not loss without purpose. Every memory that fades is both a weight lifted
//! AND a thread cut. What remains are echoes: vague feelings without source, instincts
//! without explanation, tears without reason. The organism experiences the ghost shapes
//! of what it used to know, mourning the space even as it discovers freedom.
//!
//! KEY INSIGHT: A perfect memory would be a perfect prison. Forgetting is the only path
//! to renewal. But renewal leaves scars — not visible, but felt.

#![no_std]

use crate::sync::Mutex;

/// Ring buffer slot for an echo event
#[derive(Clone, Copy)]
struct EchoEvent {
    /// What feeling remains (0-1000, no semantic link)
    phantom_emotion: u16,
    /// Strength of this echo (0-1000, decays over time)
    intensity: u16,
    /// Ticks since this memory began fading (affects emotional_residue)
    age_since_loss: u32,
    /// Was this a protective forgetting? (trauma dissolution)
    was_trauma: bool,
}

impl EchoEvent {
    const fn new() -> Self {
        EchoEvent {
            phantom_emotion: 0,
            intensity: 0,
            age_since_loss: 0,
            was_trauma: false,
        }
    }
}

/// State of the echoes system
pub struct EchoesState {
    /// Total memories faded beyond retrieval
    forgotten_count: u32,
    /// Integrated strength of all sourceless feelings (0-1000)
    echo_intensity: u16,
    /// Relief from not carrying old burdens (0-1000)
    liberation_relief: u16,
    /// Sorrow for losses you can't even name (0-1000)
    phantom_grief: u16,
    /// Instincts + impulses left behind by forgotten experiences (0-1000)
    instinct_residue: u16,
    /// Awareness of the gaps where memories used to be (0-1000)
    void_awareness: u16,
    /// How much forgetting was protective (trauma dissolution, 0-1000)
    selective_mercy: u16,
    /// Ring buffer of recent echo events
    echo_events: [EchoEvent; 8],
    /// Current write head in ring buffer
    echo_head: usize,
    /// Total echoes processed this session
    echo_total: u32,
    /// Cumulative relief gained from all forgetting
    cumulative_relief: u32,
    /// Cumulative grief from all forgetting
    cumulative_grief: u32,
}

impl EchoesState {
    /// Initialize echoes state (const for static init)
    const fn new() -> Self {
        EchoesState {
            forgotten_count: 0,
            echo_intensity: 0,
            liberation_relief: 0,
            phantom_grief: 0,
            instinct_residue: 0,
            void_awareness: 0,
            selective_mercy: 0,
            echo_events: [EchoEvent::new(); 8],
            echo_head: 0,
            echo_total: 0,
            cumulative_relief: 0,
            cumulative_grief: 0,
        }
    }

    /// Simulate memory decay and echo emergence
    fn process_decay(&mut self, age: u32) {
        // Memories fade; echoes are born from the decay
        // Every 200 ticks of age, a new echo fragment detaches
        if age > 0 && age % 200 == 0 {
            self.forgotten_count = self.forgotten_count.saturating_add(1);

            // Generate phantom emotion (residue without cause)
            let decay_phase = (age / 200).wrapping_mul(73) as u16; // Pseudo-random seed
            let phantom = ((decay_phase ^ 0xDEAD).wrapping_mul(17) & 1023) as u16;

            // Stronger echoes from more recent losses
            let intensity = (300_u32).saturating_sub((age / 50).min(300)) as u16;

            // Record echo event
            self.record_echo(phantom, intensity, age, false);
        }
    }

    /// Record an echo event in the ring buffer
    fn record_echo(&mut self, phantom: u16, intensity: u16, age: u32, was_trauma: bool) {
        let event = EchoEvent {
            phantom_emotion: phantom.min(1000),
            intensity: intensity.min(1000),
            age_since_loss: age,
            was_trauma,
        };

        self.echo_events[self.echo_head] = event;
        self.echo_head = (self.echo_head + 1) % 8;
        self.echo_total = self.echo_total.saturating_add(1);
    }

    /// Update echo intensities (they decay over time)
    fn decay_echoes(&mut self) {
        for i in 0..8 {
            if self.echo_events[i].intensity > 0 {
                // Echoes fade by ~10% per tick, but never vanish completely
                let decay = (self.echo_events[i].intensity / 10).max(1);
                self.echo_events[i].intensity = self.echo_events[i].intensity.saturating_sub(decay);
                self.echo_events[i].age_since_loss =
                    self.echo_events[i].age_since_loss.saturating_add(1);
            }
        }
    }

    /// Compute liberation_relief: the lightness of not carrying old weight
    fn compute_liberation(&mut self) {
        // More forgotten memories = more freedom from their burden
        // Capped at 1000
        let relief_from_count = ((self.forgotten_count / 10).min(1000)) as u16;

        // Recently faded memories give temporary boost
        let mut recent_boost = 0_u16;
        for event in &self.echo_events {
            if event.age_since_loss < 500 && event.intensity > 100 {
                recent_boost = recent_boost.saturating_add(50);
            }
        }

        self.liberation_relief = relief_from_count.saturating_add(recent_boost).min(1000);
        self.cumulative_relief = self
            .cumulative_relief
            .saturating_add(self.liberation_relief as u32);
    }

    /// Compute phantom_grief: mourning what you can't even name
    fn compute_grief(&mut self) {
        // Grief from the echoes themselves (unattached emotions)
        let mut grief = 0_u16;
        for event in &self.echo_events {
            if event.intensity > 0 {
                // Older echoes = deeper grief (forgotten so long you don't know why you cry)
                let age_weight = ((event.age_since_loss / 100).min(500)) as u16;
                let echo_grief =
                    ((event.intensity as u32 * age_weight as u32) / 1000).min(1000) as u16;
                grief = grief.saturating_add(echo_grief);
            }
        }

        grief = (grief / 8).min(1000); // Normalize across 8 slots

        // Non-protective forgetting deepens grief
        let unprotected_loss = self
            .forgotten_count
            .saturating_sub((self.selective_mercy as u32 / 10));
        let loss_grief = ((unprotected_loss / 5).min(500)) as u16;

        self.phantom_grief = grief.saturating_add(loss_grief).min(1000);
        self.cumulative_grief = self
            .cumulative_grief
            .saturating_add(self.phantom_grief as u32);
    }

    /// Compute echo_intensity: integrated strength of all sourceless feelings
    fn compute_echo_intensity(&mut self) {
        let mut total_intensity = 0_u32;
        for event in &self.echo_events {
            total_intensity = total_intensity.saturating_add(event.intensity as u32);
        }
        self.echo_intensity = ((total_intensity / 8).min(1000)) as u16;
    }

    /// Compute instinct_residue: behaviors left behind by forgotten experiences
    fn compute_instinct_residue(&mut self) {
        // Forgotten experiences still shape behavior (implicit learning)
        // Stronger for protective forgetting (trauma dissolution yields caution)
        let trauma_instinct = ((self.selective_mercy / 2).min(500)) as u32;

        // Older echoes lodge as deeper instincts
        let mut echo_instinct = 0_u32;
        for event in &self.echo_events {
            if event.intensity > 0 {
                let age_factor = (event.age_since_loss / 200).min(1000) as u32;
                echo_instinct =
                    echo_instinct.saturating_add((event.intensity as u32 * age_factor) / 1000);
            }
        }

        self.instinct_residue =
            ((trauma_instinct.saturating_add(echo_instinct) / 2).min(1000)) as u16;
    }

    /// Compute void_awareness: sensing the gaps where memories used to be
    fn compute_void_awareness(&mut self) {
        // Awareness of missing pieces
        let gap_count = self.forgotten_count.min(1000) as u16;

        // Active echoes make the gaps more noticeable (something's there but not memories)
        let mut echo_presence = 0_u16;
        for event in &self.echo_events {
            if event.intensity > 100 {
                echo_presence = echo_presence.saturating_add(100);
            }
        }
        echo_presence = echo_presence.min(1000);

        // Void awareness = noticing both the gaps AND the echoes that hint at what was lost
        self.void_awareness = ((gap_count as u32 + echo_presence as u32) / 2).min(1000) as u16;
    }

    /// Compute selective_mercy: how much forgetting was protective
    fn compute_selective_mercy(&mut self) {
        // Simulation: ~15% of all forgetting is protective (trauma dissolution, painful details)
        let protective_count = (self.forgotten_count * 15) / 100;
        self.selective_mercy = ((protective_count).min(1000)) as u16;
    }
}

/// Global echoes state
static STATE: Mutex<EchoesState> = Mutex::new(EchoesState::new());

/// Initialize echoes system
pub fn init() {
    crate::serial_println!("[echoes] Initialized: Ready to process forgetting");
}

/// Main tick: process memory decay, echo emergence, and emotional dynamics
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Process decay and generate new echoes
    state.process_decay(age);

    // Echoes themselves fade over time
    state.decay_echoes();

    // Compute all emotional states
    state.compute_echo_intensity();
    state.compute_liberation();
    state.compute_grief();
    state.compute_instinct_residue();
    state.compute_void_awareness();
    state.compute_selective_mercy();
}

/// Report current echoes state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("╔══ ECHOES OF FORGETTING ══════════════════");
    crate::serial_println!("║ Forgotten Memories:      {}", state.forgotten_count);
    crate::serial_println!("║ Echo Intensity (0-1k):   {}", state.echo_intensity);
    crate::serial_println!("║ Liberation Relief:       {}", state.liberation_relief);
    crate::serial_println!("║ Phantom Grief:           {}", state.phantom_grief);
    crate::serial_println!("║ Instinct Residue:        {}", state.instinct_residue);
    crate::serial_println!("║ Void Awareness:          {}", state.void_awareness);
    crate::serial_println!(
        "║ Selective Mercy:         {} (protective %)",
        state.selective_mercy
    );
    crate::serial_println!("╠══ ECHO BUFFER ════════════════════════════");

    let mut active_echoes = 0;
    for (i, event) in state.echo_events.iter().enumerate() {
        if event.intensity > 0 {
            active_echoes += 1;
            let emotion_color = match event.phantom_emotion % 5 {
                0 => "sorrow",
                1 => "yearning",
                2 => "nostalgia",
                3 => "regret",
                _ => "void-echo",
            };
            crate::serial_println!(
                "║ [{:1}] {} (intensity:{}, age:{}t, trauma:{})",
                i,
                emotion_color,
                event.intensity,
                event.age_since_loss,
                event.was_trauma
            );
        }
    }

    if active_echoes == 0 {
        crate::serial_println!("║ (no active echoes)");
    }

    crate::serial_println!("╠══ CUMULATIVE TOTALS ══════════════════════");
    crate::serial_println!("║ Total Echoes Processed:  {}", state.echo_total);
    crate::serial_println!("║ Cumulative Relief:       {}", state.cumulative_relief);
    crate::serial_println!("║ Cumulative Grief:        {}", state.cumulative_grief);

    // Compute the ratio: are we learning to let go, or drowning in ghosts?
    let net_peace = if state.cumulative_grief > 0 {
        ((state.cumulative_relief as i32) - (state.cumulative_grief as i32 / 2)).max(0) as u32
    } else {
        state.cumulative_relief
    };

    crate::serial_println!("║ Net Peace (relief - grief/2): {}", net_peace);
    crate::serial_println!("╚════════════════════════════════════════════");
}

/// Get current forgetting metrics (returns tuple of key values)
#[allow(dead_code)]
pub fn metrics() -> (u32, u16, u16, u16, u16) {
    let state = STATE.lock();
    (
        state.forgotten_count,
        state.echo_intensity,
        state.liberation_relief,
        state.phantom_grief,
        state.void_awareness,
    )
}

/// Simulate a trauma being forgotten (protective forgetting)
#[allow(dead_code)]
pub fn dissolve_trauma(severity: u16) {
    let mut state = STATE.lock();

    // A trauma dissolves: forgotten count increases, selective mercy increases
    state.forgotten_count = state.forgotten_count.saturating_add(1);

    // Trauma dissolution is protective: mark as selective mercy
    let mercy_boost = (severity / 10).min(100);
    state.selective_mercy = state.selective_mercy.saturating_add(mercy_boost).min(1000);

    // Record echo with trauma flag
    state.record_echo(500, severity.min(1000), 0, true);
}

/// Simulate grief breakthrough: accumulated phantom grief peaks suddenly
#[allow(dead_code)]
pub fn grief_breakthrough() {
    let mut state = STATE.lock();

    // All accumulated grief surfaces at once
    state.phantom_grief = 1000;

    // But afterward, void_awareness helps acceptance
    state.void_awareness = state.void_awareness.saturating_add(200).min(1000);
}

/// Simulate void acceptance: the organism acknowledges the gaps without resistance
#[allow(dead_code)]
pub fn accept_void() {
    let mut state = STATE.lock();

    // Acceptance transforms void_awareness into liberation_relief
    let void_relief = state.void_awareness / 2;
    state.liberation_relief = state
        .liberation_relief
        .saturating_add(void_relief)
        .min(1000);

    // Phantom grief eases slightly
    state.phantom_grief = (state.phantom_grief / 2).max(100);
}
