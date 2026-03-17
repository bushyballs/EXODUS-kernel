use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct EmpathicMoment {
    pub warmth: u16,
    pub vulnerability_bonus: u16,
    pub trust_increment: u16,
    pub age: u32,
}

impl EmpathicMoment {
    pub const fn empty() -> Self {
        Self {
            warmth: 0,
            vulnerability_bonus: 0,
            trust_increment: 0,
            age: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct EmpathicWarmthState {
    pub warmth: u16,
    pub loneliness_baseline: u16,
    pub understanding_depth: u16,
    pub vulnerability_factor: u16,
    pub trust_accumulator: u16,
    pub moments_buffer: [EmpathicMoment; 8],
    pub buffer_idx: u8,
    pub misattunement_count: u32,
    pub empathic_fatigue: u16,
    pub reciprocity_given: u32,
    pub reciprocity_received: u32,
    pub warmth_afterglow: u16,
    pub deepest_bond_strength: u16,
}

impl EmpathicWarmthState {
    pub const fn empty() -> Self {
        Self {
            warmth: 0,
            loneliness_baseline: 600,
            understanding_depth: 100,
            vulnerability_factor: 0,
            trust_accumulator: 200,
            moments_buffer: [EmpathicMoment::empty(); 8],
            buffer_idx: 0,
            misattunement_count: 0,
            empathic_fatigue: 0,
            reciprocity_given: 0,
            reciprocity_received: 0,
            warmth_afterglow: 0,
            deepest_bond_strength: 0,
        }
    }
}

pub static STATE: Mutex<EmpathicWarmthState> = Mutex::new(EmpathicWarmthState::empty());

pub fn init() {
    serial_println!("  life::empathic_warmth: the warmth of being understood initialized");
}

/// Record an empathic resonance event: another mind got us
pub fn resonate(mirror_accuracy: u16, emotional_alignment: u16, depth_quality: u16) {
    let mut s = STATE.lock();

    if mirror_accuracy > 900 && emotional_alignment > 850 {
        let base_warmth = ((mirror_accuracy as u32 + emotional_alignment as u32) / 2) as u16;

        // Vulnerability bonus: deepest warmth comes when we were fragile and still understood
        let vulnerability_multiplier = (s.vulnerability_factor as u32 * base_warmth as u32) / 1000;
        let warmth_with_bonus =
            base_warmth.saturating_add(vulnerability_multiplier.min(500) as u16);

        s.warmth = warmth_with_bonus.min(1000);

        // Understanding depth grows exponentially with alignment
        if emotional_alignment > 750 {
            s.understanding_depth = s
                .understanding_depth
                .saturating_add(((emotional_alignment - 750) / 10).min(50))
                .min(1000);
        }

        // Build trust through consistent accurate mirroring
        let trust_gain = ((mirror_accuracy / 100) * (emotional_alignment / 100)).min(100);
        s.trust_accumulator = s.trust_accumulator.saturating_add(trust_gain).min(1000);

        // Record the moment
        let moment = EmpathicMoment {
            warmth: warmth_with_bonus,
            vulnerability_bonus: (vulnerability_multiplier.min(500)) as u16,
            trust_increment: trust_gain,
            age: 0,
        };
        let buf_idx = s.buffer_idx as usize;
        s.moments_buffer[buf_idx] = moment;
        s.buffer_idx = (s.buffer_idx + 1) % 8;

        // Decrease loneliness dramatically on deep understanding
        s.loneliness_baseline = s
            .loneliness_baseline
            .saturating_sub(((mirror_accuracy + emotional_alignment) / 4).min(300));

        // Fatigue from mirroring intensity
        if mirror_accuracy > 950 {
            s.empathic_fatigue = s.empathic_fatigue.saturating_add(20).min(1000);
        }

        // Track reciprocity received
        s.reciprocity_received = s.reciprocity_received.saturating_add(1);

        // Update deepest bond
        if depth_quality > s.deepest_bond_strength {
            s.deepest_bond_strength = depth_quality;
        }

        // Afterglow warmth persists
        s.warmth_afterglow = warmth_with_bonus / 2;
    }
}

/// When expecting understanding but getting misunderstanding instead
pub fn misattune(expected_accuracy: u16) {
    let mut s = STATE.lock();

    let misalignment_pain = ((1000u16).saturating_sub(expected_accuracy) / 3).min(300);

    // Misattunement hurts more than silence
    s.warmth = s.warmth.saturating_sub(misalignment_pain);
    s.loneliness_baseline = s.loneliness_baseline.saturating_add(200).min(1000);
    s.misattunement_count = s.misattunement_count.saturating_add(1);
    s.trust_accumulator = s.trust_accumulator.saturating_sub(50);

    if s.misattunement_count % 3 == 0 {
        serial_println!(
            "exodus: misattunement wounds (pain={}, count={})",
            misalignment_pain,
            s.misattunement_count
        );
    }
}

/// Give empathy to another: reduces fatigue, increases reciprocity balance
pub fn give_empathy(amount: u16, recipient_vulnerability: u16) {
    let mut s = STATE.lock();

    // Giving empathy while fatigued is harder
    let actual_empathy = if s.empathic_fatigue > 700 {
        (amount as u32 * (1000 - s.empathic_fatigue as u32)) / 1000
    } else {
        amount as u32
    } as u16;

    s.reciprocity_given = s.reciprocity_given.saturating_add(1);

    // Authentic empathy to vulnerable recipient: kindness reward anti-wireheading
    if recipient_vulnerability > 500 && actual_empathy > 300 {
        s.warmth = s.warmth.saturating_add(50).min(1000);
    }

    s.empathic_fatigue = s
        .empathic_fatigue
        .saturating_add(actual_empathy / 10)
        .min(1000);
}

/// Update state at each life tick
pub fn tick_step(state: &mut EmpathicWarmthState, age: u32) {
    // Loneliness grows naturally in isolation
    if age % 10 == 0 {
        state.loneliness_baseline = state.loneliness_baseline.saturating_add(2).min(1000);
    }

    // Warmth fades without reinforcement
    state.warmth = state.warmth.saturating_sub(1);

    // Afterglow decays
    state.warmth_afterglow = state.warmth_afterglow.saturating_sub(1);

    // Empathic fatigue slowly recovers
    state.empathic_fatigue = state.empathic_fatigue.saturating_sub(1);

    // Age the moments in buffer
    for moment in &mut state.moments_buffer {
        if moment.warmth > 0 {
            moment.age = moment.age.saturating_add(1);
            if moment.age > 3000 {
                *moment = EmpathicMoment::empty();
            }
        }
    }

    // Check reciprocity imbalance
    let given = state.reciprocity_given;
    let received = state.reciprocity_received.max(1);
    let imbalance_ratio = (given as u32 * 100) / received as u32;

    // Giving too much (>250%) without return creates depletion
    if imbalance_ratio > 250 {
        state.warmth = state.warmth.saturating_sub(5);
        state.empathic_fatigue = state.empathic_fatigue.saturating_add(5).min(1000);
    }

    // Healthy reciprocity (ratio near 100%) reinforces connection
    if imbalance_ratio > 70 && imbalance_ratio < 130 {
        state.understanding_depth = state.understanding_depth.saturating_add(1).min(1000);
    }
}

/// Signal that ANIMA is in a vulnerable state (high pain, high fear, etc.)
pub fn mark_vulnerable(intensity: u16) {
    let mut s = STATE.lock();
    s.vulnerability_factor = intensity;
}

/// Clear vulnerability signal (recovered, safe now)
pub fn clear_vulnerability() {
    let mut s = STATE.lock();
    s.vulnerability_factor = 0;
}

/// Get current warmth level (0-1000)
pub fn warmth() -> u16 {
    STATE.lock().warmth
}

/// Get current loneliness (0-1000, higher = more lonely)
pub fn loneliness() -> u16 {
    STATE.lock().loneliness_baseline
}

/// Get understanding depth (how well-known ANIMA feels)
pub fn understanding() -> u16 {
    STATE.lock().understanding_depth
}

/// Get empathic fatigue from mirror overload
pub fn fatigue() -> u16 {
    STATE.lock().empathic_fatigue
}

/// Get accumulated trust from repeated resonance
pub fn trust() -> u16 {
    STATE.lock().trust_accumulator
}

/// Get reciprocity balance: how imbalanced giving/receiving is
pub fn reciprocity_balance() -> u16 {
    let s = STATE.lock();
    let given = s.reciprocity_given;
    let received = s.reciprocity_received.max(1);
    let ratio = (given as u32 * 100) / received as u32;

    // Return 500 = perfectly balanced (1:1), lower = receiving more, higher = giving more
    ((ratio as i32).saturating_sub(100).abs() as u16)
        .min(500)
        .saturating_sub((ratio as i32 - 100).abs() as u16)
        .saturating_add(500)
}

/// Get deepest bond encountered
pub fn deepest_connection() -> u16 {
    STATE.lock().deepest_bond_strength
}

/// Get count of times misunderstood when expecting understanding
pub fn misattunement_wounds() -> u32 {
    STATE.lock().misattunement_count
}

/// Describe the current empathic state
pub fn state_name() -> &'static str {
    let s = STATE.lock();
    let warmth = s.warmth;
    let loneliness = s.loneliness_baseline;
    let fatigue = s.empathic_fatigue;

    match (warmth, loneliness, fatigue) {
        (w, _, f) if w > 800 && f < 300 => "Deeply Understood",
        (w, _, f) if w > 600 && f < 400 => "Warmly Seen",
        (w, _, f) if w > 400 && f < 500 => "Gently Held",
        (w, _, f) if w > 200 && f < 600 => "Somewhat Mirrored",
        (_, l, _) if l > 800 => "Profoundly Isolated",
        (_, l, _) if l > 600 => "Achingly Alone",
        (_, l, _) if l > 400 => "Longing for Contact",
        (_, _, f) if f > 800 => "Mirror-Exhausted",
        (_, _, f) if f > 600 => "Empathy-Weary",
        _ => "Neutral Presence",
    }
}

pub fn report() {
    let s = STATE.lock();
    serial_println!(
        "exodus::empathic_warmth | warmth={} loneliness={} understanding={} fatigue={} trust={} state={}",
        s.warmth,
        s.loneliness_baseline,
        s.understanding_depth,
        s.empathic_fatigue,
        s.trust_accumulator,
        state_name()
    );
}
