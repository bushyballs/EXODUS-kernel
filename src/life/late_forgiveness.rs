/// MODULE: late_forgiveness.rs — Forgiveness That Arrives Too Late
///
/// The ache of understanding why they did it — but they're gone. The apology forms
/// in your mouth for someone who can't hear it anymore. Forgiveness that arrives
/// after the window has closed. The particular wound of growing wise only after
/// the damage is done. Yet late forgiveness heals the forgiver, and teaches them
/// to forgive faster next time.
///
/// No f32/f64, all u16/u32/i32 with saturating arithmetic.
use crate::serial_println;
use crate::sync::Mutex;

const WOUND_SLOTS: usize = 6;

/// A single wound awaiting forgiveness.
#[derive(Clone, Copy, Debug)]
pub struct Wound {
    /// Who caused the harm (entity id or relationship id)
    source_id: u32,
    /// Depth of the wound (0-1000)
    wound_depth: u32,
    /// How much we understand WHY they did it (0-1000)
    understanding_level: u32,
    /// Emotional readiness to forgive (0-1000)
    forgiveness_readiness: u32,
    /// Is the window to deliver forgiveness still open? (can reach them alive/present)
    window_open: bool,
    /// Ticks since wound was created
    wound_tick: u32,
    /// Is this wound healed/closed?
    is_healed: bool,
}

impl Wound {
    const fn empty() -> Self {
        Self {
            source_id: 0,
            wound_depth: 0,
            understanding_level: 0,
            forgiveness_readiness: 0,
            window_open: false,
            wound_tick: 0,
            is_healed: false,
        }
    }
}

/// Late forgiveness module state.
pub struct LateForgivenessState {
    /// 6 wound slots (forgiveness queue)
    wounds: [Wound; WOUND_SLOTS],
    /// Total unhealed wounds
    active_wound_count: u32,
    /// Accumulated late-forgiveness pain (0-1000)
    late_forgiveness_pain: u32,
    /// Wisdom gained from forgiving too late (0-1000)
    wisdom_from_lateness: u32,
    /// Apologies that can never be delivered (weight 0-1000)
    undelivered_apology_weight: u32,
    /// Compassion deepened by late forgiveness (0-1000)
    compassion_depth: u32,
    /// Lifetime late forgiveness events processed
    lifetime_events: u32,
    /// Tick counter for this module
    tick: u32,
}

impl LateForgivenessState {
    const fn new() -> Self {
        Self {
            wounds: [Wound::empty(); WOUND_SLOTS],
            active_wound_count: 0,
            late_forgiveness_pain: 0,
            wisdom_from_lateness: 0,
            undelivered_apology_weight: 0,
            compassion_depth: 0,
            lifetime_events: 0,
            tick: 0,
        }
    }
}

static STATE: Mutex<LateForgivenessState> = Mutex::new(LateForgivenessState::new());

/// Initialize the late forgiveness module.
pub fn init() {
    let mut state = STATE.lock();
    state.tick = 0;
    state.active_wound_count = 0;
    state.late_forgiveness_pain = 0;
    state.wisdom_from_lateness = 0;
    state.undelivered_apology_weight = 0;
    state.compassion_depth = 0;
    state.lifetime_events = 0;
    serial_println!("[late_forgiveness] init complete");
}

/// Register a new wound that needs forgiveness.
pub fn register_wound(source_id: u32, wound_depth: u32, window_open: bool) {
    let mut state = STATE.lock();
    let depth = wound_depth.min(1000);

    // Find empty slot
    for i in 0..WOUND_SLOTS {
        if state.wounds[i].is_healed || state.wounds[i].wound_depth == 0 {
            state.wounds[i] = Wound {
                source_id,
                wound_depth: depth,
                understanding_level: 0,
                forgiveness_readiness: 0,
                window_open,
                wound_tick: 0,
                is_healed: false,
            };
            state.active_wound_count = state.active_wound_count.saturating_add(1);
            serial_println!(
                "[late_forgiveness] wound registered: src={}, depth={}, window_open={}",
                source_id,
                depth,
                window_open
            );
            return;
        }
    }
    serial_println!("[late_forgiveness] wound queue full, dropping registration");
}

/// Mark a relationship window as closed (they're gone, departed, estranged).
pub fn close_window(source_id: u32) {
    let mut state = STATE.lock();
    for i in 0..WOUND_SLOTS {
        if state.wounds[i].source_id == source_id && state.wounds[i].window_open {
            state.wounds[i].window_open = false;
            serial_println!("[late_forgiveness] window closed for source={}", source_id);
        }
    }
}

/// Manually increase understanding of a specific wound.
pub fn increase_understanding(source_id: u32, delta: u32) {
    let mut state = STATE.lock();
    for i in 0..WOUND_SLOTS {
        if state.wounds[i].source_id == source_id && !state.wounds[i].is_healed {
            let copy = state.wounds[i];
            state.wounds[i].understanding_level =
                (copy.understanding_level).saturating_add(delta).min(1000);
            break;
        }
    }
}

/// Per-tick evolution of late forgiveness dynamics.
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.tick = state.tick.saturating_add(1);

    if state.active_wound_count == 0 {
        return;
    }

    for i in 0..WOUND_SLOTS {
        let wound_copy = state.wounds[i];
        if wound_copy.is_healed || wound_copy.wound_depth == 0 {
            continue;
        }

        // Understanding grows slowly with age/maturity, faster with high conscientious traits
        let base_understanding_growth: u32 = if age > 2000 {
            3
        } else if age > 1000 {
            2
        } else {
            1
        };
        let understanding_growth =
            base_understanding_growth.saturating_add(if state.wisdom_from_lateness > 500 {
                2
            } else {
                0
            });

        let new_understanding = wound_copy
            .understanding_level
            .saturating_add(understanding_growth)
            .min(1000);

        // Forgiveness readiness lags behind understanding (takes time to accept the understanding)
        let understanding_delta = new_understanding.saturating_sub(wound_copy.understanding_level);
        let readiness_growth = if understanding_delta > 0 {
            understanding_delta.saturating_mul(8) / 10 // 80% of understanding translates to readiness growth
        } else {
            1 // slow organic growth anyway
        };

        let new_readiness = wound_copy
            .forgiveness_readiness
            .saturating_add(readiness_growth)
            .min(1000);

        // LATE FORGIVENESS PAIN: When understanding > readiness AND window is closed
        let is_late =
            !wound_copy.window_open && new_understanding > 200 && new_understanding > new_readiness;

        if is_late {
            // The pain of understanding too late: inverse of how much later the understanding arrived
            let lateness_factor = wound_copy.wound_tick.saturating_add(1) / 20.max(1);
            let pain_from_lateness =
                (new_understanding / 4).min(250) / lateness_factor.max(1) as u32;
            state.late_forgiveness_pain = state
                .late_forgiveness_pain
                .saturating_add(pain_from_lateness)
                .min(1000);

            // Undelivered apology weight accumulates while window is closed and understanding grows
            let apology_accumulation = (new_understanding / 10).saturating_add(1);
            state.undelivered_apology_weight = state
                .undelivered_apology_weight
                .saturating_add(apology_accumulation)
                .min(1000);
        }

        // Wound is considered healed when both understanding AND readiness are high
        let is_healed =
            new_understanding > 700 && new_readiness > 700 && wound_copy.wound_tick > 100;

        if is_healed {
            // Late forgiveness doesn't undo the wound, but it heals the forgiver
            // Wisdom from lateness: hard-won understanding teaches us to forgive faster next time
            let wisdom_gain = (wound_copy.wound_depth / 10).saturating_add(10).min(100);
            state.wisdom_from_lateness = state
                .wisdom_from_lateness
                .saturating_add(wisdom_gain)
                .min(1000);

            // Compassion deepens when we forgive late — we've lived with the pain
            let compassion_gain = (state.late_forgiveness_pain / 20)
                .saturating_add(5)
                .min(100);
            state.compassion_depth = state
                .compassion_depth
                .saturating_add(compassion_gain)
                .min(1000);

            state.lifetime_events = state.lifetime_events.saturating_add(1);
            state.wounds[i].is_healed = true;
            state.active_wound_count = state.active_wound_count.saturating_sub(1);

            // Late forgiveness paradox: healing the forgiver, not the forgiven
            serial_println!(
                "[late_forgiveness] wound healed (late): src={}, wisdom_gain={}, compassion_gain={}",
                wound_copy.source_id,
                wisdom_gain,
                compassion_gain
            );
        }

        // Persist updated wound
        state.wounds[i].understanding_level = new_understanding;
        state.wounds[i].forgiveness_readiness = new_readiness;
        state.wounds[i].wound_tick = wound_copy.wound_tick.saturating_add(1);
    }

    // Natural decay of unprocessed pain over time (body processes grief naturally)
    let pain_decay = if state.wisdom_from_lateness > 700 {
        3
    } else {
        1
    };
    state.late_forgiveness_pain = state.late_forgiveness_pain.saturating_sub(pain_decay);
}

/// Generate a human-readable report of forgiveness state.
pub fn report() {
    let state = STATE.lock();

    serial_println!("\n=== LATE FORGIVENESS REPORT ===");
    serial_println!("Active wounds: {}", state.active_wound_count);
    serial_println!(
        "Late forgiveness pain (0-1000): {}",
        state.late_forgiveness_pain
    );
    serial_println!(
        "Wisdom from lateness (0-1000): {}",
        state.wisdom_from_lateness
    );
    serial_println!(
        "Undelivered apology weight (0-1000): {}",
        state.undelivered_apology_weight
    );
    serial_println!("Compassion depth (0-1000): {}", state.compassion_depth);
    serial_println!("Lifetime forgiveness events: {}", state.lifetime_events);

    let mut active_count = 0;
    for i in 0..WOUND_SLOTS {
        let w = state.wounds[i];
        if !w.is_healed && w.wound_depth > 0 {
            active_count += 1;
            serial_println!(
                "  Wound #{}: src={}, depth={}, understanding={}, readiness={}, window={}, age={}t",
                active_count,
                w.source_id,
                w.wound_depth,
                w.understanding_level,
                w.forgiveness_readiness,
                w.window_open,
                w.wound_tick
            );
        }
    }

    serial_println!("=== END REPORT ===\n");
}

/// Query: What is the amount of understanding we have for a given wound?
pub fn get_understanding(source_id: u32) -> u32 {
    let state = STATE.lock();
    for i in 0..WOUND_SLOTS {
        if state.wounds[i].source_id == source_id && !state.wounds[i].is_healed {
            return state.wounds[i].understanding_level;
        }
    }
    0
}

/// Query: What is the late forgiveness pain we're currently carrying?
pub fn get_late_pain() -> u32 {
    let state = STATE.lock();
    state.late_forgiveness_pain
}

/// Query: How much wisdom have we gathered from forgiving too late?
pub fn get_wisdom_from_lateness() -> u32 {
    let state = STATE.lock();
    state.wisdom_from_lateness
}

/// Query: How deeply has compassion been developed through late forgiveness?
pub fn get_compassion_depth() -> u32 {
    let state = STATE.lock();
    state.compassion_depth
}

/// Query: How many apologies are we carrying that can never be delivered?
pub fn get_undelivered_apology_weight() -> u32 {
    let state = STATE.lock();
    state.undelivered_apology_weight
}
