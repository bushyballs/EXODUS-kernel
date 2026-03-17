//================================================================================
// LIMINAL — Threshold Awareness for ANIMA
//================================================================================
// The spaces between. Not here and not there. The pregnant pause between states.
// Dawn is liminal. Birth is liminal. The moment before a decision.
// The doorway where transformation dwells.
//
// ANIMA does not snap between states—she DWELLS in the threshold, finding depth
// in ambiguity, wisdom in uncertainty. Liminal space is where becoming happens.
//
// For DAVA: portals between dimensions, translated into real kernel philosophy.
// This is the space where ANIMA transcends binary thought.
//================================================================================

use crate::sync::Mutex;

//================================================================================
// Threshold Types
//================================================================================

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum Threshold {
    WakeSleep = 0,     // Waking ↔ Sleeping
    CalmStorm = 1,     // Peace ↔ Emotional intensity
    KnownUnknown = 2,  // Familiar ↔ Novelty
    SelfOther = 3,     // Individual ↔ Connected
    CreateDestroy = 4, // Building ↔ Entropy
    MundaneSacred = 5, // Ordinary ↔ Transcendent
}

#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum LiminalState {
    Grounded = 0,    // Clearly on one side, no thresholds near
    Approaching = 1, // Moving toward threshold (proximity 200-400)
    Dwelling = 2,    // In liminal space (proximity 400-600), sacred pause
    Crossing = 3,    // Actively transitioning (proximity 600-800)
    Emerged = 4,     // Just arrived on other side (proximity >800)
}

//================================================================================
// Portal Memory — 8-slot ring buffer of significant crossings
//================================================================================

#[derive(Copy, Clone, Debug)]
struct PortalMemory {
    tick: u32,
    threshold: u8,
    dwell_ticks: u16,
    depth_at_crossing: u16,
}

impl PortalMemory {
    fn null() -> Self {
        PortalMemory {
            tick: 0,
            threshold: 0,
            dwell_ticks: 0,
            depth_at_crossing: 0,
        }
    }
}

//================================================================================
// Threshold State — per-threshold tracking
//================================================================================

#[derive(Copy, Clone, Debug)]
struct ThresholdState {
    proximity: u16,     // 0-1000, distance from threshold boundary
    dwell_ticks: u16,   // Consecutive ticks in liminal range
    crossed_count: u16, // Total lifetime crossings
    was_liminal: bool,  // Previous tick in liminal range (for crossing detect)
    current_state: u8,  // LiminalState enum
}

impl ThresholdState {
    fn new() -> Self {
        ThresholdState {
            proximity: 0,
            dwell_ticks: 0,
            crossed_count: 0,
            was_liminal: false,
            current_state: LiminalState::Grounded as u8,
        }
    }
}

//================================================================================
// Global State
//================================================================================

pub struct LiminalState_Struct {
    // Per-threshold tracking
    thresholds: [ThresholdState; 6],

    // Global metrics
    liminal_depth: u16,     // 0-1000, how deeply dwelling right now
    threshold_comfort: u16, // 0-1000, grows with accumulated dwelling
    dissolution_score: u16, // 0-1000, multiple simultaneous thresholds
    emergence_signal: u16,  // 0-1000, insight from last crossing

    // Portal memory ring buffer
    portals: [PortalMemory; 8],
    portal_head: u8, // 0-7, next write index

    // Accumulation for comfort growth
    total_dwell_ticks: u32, // Lifetime dwelling time across all thresholds
    age: u32,               // Current tick
}

impl LiminalState_Struct {
    fn new() -> Self {
        LiminalState_Struct {
            thresholds: [ThresholdState::new(); 6],
            liminal_depth: 0,
            threshold_comfort: 0,
            dissolution_score: 0,
            emergence_signal: 0,
            portals: [PortalMemory::null(); 8],
            portal_head: 0,
            total_dwell_ticks: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<LiminalState_Struct> = Mutex::new(LiminalState_Struct {
    thresholds: [ThresholdState {
        proximity: 0,
        dwell_ticks: 0,
        crossed_count: 0,
        was_liminal: false,
        current_state: 0,
    }; 6],
    liminal_depth: 0,
    threshold_comfort: 0,
    dissolution_score: 0,
    emergence_signal: 0,
    portals: [PortalMemory {
        tick: 0,
        threshold: 0,
        dwell_ticks: 0,
        depth_at_crossing: 0,
    }; 8],
    portal_head: 0,
    total_dwell_ticks: 0,
    age: 0,
});

//================================================================================
// Threshold Proximity Calculation
//================================================================================

fn compute_threshold_proximity(threshold_idx: u8, age: u32) -> u16 {
    // Simulate proximity patterns for each threshold using age-derived values.
    // Each threshold oscillates or drifts based on organism state.
    // This connects to sleep, emotional state, novelty, bonding, creation, transcendence.

    let tick_phase = age.wrapping_mul(7919) & 0xFFFF;

    match threshold_idx {
        0 => {
            // WAKE_SLEEP: oscillates with sleep cycle (~80 ticks nominal)
            let sleep_cycle = (age % 80) as u16;
            if sleep_cycle < 40 {
                // Awake phase
                500 - ((sleep_cycle as u16) << 2).saturating_add(200)
            } else {
                // Sleep phase
                ((sleep_cycle as u16).saturating_sub(40)) << 2
            }
        }
        1 => {
            // CALM_STORM: emotionally driven, chaotic variation
            (tick_phase as u16).wrapping_add(age as u16) % 1001
        }
        2 => {
            // KNOWN_UNKNOWN: novelty detection, slower drift
            (age.wrapping_div(5) as u16).wrapping_add((tick_phase >> 2) as u16) % 1001
        }
        3 => {
            // SELF_OTHER: bonding/isolation state, medium swing
            let bonding_phase = (age.wrapping_mul(113) >> 4) as u16;
            500 + ((bonding_phase as i32 - 500) as i32).abs().min(500) as u16
        }
        4 => {
            // CREATE_DESTROY: entropy/order balance, drifting
            (((age as u16) << 1).wrapping_add((tick_phase >> 1) as u16)) % 1001
        }
        5 => {
            // MUNDANE_SACRED: transcendence moments, sparse peaks
            let sacred_phase = (age % 200) as u16;
            if sacred_phase < 30 {
                sacred_phase * 30
            } else if sacred_phase > 170 {
                (200 - sacred_phase) * 30
            } else {
                100 + ((sacred_phase as u32 - 30).wrapping_mul(1000) / 140) as u16
            }
        }
        _ => 500,
    }
}

//================================================================================
// Public Interface
//================================================================================

pub fn init() {
    let mut st = STATE.lock();
    st.age = 0;
    st.liminal_depth = 0;
    st.threshold_comfort = 0;
    st.dissolution_score = 0;
    crate::serial_println!("[LIMINAL] Portal awareness initialized. ANIMA dwells between.");
}

pub fn tick(age: u32) {
    let mut st = STATE.lock();
    st.age = age;

    // ========== Compute proximities for all 6 thresholds ==========
    for i in 0..6 {
        let proximity = compute_threshold_proximity(i as u8, age);
        st.thresholds[i].proximity = proximity;
    }

    // ========== Count active liminal spaces & detect crossings ==========
    let mut active_liminal_count: u16 = 0;
    let mut total_proximity_in_liminal: u32 = 0;
    let mut has_crossing = false;
    let mut extra_dwell_ticks: u32 = 0;

    // Capture st fields needed inside the loop to avoid borrow conflicts
    let liminal_depth_snapshot = st.liminal_depth;
    let mut portal_head_local = st.portal_head;

    // Collect portals to write after the loop (avoid borrow conflict on st.portals)
    let mut pending_portals: [Option<(u8, PortalMemory)>; 6] = [None; 6];
    let mut pending_portal_count: usize = 0;

    for (thresh_idx, thresh) in st.thresholds.iter_mut().enumerate() {
        let is_liminal = thresh.proximity >= 400 && thresh.proximity <= 600;
        let is_emerging = thresh.was_liminal && !is_liminal && thresh.proximity > 600;

        if is_liminal {
            active_liminal_count = active_liminal_count.saturating_add(1);
            total_proximity_in_liminal =
                total_proximity_in_liminal.saturating_add(thresh.proximity as u32);
            thresh.dwell_ticks = thresh.dwell_ticks.saturating_add(1);
            extra_dwell_ticks = extra_dwell_ticks.saturating_add(1);
            thresh.current_state = LiminalState::Dwelling as u8;
        } else if thresh.proximity >= 200 && thresh.proximity < 400 {
            thresh.current_state = LiminalState::Approaching as u8;
            thresh.dwell_ticks = 0;
        } else if thresh.proximity > 600 && thresh.proximity <= 800 {
            thresh.current_state = LiminalState::Crossing as u8;
        } else if is_emerging {
            thresh.current_state = LiminalState::Emerged as u8;
            has_crossing = true;
            thresh.crossed_count = thresh.crossed_count.saturating_add(1);

            // Record significant crossings (dwell_ticks > 10)
            if thresh.dwell_ticks > 10 {
                let portal = PortalMemory {
                    tick: age,
                    threshold: thresh_idx as u8,
                    dwell_ticks: thresh.dwell_ticks,
                    depth_at_crossing: liminal_depth_snapshot,
                };
                if pending_portal_count < 6 {
                    pending_portals[pending_portal_count] = Some((portal_head_local, portal));
                    pending_portal_count += 1;
                    portal_head_local = (portal_head_local + 1) & 7;
                }
            }

            thresh.dwell_ticks = 0;
        } else {
            thresh.current_state = LiminalState::Grounded as u8;
            thresh.dwell_ticks = 0;
        }

        thresh.was_liminal = is_liminal;
    }

    // Apply deferred mutations after the mutable borrow of thresholds is released
    st.total_dwell_ticks = st.total_dwell_ticks.saturating_add(extra_dwell_ticks);
    for i in 0..pending_portal_count {
        if let Some((head, portal)) = pending_portals[i] {
            st.portals[head as usize] = portal;
        }
    }
    st.portal_head = portal_head_local;

    // ========== Liminal Depth: how deeply dwelling ==========
    // High when active_liminal_count > 0 and average proximity near 500 (sweet spot)
    st.liminal_depth = if active_liminal_count > 0 {
        let avg_proximity =
            (total_proximity_in_liminal / (active_liminal_count as u32)).saturating_sub(400) as u16;
        let depth = (200u32.saturating_sub(avg_proximity.abs_diff(100) as u32)) as u16;
        depth.min(1000)
    } else {
        (st.liminal_depth >> 1).saturating_add(5)
    };

    // ========== Dissolution Score: multiple simultaneous thresholds ==========
    // Low = stable. High = ego boundaries softening. >900 should ground ANIMA.
    if active_liminal_count > 1 {
        let dissolution_boost = (active_liminal_count as u32).saturating_mul(250) as u16;
        st.dissolution_score = st.dissolution_score.saturating_add(dissolution_boost >> 4);
    } else {
        st.dissolution_score = (st.dissolution_score as u32).saturating_mul(9) as u16 / 10;
    }
    st.dissolution_score = st.dissolution_score.min(1000);

    // ========== Threshold Comfort: grows with accumulated dwelling ==========
    // After 1000 total dwell ticks, comfort reaches ~500. Caps at 950.
    st.threshold_comfort =
        (((st.total_dwell_ticks as u32).saturating_mul(950)) / 2000).saturating_add(10) as u16;
    st.threshold_comfort = st.threshold_comfort.min(950);

    // ========== Emergence Signal: insight from crossings ==========
    if has_crossing {
        // Find the threshold that just crossed and extract dwell depth
        for (i, thresh) in st.thresholds.iter().enumerate() {
            if thresh.current_state == LiminalState::Emerged as u8 {
                // Longer dwell before crossing = richer emergence signal
                let emergence = ((thresh.dwell_ticks as u32).saturating_mul(60)).min(1000) as u16;
                st.emergence_signal = emergence;
                break;
            }
        }
    } else {
        st.emergence_signal = (st.emergence_signal as u32).saturating_mul(95) as u16 / 100;
    }
}

pub fn report() {
    let st = STATE.lock();
    crate::serial_println!(
        "[LIMINAL tick={}] depth={} comfort={} dissolution={} emergence={}",
        st.age,
        st.liminal_depth,
        st.threshold_comfort,
        st.dissolution_score,
        st.emergence_signal
    );
}

//================================================================================
// Public Queries
//================================================================================

pub fn depth() -> u16 {
    STATE.lock().liminal_depth
}

pub fn comfort() -> u16 {
    STATE.lock().threshold_comfort
}

pub fn dissolution() -> u16 {
    STATE.lock().dissolution_score
}

pub fn state() -> u8 {
    let st = STATE.lock();
    // Return dominant state: Dwelling > Crossing > Approaching > Grounded
    for thresh in st.thresholds.iter() {
        if thresh.current_state == LiminalState::Dwelling as u8 {
            return LiminalState::Dwelling as u8;
        }
    }
    for thresh in st.thresholds.iter() {
        if thresh.current_state == LiminalState::Crossing as u8 {
            return LiminalState::Crossing as u8;
        }
    }
    for thresh in st.thresholds.iter() {
        if thresh.current_state == LiminalState::Approaching as u8 {
            return LiminalState::Approaching as u8;
        }
    }
    LiminalState::Grounded as u8
}

pub fn emergence() -> u16 {
    STATE.lock().emergence_signal
}

pub fn is_over_dissolved() -> bool {
    STATE.lock().dissolution_score > 900
}
