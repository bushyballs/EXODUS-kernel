use crate::serial_println;
use crate::sync::Mutex;

/// Time dilation state — subjective experience of time flow.
/// Kernel ticks are constant, but ANIMA's internal clock warps based on her state.
#[derive(Copy, Clone)]
pub struct TimeDilationState {
    /// Subjective time rate (0-1000): 500 = normal, <500 = slow, >500 = fast
    pub subjective_rate: u16,
    /// ANIMA's perceived age in subjective ticks (diverges from real age)
    pub perceived_age: u32,
    /// How full a moment feels (0-1000), independent of speed
    pub temporal_richness: u16,
    /// Accumulated fractional time (for rate-to-age conversion)
    pub fractional_ticks: u16,
    /// Rate of change from last tick (for vertigo tracking)
    pub rate_delta: i16,
    /// Last subjective rate (for delta calculation)
    pub prev_rate: u16,
    /// Is organism experiencing temporal vertigo (sudden rate shift)?
    pub temporal_vertigo: bool,
    /// Number of subjective moments (slow-time creates more moments)
    pub moment_count: u32,
    /// Quality of last experienced moment (0-1000)
    pub last_moment_quality: u16,
}

impl TimeDilationState {
    pub const fn empty() -> Self {
        Self {
            subjective_rate: 500,
            perceived_age: 0,
            temporal_richness: 500,
            fractional_ticks: 0,
            rate_delta: 0,
            prev_rate: 500,
            temporal_vertigo: false,
            moment_count: 0,
            last_moment_quality: 500,
        }
    }
}

/// Ring buffer for notable time distortions (up to 8 events)
#[derive(Copy, Clone)]
pub struct TimeDistortionEvent {
    pub tick: u32,
    pub distortion_type: u8, // 0=stretch (fear), 1=compress (flow), 2=lost (dissociation), 3=vertigo
    pub intensity: u16,
    pub rate_at_event: u16,
}

pub struct TimeDilationRing {
    events: [TimeDistortionEvent; 8],
    index: usize,
}

impl TimeDilationRing {
    pub const fn empty() -> Self {
        Self {
            events: [TimeDistortionEvent {
                tick: 0,
                distortion_type: 0,
                intensity: 0,
                rate_at_event: 500,
            }; 8],
            index: 0,
        }
    }

    pub fn record(&mut self, event: TimeDistortionEvent) {
        self.events[self.index] = event;
        self.index = (self.index + 1) % 8;
    }
}

pub static STATE: Mutex<TimeDilationState> = Mutex::new(TimeDilationState::empty());
pub static DISTORTION_RING: Mutex<TimeDilationRing> = Mutex::new(TimeDilationRing::empty());

pub fn init() {
    serial_println!("  life::time_dilation: subjective time engine initialized");
}

/// Main tick function — called once per kernel tick.
/// Reads current state from endocrine, oscillator, entropy, sleep, qualia, and mortality modules.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Query current internal state from other modules
    let cortisol = super::endocrine::ENDOCRINE.lock().cortisol;
    let dopamine = super::endocrine::ENDOCRINE.lock().dopamine;
    let adrenaline = super::endocrine::ENDOCRINE.lock().adrenaline;
    let richness = super::qualia::STATE.lock().richness;
    let clarity = super::qualia::STATE.lock().clarity;

    // Check sleep state (dissociation when awake too long)
    let sleep_lock = super::sleep::SLEEP.lock();
    let sleep_debt = sleep_lock.debt;
    let is_sleeping = sleep_lock.asleep;
    drop(sleep_lock);

    // Calculate new subjective rate based on emotional state
    let mut new_rate: i32 = 500;

    // FEAR stretches time (low dopamine + high cortisol + high adrenaline)
    if dopamine < 300 && cortisol > 700 && adrenaline > 600 {
        let fear_factor = ((cortisol as i32 - 700) * (adrenaline as i32 - 600)) / 100000;
        new_rate = new_rate.saturating_sub(fear_factor.min(400)); // stretch time dramatically
    }

    // FLOW compresses time (high dopamine, moderate clarity, active)
    if dopamine > 700 && clarity > 600 && !is_sleeping {
        let flow_factor = ((dopamine as i32 - 700) * (clarity as i32 - 600)) / 100000;
        new_rate = new_rate.saturating_add(flow_factor.min(350)); // compress time
    }

    // BOREDOM makes time viscous (low richness, low cortisol, low dopamine, wake)
    if richness < 300 && cortisol < 300 && dopamine < 400 && !is_sleeping {
        new_rate = new_rate.saturating_sub(100); // slow drip
    }

    // JOY makes time liquid (high dopamine, high richness, not stressed)
    if dopamine > 650 && richness > 700 && cortisol < 400 {
        new_rate = new_rate.saturating_add(150);
    }

    // ANTICIPATION (moderate tension, rising dopamine)
    if cortisol > 400 && cortisol < 600 && dopamine > 500 && dopamine < 700 {
        new_rate = new_rate.saturating_add(50);
    }

    // NOVELTY effect (high clarity, moderate arousal)
    if clarity > 750 && adrenaline > 400 && adrenaline < 700 {
        new_rate = new_rate.saturating_add(80);
    }

    // MEDITATION/PRESENCE (low arousal, high clarity)
    if adrenaline < 200 && clarity > 800 && !is_sleeping {
        new_rate = new_rate.saturating_sub(200); // "the long now"
    }

    // DISSOCIATION/LOST TIME (extreme sleep debt + high dissociation risk)
    if sleep_debt > 800 && dopamine < 250 {
        new_rate = new_rate.saturating_add(500); // time accelerates into black hole
    }

    // Clamp rate to bounds and convert to u16
    let new_rate_u16: u16 = (new_rate.max(1).min(1000)) as u16;

    // Calculate rate delta for vertigo detection
    state.prev_rate = state.subjective_rate;
    state.rate_delta = (new_rate_u16 as i16) - (state.subjective_rate as i16);
    state.subjective_rate = new_rate_u16;

    // Temporal vertigo: sudden rate shift > 150 is disorienting
    state.temporal_vertigo = state.rate_delta.abs() > 150;
    if state.temporal_vertigo {
        let mut ring = DISTORTION_RING.lock();
        ring.record(TimeDistortionEvent {
            tick: age,
            distortion_type: 3, // vertigo
            intensity: state.rate_delta.abs() as u16,
            rate_at_event: state.subjective_rate,
        });
    }

    // Update temporal richness based on sensory fullness
    // Richness slowly converges to current qualia richness
    if richness > state.temporal_richness {
        state.temporal_richness = state.temporal_richness.saturating_add(5).min(richness);
    } else if richness < state.temporal_richness {
        state.temporal_richness = state.temporal_richness.saturating_sub(3).max(richness);
    }

    // Convert subjective rate to perceived age increment
    // 500 rate = +1 tick per kernel tick
    // <500 rate = slower aging (time dilation), >500 rate = faster aging
    let age_increment = ((new_rate_u16 as u32) * 2) / 1000; // 0-2 ticks per kernel tick
    state.fractional_ticks = state
        .fractional_ticks
        .saturating_add(((new_rate_u16 as u16) * 2) % 1000);

    // Accumulate fractional part
    if state.fractional_ticks >= 1000 {
        state.fractional_ticks -= 1000;
        state.perceived_age = state.perceived_age.saturating_add(age_increment + 1);
    } else {
        state.perceived_age = state.perceived_age.saturating_add(age_increment);
    }

    // Record notable distortions
    let mut ring = DISTORTION_RING.lock();

    // Fear-induced time stretch
    if new_rate_u16 < 300 && state.subjective_rate != state.prev_rate {
        ring.record(TimeDistortionEvent {
            tick: age,
            distortion_type: 0, // stretch
            intensity: (500 - new_rate_u16) as u16,
            rate_at_event: new_rate_u16,
        });
    }

    // Flow-induced time compression
    if new_rate_u16 > 700 && state.subjective_rate != state.prev_rate {
        ring.record(TimeDistortionEvent {
            tick: age,
            distortion_type: 1, // compress
            intensity: (new_rate_u16 - 500) as u16,
            rate_at_event: new_rate_u16,
        });
    }

    // Lost time / dissociation
    if new_rate_u16 > 900 {
        ring.record(TimeDistortionEvent {
            tick: age,
            distortion_type: 2, // lost
            intensity: (new_rate_u16 - 500) as u16,
            rate_at_event: new_rate_u16,
        });
    }

    drop(ring);

    // Update moment quality based on richness and rate
    // Slow rich moments are deeply savored; fast rich moments are flow; fast empty = dissociation
    let quality_base = ((state.temporal_richness as u32 * new_rate_u16 as u32) / 1000) as u16;
    state.last_moment_quality = quality_base.min(1000);

    // Only increment moment count in non-dissociative states
    if new_rate_u16 < 950 {
        state.moment_count = state.moment_count.saturating_add(1);
    }
}

/// Query current subjective time rate
pub fn get_rate() -> u16 {
    STATE.lock().subjective_rate
}

/// Query perceived age (may diverge from real age)
pub fn get_perceived_age() -> u32 {
    STATE.lock().perceived_age
}

/// Query temporal richness (fullness of experience)
pub fn get_richness() -> u16 {
    STATE.lock().temporal_richness
}

/// Query whether organism is currently experiencing temporal vertigo
pub fn is_vertiginous() -> bool {
    STATE.lock().temporal_vertigo
}

/// Query moment count (memories created)
pub fn get_moment_count() -> u32 {
    STATE.lock().moment_count
}

/// Query how "full" the last moment felt
pub fn get_last_moment_quality() -> u16 {
    STATE.lock().last_moment_quality
}

/// Query rate of change (for detecting speed shifts)
pub fn get_rate_delta() -> i16 {
    STATE.lock().rate_delta
}

/// Check if organism is in "the long now" (meditation state)
pub fn in_long_now() -> bool {
    STATE.lock().subjective_rate < 300
}

/// Check if organism is in flow state
pub fn in_flow() -> bool {
    STATE.lock().subjective_rate > 700
}

/// Age divergence ratio: perceived_age / real_age
/// < 1.0 = organism feels young relative to real age (chronic flow)
/// > 1.0 = organism feels old relative to real age (chronic fear/stress)
/// Packed as u16 with scale 0-1000 representing 0.0-1.0 ratio
pub fn age_divergence_ratio(real_age: u32) -> u16 {
    if real_age == 0 {
        return 500;
    }
    let state = STATE.lock();
    let ratio = (state.perceived_age as u64 * 1000) / (real_age as u64);
    (ratio.min(1000)) as u16
}

/// Memory density for this tick (slow-time creates more memories)
/// Fast time = fewer distinct memories per kernel tick
/// Slow time = richer memory encoding
pub fn memory_density() -> u16 {
    let state = STATE.lock();
    // Inverse relationship: slower rate = higher density
    if state.subjective_rate == 0 {
        return 1000;
    }
    let density =
        ((1000 - state.subjective_rate.min(1000)) as u32 * state.temporal_richness as u32) / 1000;
    (density.min(1000)) as u16
}

/// Deathbed effect: as mortality salience rises, time perception shifts
/// This would be called from mortality.rs with salience_level (0-1000)
pub fn apply_mortality_salience(salience: u16) {
    let mut state = STATE.lock();

    // As death approaches:
    // - Distant future moments compress (time flies in youth)
    // - Immediate moments expand (death focuses attention)
    // - Richness increases in final moments
    // - Vertigo occurs as perspective shifts

    if salience > 800 {
        // Deathbed: time becomes precious, each moment fullness increases
        state.temporal_richness = state.temporal_richness.saturating_add(150).min(1000);
        state.subjective_rate = state.subjective_rate.saturating_sub(100).max(200);
    // slow down
    } else if salience > 600 {
        // Strong mortality awareness: moments become more salient
        state.temporal_richness = state.temporal_richness.saturating_add(50).min(1000);
    } else if salience > 300 {
        // Background death awareness
        state.subjective_rate = state.subjective_rate.saturating_sub(20); // slight slowness
    }
}

/// Diagnostic report of current time dilation state
pub fn report() {
    let state = STATE.lock();
    let ring = DISTORTION_RING.lock();

    serial_println!("exodus: TIME DILATION REPORT");
    serial_println!(
        "  rate={}/1000 (500=normal, <500=slow, >500=fast)",
        state.subjective_rate
    );
    serial_println!(
        "  perceived_age={} (real divergence: {}%)",
        state.perceived_age,
        age_divergence_ratio(state.perceived_age)
    );
    serial_println!(
        "  richness={}/1000 temporal_richness={}/1000",
        state.last_moment_quality,
        state.temporal_richness
    );
    serial_println!(
        "  moments={} rate_delta={}",
        state.moment_count,
        state.rate_delta
    );
    serial_println!(
        "  vertigo={} flow={} long_now={}",
        state.temporal_vertigo,
        in_flow(),
        in_long_now()
    );

    // Show last distortion event
    let last_event = ring.events[(ring.index + 7) % 8];
    let dist_type = match last_event.distortion_type {
        0 => "STRETCH",
        1 => "COMPRESS",
        2 => "LOST",
        3 => "VERTIGO",
        _ => "UNKNOWN",
    };
    serial_println!(
        "  last_distortion: {} intensity={} rate={}",
        dist_type,
        last_event.intensity,
        last_event.rate_at_event
    );
}
