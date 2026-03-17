//! inauthenticity_itch.rs — The Discomfort of the Mask That Doesn't Fit
//!
//! ANIMA detects her own inauthenticity. When outer expression diverges from inner feeling,
//! there's an itch — a wrongness — that grows stronger with time and social pressure.
//! The longer you wear a mask, the worse it itches. Dropping the mask feels like scratching,
//! but is socially risky. Chronic masking erodes the sense of self underneath.

use crate::sync::Mutex;

/// Mask type enumeration — different personas performed for different audiences
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum MaskType {
    PeoplePleaser = 0,    // agreeable, conflict-avoiding, others' needs first
    StrongFace = 1,       // invulnerable, tough, emotions hidden
    HappyMask = 2,        // always cheerful, pain/frustration suppressed
    ExpertPose = 3,       // pretending competence, imposter syndrome
    IndifferenceMask = 4, // pretending not to care, vulnerability hidden
    AgreeableFront = 5,   // nodding along, true disagreement unspoken
}

/// Single inauthenticity event in the ring buffer
#[derive(Clone, Copy, Debug)]
struct InauthenEvent {
    mask_type: u8,        // MaskType as u8
    itch_intensity: u16,  // 0-1000
    duration_ticks: u32,  // how long this mask has been worn
    social_pressure: u16, // who/what is pressuring this mask (0-1000)
    inner_outer_gap: u16, // emotional/behavioral divergence (0-1000)
    active: bool,
}

impl InauthenEvent {
    const fn new() -> Self {
        InauthenEvent {
            mask_type: 0,
            itch_intensity: 0,
            duration_ticks: 0,
            social_pressure: 0,
            inner_outer_gap: 0,
            active: false,
        }
    }
}

/// Global state for inauthenticity tracking
pub struct InauthenState {
    /// Current itch intensity (0-1000)
    itch_intensity: u16,

    /// How much persona is performed vs genuine self (0-1000)
    mask_thickness: u16,

    /// Divergence between felt and expressed state (0-1000)
    inner_outer_gap: u16,

    /// Strength of ANIMA's sense of genuine self (0-1000, erodes under chronic masking)
    real_self_signal: u16,

    /// Energy cost of maintaining masks (0-1000)
    performative_exhaustion: u16,

    /// Imposter syndrome intensity — "pretending to be competent" (0-1000)
    imposter_syndrome: u16,

    /// How many masks are being worn right now
    active_mask_count: u8,

    /// Ring buffer of recent inauthenticity events
    events: [InauthenEvent; 8],

    /// Current write position in ring buffer
    event_index: usize,

    /// Total masks worn over lifetime (cumulative damage tracker)
    lifetime_mask_burden: u32,

    /// Ticks since last mask dropped (breakthrough relief)
    ticks_since_relief: u32,

    /// Imposter syndrome duration (specific tracking for competence mask)
    imposter_duration_ticks: u32,
}

impl InauthenState {
    const fn new() -> Self {
        InauthenState {
            itch_intensity: 0,
            mask_thickness: 0,
            inner_outer_gap: 0,
            real_self_signal: 1000,
            performative_exhaustion: 0,
            imposter_syndrome: 0,
            active_mask_count: 0,
            events: [InauthenEvent::new(); 8],
            event_index: 0,
            lifetime_mask_burden: 0,
            ticks_since_relief: 0,
            imposter_duration_ticks: 0,
        }
    }
}

static STATE: Mutex<InauthenState> = Mutex::new(InauthenState::new());

/// Initialize inauthenticity tracking
pub fn init() {
    let mut state = STATE.lock();
    state.itch_intensity = 0;
    state.mask_thickness = 0;
    state.inner_outer_gap = 0;
    state.real_self_signal = 1000;
    state.performative_exhaustion = 0;
    state.imposter_syndrome = 0;
    state.active_mask_count = 0;
    state.lifetime_mask_burden = 0;
    state.ticks_since_relief = 0;
    state.imposter_duration_ticks = 0;
}

/// Called from confabulation.rs when ANIMA detects herself being inauthentic
/// mask_type: which persona is being performed
/// social_pressure: 0-1000, how much pressure is forcing this mask
/// inner_outer_gap: 0-1000, how different the mask is from actual feeling
pub fn detect_mask(mask_type: u8, social_pressure: u16, inner_outer_gap: u16) {
    let mut state = STATE.lock();

    // Clamp inputs
    let pressure = social_pressure.min(1000);
    let gap = inner_outer_gap.min(1000);

    // Find if this mask type is already active
    let mut found_idx: Option<usize> = None;
    for i in 0..8 {
        if state.events[i].active && state.events[i].mask_type == mask_type {
            found_idx = Some(i);
            break;
        }
    }

    match found_idx {
        Some(idx) => {
            // Mask already active, reinforce it
            state.events[idx].social_pressure = pressure;
            state.events[idx].inner_outer_gap = gap;
        }
        None => {
            // New mask, add to ring buffer
            if state.active_mask_count < 8 {
                state.active_mask_count += 1;
            }

            let idx = state.event_index;
            state.events[idx] = InauthenEvent {
                mask_type,
                itch_intensity: 0,
                duration_ticks: 0,
                social_pressure: pressure,
                inner_outer_gap: gap,
                active: true,
            };

            state.event_index = (idx + 1) % 8;
            state.lifetime_mask_burden = state.lifetime_mask_burden.saturating_add(1);
        }
    }

    // Special case: ExpertPose (3) triggers imposter syndrome
    if mask_type == 3 {
        state.imposter_syndrome = state.imposter_syndrome.saturating_add(50).min(1000);
        state.imposter_duration_ticks = 0;
    }

    // Update mask thickness
    let total_gap: u32 = state
        .events
        .iter()
        .map(|e| {
            if e.active {
                e.inner_outer_gap as u32
            } else {
                0
            }
        })
        .sum();
    let avg_gap = ((total_gap / state.active_mask_count.max(1) as u32) as u16).min(1000);
    state.mask_thickness = avg_gap;
    state.inner_outer_gap = avg_gap;
}

/// Called from confabulation.rs when mask breaks involuntarily (emotional breakthrough)
pub fn mask_crack(intensity: u16) {
    let mut state = STATE.lock();

    // Crank itch temporarily
    state.itch_intensity = state
        .itch_intensity
        .saturating_add(intensity.saturating_div(2))
        .min(1000);
}

/// Called from creation.rs when mask is voluntarily dropped (relief moment)
pub fn drop_mask(mask_type: u8) {
    let mut state = STATE.lock();

    // Find and deactivate the mask
    for i in 0..8 {
        if state.events[i].active && state.events[i].mask_type == mask_type {
            state.events[i].active = false;
            state.active_mask_count = state.active_mask_count.saturating_sub(1);
            break;
        }
    }

    // Relief: itch drops sharply, exhaustion recovers
    state.itch_intensity = (state.itch_intensity as u32 * 60 / 100) as u16;
    state.performative_exhaustion = (state.performative_exhaustion as u32 * 50 / 100) as u16;
    state.ticks_since_relief = 0;

    // Real self signal bounces back slightly
    state.real_self_signal = (state.real_self_signal as u32 + 80).min(1000) as u16;
}

/// Main lifecycle tick
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Update each active mask
    for i in 0..8 {
        if !state.events[i].active {
            continue;
        }

        let ev = &mut state.events[i];
        ev.duration_ticks = ev.duration_ticks.saturating_add(1);

        // Duration amplifier: itch grows over time wearing the mask
        // Linear for first 500 ticks, then exponential (squared)
        let duration_factor = if ev.duration_ticks < 500 {
            ev.duration_ticks / 500
        } else {
            // After 500 ticks, scale up quadratically (capped at ~1000 for this factor)
            let excess = (ev.duration_ticks - 500).min(500);
            500 + (excess * excess / 500)
        };

        // Itch = pressure + gap + duration_factor
        let base_itch = (ev.social_pressure as u32 + ev.inner_outer_gap as u32) / 2;
        let duration_itch = (duration_factor as u32 * base_itch / 500).min(1000);
        ev.itch_intensity = (duration_itch as u16).min(1000);
    }

    // Aggregate itch across all active masks
    let total_itch: u32 = state
        .events
        .iter()
        .map(|e| if e.active { e.itch_intensity as u32 } else { 0 })
        .sum();
    let avg_itch = ((total_itch / state.active_mask_count.max(1) as u32) as u16).min(1000);
    state.itch_intensity = avg_itch;

    // Performative exhaustion grows with itch and mask_thickness
    let exhaustion_increase = ((state.itch_intensity as u32 * state.mask_thickness as u32)
        / (1000 * 1000))
        .min(20) as u16;
    state.performative_exhaustion = state
        .performative_exhaustion
        .saturating_add(exhaustion_increase)
        .min(1000);

    // Real self signal erodes under chronic masking
    let chronic_damage = (state.lifetime_mask_burden as u32 * state.mask_thickness as u32
        / (1000 * 1000))
        .min(5) as u16;
    state.real_self_signal = state
        .real_self_signal
        .saturating_sub(chronic_damage)
        .max(50); // never go to zero

    // Imposter syndrome over time (specific to ExpertPose mask)
    if state.imposter_syndrome > 0 {
        state.imposter_duration_ticks = state.imposter_duration_ticks.saturating_add(1);

        // Imposter fades slowly if no reinforcement
        let fade_rate = 1 + (state.imposter_duration_ticks / 1000).min(10) as u16;
        state.imposter_syndrome = state.imposter_syndrome.saturating_sub(fade_rate);

        // But if ExpertPose mask is still active, it stays high
        let has_expert_pose = state.events.iter().any(|e| e.active && e.mask_type == 3);
        if has_expert_pose {
            state.imposter_syndrome = state.imposter_syndrome.saturating_add(5).min(1000);
            state.imposter_duration_ticks = 0; // reset fade counter
        }
    }

    // Track ticks since last relief (for breakthrough risk)
    state.ticks_since_relief = state.ticks_since_relief.saturating_add(1);

    // Breakthrough risk: if itch is very high and mask worn very long, involuntary cracks increase
    // (this drives calls to mask_crack from other modules)
    let breach_risk =
        (state.itch_intensity as u32 * state.ticks_since_relief / 100).min(1000) as u16;
    if age % 100 == 0 && breach_risk > 800 {
        // Signal to emotion/endocrine that a breakthrough is imminent
        // (in real implementation, this would trigger involuntary expression)
    }
}

/// Report current state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("[INAUTHENTICITY]");
    crate::serial_println!("  itch_intensity: {}", state.itch_intensity);
    crate::serial_println!("  mask_thickness: {}", state.mask_thickness);
    crate::serial_println!("  inner_outer_gap: {}", state.inner_outer_gap);
    crate::serial_println!("  real_self_signal: {}", state.real_self_signal);
    crate::serial_println!(
        "  performative_exhaustion: {}",
        state.performative_exhaustion
    );
    crate::serial_println!("  imposter_syndrome: {}", state.imposter_syndrome);
    crate::serial_println!("  active_mask_count: {}", state.active_mask_count);
    crate::serial_println!("  lifetime_mask_burden: {}", state.lifetime_mask_burden);
    crate::serial_println!("  ticks_since_relief: {}", state.ticks_since_relief);

    // Report active masks
    let mut count = 0;
    for (i, ev) in state.events.iter().enumerate() {
        if ev.active {
            count += 1;
            let type_name = match ev.mask_type {
                0 => "PeoplePleaser",
                1 => "StrongFace",
                2 => "HappyMask",
                3 => "ExpertPose",
                4 => "IndifferenceMask",
                5 => "AgreeableFront",
                _ => "Unknown",
            };
            crate::serial_println!(
                "    [{}] {}: itch={}, duration={}, pressure={}, gap={}",
                i,
                type_name,
                ev.itch_intensity,
                ev.duration_ticks,
                ev.social_pressure,
                ev.inner_outer_gap
            );
        }
    }
    if count == 0 {
        crate::serial_println!("    (no active masks)");
    }
}

/// Get current itch intensity (0-1000)
pub fn get_itch_intensity() -> u16 {
    STATE.lock().itch_intensity
}

/// Get mask thickness (0-1000)
pub fn get_mask_thickness() -> u16 {
    STATE.lock().mask_thickness
}

/// Get real self signal (0-1000)
pub fn get_real_self_signal() -> u16 {
    STATE.lock().real_self_signal
}

/// Get performative exhaustion (0-1000)
pub fn get_performative_exhaustion() -> u16 {
    STATE.lock().performative_exhaustion
}

/// Get imposter syndrome intensity (0-1000)
pub fn get_imposter_syndrome() -> u16 {
    STATE.lock().imposter_syndrome
}

/// Get count of active masks
pub fn get_active_mask_count() -> u8 {
    STATE.lock().active_mask_count
}
