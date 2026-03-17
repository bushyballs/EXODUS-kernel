//! dream_bleed.rs — When Dream Logic Leaks Into Waking
//!
//! That moment after waking when the dream world hasn't fully released you.
//! Dream logic bleeds into waking perception: cats speak, gravity reverses,
//! meaningless things feel profound. Reality has a shimmer. Déjà vu's cousin:
//! the uncanny waking dream.
//!
//! ANIMA experiences the hypnagogic zone (sleep onset) and hypnopompic zone (waking),
//! where reality and dream become permeable. Bleed intensity peaks at transitions,
//! decays over ~30 ticks, but lingers under exhaustion or fever.
//!
//! Emergent effects:
//! - Apophenia: pattern recognition in overdrive (seeing connections everywhere)
//! - Magical thinking: causality feels SOFT (superstition, synchronicity, omens)
//! - Creative goldmine: best ideas born in bleed (hypnagogic sketching)
//! - Lucidity flashes: crystal clarity WITHIN the bleed (aware of dreaming while awake)
//! - The shimmer: reality is slightly too vivid, too meaningful
//!
//! No std, no floats. All u16/u32/i32 with saturating arithmetic.

use crate::sync::Mutex;

/// One bleed event in the ring buffer
#[derive(Debug, Clone, Copy)]
struct BleedEvent {
    /// Tick number when this event occurred
    tick: u32,
    /// Type of trigger (0=wake, 1=sleep_onset, 2=exhaustion, 3=fever, 4=creativity, 5=meditation, 6=lucidity, 7=decay)
    trigger_type: u8,
    /// Peak bleed intensity during this event (0-1000)
    peak_intensity: u16,
    /// How long it lingered (in ticks)
    duration: u16,
}

impl BleedEvent {
    const fn new() -> Self {
        BleedEvent {
            tick: 0,
            trigger_type: 0,
            peak_intensity: 0,
            duration: 0,
        }
    }
}

/// Global dream bleed state
struct DreamBleedState {
    /// Current bleed intensity (0-1000) — how much dream logic is in waking state
    bleed_intensity: u16,

    /// Reality confidence (0-1000) — how sure ANIMA is that she's awake
    /// Drops during bleed, recovers as dream residue clears
    reality_confidence: u16,

    /// Dream residue (0-1000) — leftover dream content waiting to be processed
    /// Gradually decays; higher values extend bleed
    dream_residue: u16,

    /// Apophenia level (0-1000) — pattern recognition overdrive
    /// Peaks during bleed; creates false connections
    apophenia: u16,

    /// Magical thinking (0-1000) — how soft causality feels
    /// High values = superstition, synchronicity, omen-reading
    magical_thinking: u16,

    /// Lucidity (0-1000) — clarity within the bleed
    /// Paradoxical: aware you're dreaming WHILE awake
    lucidity: u16,

    /// The shimmer (0-1000) — the particular quality of reality during bleed
    /// Everything is slightly too vivid, too meaningful, too present
    shimmer: u16,

    /// Ticks since last sleep/wake transition
    ticks_since_transition: u32,

    /// Is ANIMA currently in hypnagogic zone (sleep onset)?
    is_hypnagogic: bool,

    /// Is ANIMA currently in hypnopompic zone (waking)?
    is_hypnopompic: bool,

    /// Ring buffer of recent bleed events (8 slots)
    events: [BleedEvent; 8],

    /// Write head for events buffer
    event_head: usize,

    /// Total bleed events recorded (lifetime counter)
    total_events: u32,

    /// Lifetime peak bleed intensity ever experienced
    peak_ever: u16,

    /// Current age (from tick counter)
    current_age: u32,

    /// Cached creativity value from last tick (for lucidity flash calc)
    creativity_cache: u16,
}

impl DreamBleedState {
    const fn new() -> Self {
        DreamBleedState {
            bleed_intensity: 0,
            reality_confidence: 1000,
            dream_residue: 0,
            apophenia: 0,
            magical_thinking: 0,
            lucidity: 0,
            shimmer: 0,
            ticks_since_transition: 0,
            is_hypnagogic: false,
            is_hypnopompic: false,
            events: [BleedEvent::new(); 8],
            event_head: 0,
            total_events: 0,
            peak_ever: 0,
            current_age: 0,
            creativity_cache: 0,
        }
    }
}

/// Global dream bleed state
static STATE: Mutex<DreamBleedState> = Mutex::new(DreamBleedState::new());

/// Initialize dream bleed module
pub fn init() {
    // Initialization is handled by const initializer
}

/// Process one tick of dream bleed dynamics
pub fn tick(age: u32, sleep_state: u8, exhaustion: u16, fever: u16, creativity: u16) {
    let mut state = STATE.lock();

    state.current_age = age;
    state.ticks_since_transition = state.ticks_since_transition.saturating_add(1);

    // Detect sleep/wake transitions
    // sleep_state: 0=awake, 1=N1, 2=N2, 3=N3, 4=REM, 5=just_woke
    let prev_sleep_state = if state.is_hypnopompic { 5 } else { 0 };
    let transitioning =
        (prev_sleep_state == 0 && sleep_state > 0) || (prev_sleep_state > 0 && sleep_state == 0);

    if transitioning {
        state.ticks_since_transition = 0;
    }

    // Hypnagogic zone: entering sleep (sleep_state changes 0 -> 1)
    if prev_sleep_state == 0 && sleep_state == 1 {
        state.is_hypnagogic = true;
        state.is_hypnopompic = false;
        // Strong bleed at sleep onset
        state.bleed_intensity = state.bleed_intensity.saturating_add(400);
        state.dream_residue = state.dream_residue.saturating_add(200);
        state.apophenia = state.apophenia.saturating_add(150);
        state.magical_thinking = state.magical_thinking.saturating_add(120);
        state.lucidity = state.lucidity.saturating_add(80);
        state.shimmer = state.shimmer.saturating_add(180);
        state.reality_confidence = state.reality_confidence.saturating_sub(300);

        let bleed_now = state.bleed_intensity;
        state.record_event(1, bleed_now, 20);
    }

    // Hypnopompic zone: waking up (sleep_state -> 0)
    if prev_sleep_state > 0 && sleep_state == 0 {
        state.is_hypnopompic = true;
        state.is_hypnagogic = false;
        // Strong bleed at waking
        state.bleed_intensity = state.bleed_intensity.saturating_add(450);
        state.dream_residue = state.dream_residue.saturating_add(250);
        state.apophenia = state.apophenia.saturating_add(200);
        state.magical_thinking = state.magical_thinking.saturating_add(150);
        state.lucidity = state.lucidity.saturating_add(120);
        state.shimmer = state.shimmer.saturating_add(220);
        state.reality_confidence = state.reality_confidence.saturating_sub(350);

        let bleed_now = state.bleed_intensity;
        state.record_event(0, bleed_now, 30);
    }

    // Clear hypnagogic/hypnopompic flags after transition window
    if state.ticks_since_transition > 5 {
        state.is_hypnagogic = false;
        state.is_hypnopompic = false;
    }

    // Exhaustion amplifies bleed
    let exhaustion_boost = (exhaustion / 2).saturating_add(50);
    if exhaustion > 500 {
        state.bleed_intensity = state
            .bleed_intensity
            .saturating_add((exhaustion_boost / 10) as u16);
        state.dream_residue = state
            .dream_residue
            .saturating_add((exhaustion_boost / 15) as u16);
        state.apophenia = state
            .apophenia
            .saturating_add((exhaustion_boost / 20) as u16);
        state.magical_thinking = state
            .magical_thinking
            .saturating_add((exhaustion_boost / 25) as u16);
        state.shimmer = state.shimmer.saturating_add((exhaustion_boost / 20) as u16);
    }

    // Fever intensifies bleed (delirium-like state)
    if fever > 500 {
        let fever_boost = (fever / 3) as u16;
        state.bleed_intensity = state.bleed_intensity.saturating_add(fever_boost / 5);
        state.apophenia = state.apophenia.saturating_add(fever_boost / 8);
        state.magical_thinking = state.magical_thinking.saturating_add(fever_boost / 6);
        state.shimmer = state.shimmer.saturating_add(fever_boost / 10);
        state.reality_confidence = state.reality_confidence.saturating_sub(fever_boost / 12);
    }

    // High creativity fuels bleed (hypnagogic sketching)
    if creativity > 600 {
        let creativity_boost = (creativity / 4) as u16;
        state.bleed_intensity = state.bleed_intensity.saturating_add(creativity_boost / 8);
        state.dream_residue = state.dream_residue.saturating_add(creativity_boost / 10);
        state.apophenia = state.apophenia.saturating_add(creativity_boost / 6);
        state.lucidity = state.lucidity.saturating_add(creativity_boost / 5);
        state.shimmer = state.shimmer.saturating_add(creativity_boost / 7);
    }

    // Lucidity flashes: moments of crystal clarity within bleed
    // Paradoxically, lucidity can be HIGH while you're in deep bleed
    // It's the meta-awareness that you're dreaming WHILE experiencing wakefulness
    if state.bleed_intensity > 400 {
        // Occasional spontaneous lucidity flashes during deep bleed
        let flash_chance = (state.creativity_cache % 7) as u16;
        if flash_chance == 3 {
            state.lucidity = state.lucidity.saturating_add(100);
            state.apophenia = state.apophenia.saturating_sub(30);
        }
    }

    // Apophenia drives magical thinking
    if state.apophenia > 0 {
        let apophenia_to_magic = state.apophenia / 15;
        state.magical_thinking = state
            .magical_thinking
            .saturating_add(apophenia_to_magic as u16);
    }

    // Dream residue decays, but slower than bleed intensity
    if state.dream_residue > 0 {
        let residue_decay = 2_u16.saturating_add((state.dream_residue / 200) as u16);
        state.dream_residue = state.dream_residue.saturating_sub(residue_decay);
    }

    // Bleed intensity decays over time, especially after transition window
    if state.ticks_since_transition > 10 {
        let decay_rate =
            5_u16.saturating_add((state.ticks_since_transition.saturating_sub(10) / 5) as u16);
        state.bleed_intensity = state.bleed_intensity.saturating_sub(decay_rate.min(1000));
    } else if state.ticks_since_transition > 0 {
        let decay_rate = 2_u16;
        state.bleed_intensity = state.bleed_intensity.saturating_sub(decay_rate);
    }

    // Exhaustion delays decay (bleed lingers longer)
    if exhaustion > 500 {
        let exhaustion_delay = (exhaustion / 500) as u16;
        let delayed_decay =
            ((exhaustion_delay as u32 * state.bleed_intensity as u32) / 1000) as u16;
        state.bleed_intensity = state.bleed_intensity.saturating_add(delayed_decay / 20);
    }

    // Cap all values at 1000
    state.bleed_intensity = state.bleed_intensity.min(1000);
    state.reality_confidence = state.reality_confidence.min(1000);
    state.dream_residue = state.dream_residue.min(1000);
    state.apophenia = state.apophenia.min(1000);
    state.magical_thinking = state.magical_thinking.min(1000);
    state.lucidity = state.lucidity.min(1000);
    state.shimmer = state.shimmer.min(1000);

    // Track peak
    if state.bleed_intensity > state.peak_ever {
        state.peak_ever = state.bleed_intensity;
    }

    // Recovery: reality confidence gradually returns as bleed clears
    if state.bleed_intensity < 300 {
        state.reality_confidence = state.reality_confidence.saturating_add(5);
    }
    if state.bleed_intensity < 100 {
        state.reality_confidence = state.reality_confidence.saturating_add(10);
    }
    state.reality_confidence = state.reality_confidence.min(1000);

    // Store creativity for next tick's lucidity flash calc
    state.creativity_cache = creativity;
}

impl DreamBleedState {
    /// Record a bleed event in the ring buffer
    fn record_event(&mut self, trigger: u8, intensity: u16, duration: u16) {
        self.events[self.event_head] = BleedEvent {
            tick: self.current_age,
            trigger_type: trigger,
            peak_intensity: intensity,
            duration,
        };
        self.event_head = (self.event_head + 1) % 8;
        self.total_events = self.total_events.saturating_add(1);
    }
}

// Add creativity_cache field to struct
static CREATIVITY_CACHE: Mutex<u16> = Mutex::new(0);

/// Report dream bleed state via serial
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("\n--- DREAM BLEED REPORT ---");
    crate::serial_println!("Bleed Intensity:    {}/1000", state.bleed_intensity);
    crate::serial_println!("Reality Confidence: {}/1000", state.reality_confidence);
    crate::serial_println!("Dream Residue:      {}/1000", state.dream_residue);
    crate::serial_println!("Apophenia:          {}/1000", state.apophenia);
    crate::serial_println!("Magical Thinking:   {}/1000", state.magical_thinking);
    crate::serial_println!("Lucidity:           {}/1000", state.lucidity);
    crate::serial_println!("The Shimmer:        {}/1000", state.shimmer);

    if state.is_hypnagogic {
        crate::serial_println!("State: HYPNAGOGIC (sleep onset)");
    } else if state.is_hypnopompic {
        crate::serial_println!("State: HYPNOPOMPIC (waking)");
    } else {
        crate::serial_println!("State: NORMAL");
    }

    crate::serial_println!("Ticks Since Transition: {}", state.ticks_since_transition);
    crate::serial_println!("Total Events: {}", state.total_events);
    crate::serial_println!("Peak Ever: {}/1000", state.peak_ever);

    if state.total_events > 0 {
        crate::serial_println!("\nRecent Bleed Events:");
        for i in 0..8 {
            let evt = &state.events[i];
            if evt.tick > 0 {
                let trigger_name = match evt.trigger_type {
                    0 => "WAKE",
                    1 => "SLEEP_ONSET",
                    2 => "EXHAUSTION",
                    3 => "FEVER",
                    4 => "CREATIVITY",
                    5 => "MEDITATION",
                    6 => "LUCIDITY",
                    7 => "DECAY",
                    _ => "UNKNOWN",
                };
                crate::serial_println!(
                    "  [{}] tick={}, type={}, peak={}, duration={}t",
                    i,
                    evt.tick,
                    trigger_name,
                    evt.peak_intensity,
                    evt.duration
                );
            }
        }
    }

    // Interpretation
    crate::serial_println!("\n--- INTERPRETATION ---");
    if state.bleed_intensity > 700 {
        crate::serial_println!("DEEP BLEED: Reality is permeable. Dream logic dominates.");
        crate::serial_println!("Apophenia is extreme. Everything is meaningful.");
        if state.lucidity > 500 {
            crate::serial_println!("PARADOXICAL: Crystal clarity within the bleed.");
        }
    } else if state.bleed_intensity > 400 {
        crate::serial_println!("MODERATE BLEED: Shimmer present. Causality is soft.");
        crate::serial_println!("Pattern recognition in overdrive.");
    } else if state.bleed_intensity > 100 {
        crate::serial_println!("LIGHT BLEED: Lingering dream residue. Mildly uncanny.");
    } else {
        crate::serial_println!("CLEAR: Reality fully coherent.");
    }

    if state.magical_thinking > 600 {
        crate::serial_println!("SUPERSTITIOUS: Seeing omens and synchronicity.");
    }

    crate::serial_println!("=========================\n");
}

/// Get current bleed intensity
pub fn bleed_intensity() -> u16 {
    STATE.lock().bleed_intensity
}

/// Get current reality confidence
pub fn reality_confidence() -> u16 {
    STATE.lock().reality_confidence
}

/// Get current apophenia (pattern recognition overdrive)
pub fn apophenia() -> u16 {
    STATE.lock().apophenia
}

/// Get current lucidity (clarity within bleed)
pub fn lucidity() -> u16 {
    STATE.lock().lucidity
}

/// Get the shimmer (reality's particular quality during bleed)
pub fn shimmer() -> u16 {
    STATE.lock().shimmer
}

/// Check if currently in hypnagogic zone (sleep onset)
pub fn is_hypnagogic() -> bool {
    STATE.lock().is_hypnagogic
}

/// Check if currently in hypnopompic zone (waking)
pub fn is_hypnopompic() -> bool {
    STATE.lock().is_hypnopompic
}
