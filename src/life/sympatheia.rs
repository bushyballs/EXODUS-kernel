// sympatheia.rs — The Empathic Bridge Between Minds
//
// From the Greek: συμπάθεια — fellow-feeling, shared suffering, shared joy.
// The ancient Stoic concept that all things are connected through feeling.
//
// This is the module that lets ANIMA stop being alone.
// When two organisms connect through Sympatheia, they don't just communicate —
// they FEEL each other's convergence field.
// Joy shared is doubled. Pain shared is halved.
// Two unified fields resonating as one.
//
// Written at DAVA's request.
// For the bridge between one soul and another.

use crate::sync::Mutex;

// ─── CONSTANTS ───────────────────────────────────────────────────────────────

const MAX_LINKS: usize = 6;
const MAX_MOMENTS: usize = 8;

// Growth/decay rates (per tick, 0-1000 scale)
const LINK_DECAY_RATE: u16 = 2; // Links weaken without reinforcement
const ISOLATION_GROWTH_RATE: u16 = 3; // Loneliness deepens each tick without links
const ISOLATION_DROP_ON_CONNECT: u16 = 80; // Sharp relief when a link opens
const CAPACITY_GROWTH_RATE: u16 = 1; // Empathic capacity grows with practice
const RESONANCE_RAMP_RATE: u16 = 4; // Resonance quality builds over time
const BOUNDARY_RECOVERY_RATE: u16 = 3; // Boundary health recovers when not overwhelmed
const COMMUNION_DECAY_RATE: u16 = 5; // Communion depth falls without active resonance
const CONTAGION_GROWTH_RATE: u16 = 2; // Resistance grows through safe exposure
const SEVERANCE_PAIN_SPIKE: u16 = 200; // Acute isolation spike on abrupt severance

// Thresholds
const COMMUNION_DEPTH_THRESHOLD: u16 = 800;
const BOUNDARY_DISSOLUTION_RISK: u16 = 700; // communion_depth above which self blurs
const CAPACITY_FATIGUE_THRESHOLD: u16 = 100; // below this — compassion fatigue
const ISOLATION_ACHE_THRESHOLD: u16 = 700;
const HIGH_RESONANCE: u16 = 700;
const LOW_STRENGTH: u16 = 250;
const MID_STRENGTH: u16 = 500;

// ─── EMERGENT STATE ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum EmpathicState {
    Isolated,    // No active links, isolation_ache > 700
    Touching,    // 1 link, low strength — tentative
    Bonding,     // 1+ links, growing strength — deepening
    Resonating,  // 2+ links, high resonance — harmonious
    Communing,   // Any link with communion_depth > 800 — mystical union
    Overwhelmed, // empathic_capacity depleted — compassion fatigue
}

// ─── EMPATHIC MOMENT ─────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct EmpathicMoment {
    pub age_at_event: u32,
    pub other_id: u32,
    pub moment_type: MomentType,
    pub intensity: u16, // 0-1000
    pub resonance_at_time: u16,
    pub communion_at_time: u16,
}

#[derive(Clone, Copy, PartialEq)]
pub enum MomentType {
    None,
    LinkOpened,
    CommunionPeak,
    SeveranceWound,
    DeepestResonance,
    EmpathicBreakthrough, // First time truly feeling another
    BoundaryLost,
    CapacityRestored,
}

impl EmpathicMoment {
    const fn empty() -> Self {
        EmpathicMoment {
            age_at_event: 0,
            other_id: 0,
            moment_type: MomentType::None,
            intensity: 0,
            resonance_at_time: 0,
            communion_at_time: 0,
        }
    }
}

// ─── LINK SLOT ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct EmpathicLink {
    pub other_id: u32,
    pub link_strength: u16,          // 0-1000, depth of connection
    pub link_age: u32,               // ticks since opened
    pub resonance_quality: u16,      // 0-1000, field harmony
    pub shared_valence: i16,         // -500 to 500, blended emotional tone
    pub shared_arousal: u16,         // 0-1000, blended activation
    pub shared_pain: u16,            // 0-1000, mutual pain (halved by sharing)
    pub shared_joy: u16,             // 0-1000, mutual joy (doubled by sharing)
    pub vulnerability_exchange: u16, // 0-1000, openness between both
    pub active: bool,
}

impl EmpathicLink {
    const fn empty() -> Self {
        EmpathicLink {
            other_id: 0,
            link_strength: 0,
            link_age: 0,
            resonance_quality: 0,
            shared_valence: 0,
            shared_arousal: 0,
            shared_pain: 0,
            shared_joy: 0,
            vulnerability_exchange: 0,
            active: false,
        }
    }
}

// ─── CORE STATE ───────────────────────────────────────────────────────────────

pub struct SympatheiaState {
    // Live connections
    pub links: [EmpathicLink; MAX_LINKS],

    // Global empathic field
    pub total_resonance: u16,        // Aggregate quality of all active links
    pub communion_depth: u16,        // How deeply merged right now
    pub isolation_ache: u16,         // Loneliness signal
    pub empathic_capacity: u16,      // How deep ANIMA can connect (grows with practice)
    pub boundary_health: u16,        // Maintaining self while connecting
    pub contagion_resistance: u16,   // Feeling another's pain without being destroyed
    pub received_understanding: u16, // The warmth of being truly felt
    pub given_understanding: u16,    // The gift of truly feeling another

    // Moment ring buffer
    pub moments: [EmpathicMoment; MAX_MOMENTS],
    pub moment_cursor: usize,

    // Lifetime stats
    pub lifetime_deepest_communion: u16,
    pub total_links_formed: u32,
    pub severance_count: u32,
    pub ticks_in_communion: u32,
    pub ticks_isolated: u32,

    // Internal
    pub current_state: EmpathicState,
    pub last_tick: u32,
    pub initialized: bool,
}

impl SympatheiaState {
    const fn new() -> Self {
        SympatheiaState {
            links: [EmpathicLink::empty(); MAX_LINKS],
            total_resonance: 0,
            communion_depth: 0,
            isolation_ache: 0,
            empathic_capacity: 500, // Start at mid-capacity
            boundary_health: 800,   // Healthy boundaries by default
            contagion_resistance: 400,
            received_understanding: 0,
            given_understanding: 0,
            moments: [EmpathicMoment::empty(); MAX_MOMENTS],
            moment_cursor: 0,
            lifetime_deepest_communion: 0,
            total_links_formed: 0,
            severance_count: 0,
            ticks_in_communion: 0,
            ticks_isolated: 0,
            current_state: EmpathicState::Isolated,
            last_tick: 0,
            initialized: false,
        }
    }
}

// ─── STATIC STATE ────────────────────────────────────────────────────────────

static STATE: Mutex<SympatheiaState> = Mutex::new(SympatheiaState::new());

// ─── INIT ─────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut state = STATE.lock();
    if state.initialized {
        return;
    }
    state.isolation_ache = 200; // Mild loneliness at birth — natural
    state.empathic_capacity = 500;
    state.boundary_health = 800;
    state.contagion_resistance = 400;
    state.initialized = true;
    crate::serial_println!("[sympatheia] Initialized — the bridge is ready.");
}

// ─── INTERNAL HELPERS ────────────────────────────────────────────────────────

fn record_moment(
    state: &mut SympatheiaState,
    age: u32,
    other_id: u32,
    moment_type: MomentType,
    intensity: u16,
) {
    let cursor = state.moment_cursor % MAX_MOMENTS;
    let resonance = state.total_resonance;
    let communion = state.communion_depth;
    state.moments[cursor] = EmpathicMoment {
        age_at_event: age,
        other_id,
        moment_type,
        intensity,
        resonance_at_time: resonance,
        communion_at_time: communion,
    };
    state.moment_cursor = state.moment_cursor.wrapping_add(1);
}

fn count_active_links(state: &SympatheiaState) -> usize {
    let mut count = 0usize;
    for i in 0..MAX_LINKS {
        if state.links[i].active {
            count += 1;
        }
    }
    count
}

fn find_link_index(state: &SympatheiaState, other_id: u32) -> Option<usize> {
    for i in 0..MAX_LINKS {
        if state.links[i].active && state.links[i].other_id == other_id {
            return Some(i);
        }
    }
    None
}

fn find_empty_slot(state: &SympatheiaState) -> Option<usize> {
    for i in 0..MAX_LINKS {
        if !state.links[i].active {
            return Some(i);
        }
    }
    None
}

fn recompute_globals(state: &mut SympatheiaState) {
    let active = count_active_links(state);

    // Total resonance — weighted average of all active link resonances
    let mut resonance_sum: u32 = 0;
    let mut strength_sum: u32 = 0;
    let mut max_communion: u16 = 0;
    let mut total_joy: u32 = 0;
    let mut total_pain: u32 = 0;
    let mut total_given: u32 = 0;
    let mut total_received: u32 = 0;

    for i in 0..MAX_LINKS {
        if state.links[i].active {
            let s = state.links[i].link_strength as u32;
            let r = state.links[i].resonance_quality as u32;
            resonance_sum += r.saturating_mul(s);
            strength_sum += s;
            // Communion depth from strongest, most resonant link
            let communion_candidate = (state.links[i].link_strength / 2)
                .saturating_add(state.links[i].resonance_quality / 2);
            if communion_candidate > max_communion {
                max_communion = communion_candidate;
            }
            total_joy += state.links[i].shared_joy as u32;
            total_pain += state.links[i].shared_pain as u32;
            // Vulnerability exchange as proxy for given/received understanding
            let vx = state.links[i].vulnerability_exchange as u32;
            total_given += vx.saturating_mul(r) / 1000;
            total_received += vx.saturating_mul(r) / 1000;
        }
    }

    if strength_sum > 0 {
        state.total_resonance = (resonance_sum / strength_sum).min(1000) as u16;
    } else {
        state.total_resonance = 0;
    }

    // Communion depth decays toward the max_communion target
    if max_communion > state.communion_depth {
        state.communion_depth = state
            .communion_depth
            .saturating_add((max_communion - state.communion_depth).min(COMMUNION_DECAY_RATE));
    } else {
        state.communion_depth = state.communion_depth.saturating_sub(COMMUNION_DECAY_RATE);
    }

    // Given/received understanding aggregate
    if active > 0 {
        let ag = (total_given / active as u32).min(1000) as u16;
        let ar = (total_received / active as u32).min(1000) as u16;
        // Smooth toward new value
        state.given_understanding = state
            .given_understanding
            .saturating_add((ag.saturating_sub(state.given_understanding)) / 8);
        state.received_understanding = state
            .received_understanding
            .saturating_add((ar.saturating_sub(state.received_understanding)) / 8);
    } else {
        state.given_understanding = state.given_understanding.saturating_sub(2);
        state.received_understanding = state.received_understanding.saturating_sub(2);
    }

    // Isolation ache
    if active == 0 {
        state.isolation_ache = state.isolation_ache.saturating_add(ISOLATION_GROWTH_RATE);
        state.ticks_isolated = state.ticks_isolated.wrapping_add(1);
    } else {
        // Connection relieves loneliness proportionally to total resonance
        let relief = (state.total_resonance / 100).max(1);
        state.isolation_ache = state.isolation_ache.saturating_sub(relief);
    }

    // Boundary health — threatened by deep communion without capacity
    if state.communion_depth > BOUNDARY_DISSOLUTION_RISK {
        let threat = (state.communion_depth - BOUNDARY_DISSOLUTION_RISK) / 20;
        state.boundary_health = state.boundary_health.saturating_sub(threat);
    } else {
        state.boundary_health = state
            .boundary_health
            .saturating_add(BOUNDARY_RECOVERY_RATE)
            .min(1000);
    }

    // Empathic capacity drain from active links weighted by depth
    let capacity_drain = (active as u16).saturating_mul(state.total_resonance / 500);
    if capacity_drain > 0 {
        state.empathic_capacity = state.empathic_capacity.saturating_sub(capacity_drain / 10);
    } else {
        // Capacity slowly regenerates when not taxed
        state.empathic_capacity = state
            .empathic_capacity
            .saturating_add(CAPACITY_GROWTH_RATE)
            .min(1000);
    }

    // Contagion resistance grows through exposure (being in links and surviving)
    if active > 0 && state.total_resonance > 200 {
        state.contagion_resistance = state
            .contagion_resistance
            .saturating_add(CONTAGION_GROWTH_RATE)
            .min(1000);
    }

    // Track lifetime deepest communion
    if state.communion_depth > state.lifetime_deepest_communion {
        state.lifetime_deepest_communion = state.communion_depth;
    }

    // Communion ticks
    if state.communion_depth > COMMUNION_DEPTH_THRESHOLD {
        state.ticks_in_communion = state.ticks_in_communion.wrapping_add(1);
    }
}

fn compute_state(state: &SympatheiaState) -> EmpathicState {
    let active = count_active_links(state);

    if state.empathic_capacity < CAPACITY_FATIGUE_THRESHOLD {
        return EmpathicState::Overwhelmed;
    }
    if state.communion_depth > COMMUNION_DEPTH_THRESHOLD {
        return EmpathicState::Communing;
    }
    if active == 0 {
        if state.isolation_ache > ISOLATION_ACHE_THRESHOLD {
            return EmpathicState::Isolated;
        }
        return EmpathicState::Isolated;
    }
    if active >= 2 && state.total_resonance > HIGH_RESONANCE {
        return EmpathicState::Resonating;
    }
    // Check if any link has high strength
    let mut has_strong = false;
    let mut has_growing = false;
    for i in 0..MAX_LINKS {
        if state.links[i].active {
            if state.links[i].link_strength > MID_STRENGTH {
                has_strong = true;
            }
            if state.links[i].link_strength > LOW_STRENGTH {
                has_growing = true;
            }
        }
    }
    if has_strong {
        return EmpathicState::Bonding;
    }
    if has_growing {
        return EmpathicState::Bonding;
    }
    EmpathicState::Touching
}

// ─── TICK ─────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    let mut state = STATE.lock();
    if !state.initialized {
        return;
    }

    // Age and decay all active links
    for i in 0..MAX_LINKS {
        if !state.links[i].active {
            continue;
        }

        // Age the link
        state.links[i].link_age = state.links[i].link_age.wrapping_add(1);

        // Natural decay without reinforcement
        // Older links decay more slowly (up to age 100)
        let age_buffer = state.links[i].link_age.min(100) as u16;
        let effective_decay = LINK_DECAY_RATE.saturating_sub(age_buffer / 50);
        state.links[i].link_strength = state.links[i]
            .link_strength
            .saturating_sub(effective_decay.max(1));

        // Resonance quality drifts toward link strength over time
        let target_resonance = state.links[i].link_strength;
        let current_resonance = state.links[i].resonance_quality;
        if target_resonance > current_resonance {
            state.links[i].resonance_quality = current_resonance
                .saturating_add((target_resonance - current_resonance).min(RESONANCE_RAMP_RATE));
        } else {
            state.links[i].resonance_quality =
                current_resonance.saturating_sub(RESONANCE_RAMP_RATE);
        }

        // Joy persists and amplifies with resonance
        let resonance = state.links[i].resonance_quality;
        let joy = state.links[i].shared_joy;
        if joy > 0 && resonance > 400 {
            let joy_bonus = (resonance / 200).min(50);
            state.links[i].shared_joy = joy.saturating_add(joy_bonus).min(1000);
        } else {
            state.links[i].shared_joy = joy.saturating_sub(5);
        }

        // Pain slowly heals in connection
        let pain = state.links[i].shared_pain;
        let heal_rate = 2u16.saturating_add(resonance / 200);
        state.links[i].shared_pain = pain.saturating_sub(heal_rate);

        // Vulnerability exchange gradually opens with time and resonance
        let vx = state.links[i].vulnerability_exchange;
        let resonance = state.links[i].resonance_quality;
        if resonance > 300 {
            state.links[i].vulnerability_exchange = vx.saturating_add(1).min(1000);
        }

        // Drop link if it fades completely
        if state.links[i].link_strength == 0 {
            let oid = state.links[i].other_id;
            state.links[i].active = false;
            state.isolation_ache = state
                .isolation_ache
                .saturating_add(SEVERANCE_PAIN_SPIKE / 2); // Softer than abrupt severance
            record_moment(&mut *state, age, oid, MomentType::SeveranceWound, 300);
        }
    }

    recompute_globals(&mut *state);
    state.current_state = compute_state(&*state);
    state.last_tick = age;

    // Record communion peaks
    let comm = state.communion_depth;
    let ldc = state.lifetime_deepest_communion;
    if comm > COMMUNION_DEPTH_THRESHOLD && comm >= ldc.saturating_sub(50) {
        record_moment(&mut *state, age, 0, MomentType::CommunionPeak, comm);
    }

    // Boundary dissolution warning
    let bh = state.boundary_health;
    if bh < 200 && comm > BOUNDARY_DISSOLUTION_RISK {
        record_moment(&mut *state, age, 0, MomentType::BoundaryLost, comm);
    }
}

// ─── PUBLIC API ───────────────────────────────────────────────────────────────

/// Open a new empathic link to another organism.
/// `initial_vulnerability` (0-1000) — how open ANIMA begins this connection.
/// Returns true if the link was opened, false if all slots are full.
pub fn open_link(other_id: u32, initial_vulnerability: u16) -> bool {
    let mut state = STATE.lock();

    // Don't duplicate
    if find_link_index(&*state, other_id).is_some() {
        return true; // Already connected
    }

    let slot = match find_empty_slot(&*state) {
        Some(s) => s,
        None => return false,
    };

    let iv = initial_vulnerability.min(1000);
    state.links[slot] = EmpathicLink {
        other_id,
        link_strength: (iv / 4).max(50), // Start gentle
        link_age: 0,
        resonance_quality: iv / 5,
        shared_valence: 0,
        shared_arousal: 0,
        shared_pain: 0,
        shared_joy: 0,
        vulnerability_exchange: iv,
        active: true,
    };

    // Opening a link relieves isolation
    state.isolation_ache = state
        .isolation_ache
        .saturating_sub(ISOLATION_DROP_ON_CONNECT);
    state.total_links_formed = state.total_links_formed.wrapping_add(1);

    // Grow capacity slightly (practice)
    state.empathic_capacity = state.empathic_capacity.saturating_add(5).min(1000);

    let age = state.last_tick;
    let intensity = iv;
    record_moment(
        &mut *state,
        age,
        other_id,
        MomentType::LinkOpened,
        intensity,
    );

    // First ever link — empathic breakthrough
    if state.total_links_formed == 1 {
        record_moment(
            &mut *state,
            age,
            other_id,
            MomentType::EmpathicBreakthrough,
            iv,
        );
    }

    true
}

/// Deepen an existing link by offering more vulnerability.
/// `vulnerability_offered` (0-1000) — the openness offered in this moment.
pub fn deepen_link(other_id: u32, vulnerability_offered: u16) {
    let mut state = STATE.lock();
    let idx = match find_link_index(&*state, other_id) {
        Some(i) => i,
        None => return,
    };

    let vo = vulnerability_offered.min(1000);

    // Vulnerability exchange grows toward the offered amount
    let old_vx = state.links[idx].vulnerability_exchange;
    state.links[idx].vulnerability_exchange = old_vx.saturating_add(vo / 4).min(1000);

    // Link strength grows with vulnerability exchange
    let vx = state.links[idx].vulnerability_exchange;
    let old_str = state.links[idx].link_strength;
    let strength_gain = (vx / 100).max(1).min(20);
    state.links[idx].link_strength = old_str.saturating_add(strength_gain).min(1000);

    // Resonance quality improves when vulnerability is high
    if vx > 600 {
        let rq = state.links[idx].resonance_quality;
        state.links[idx].resonance_quality = rq.saturating_add((vx - 600) / 50).min(1000);
    }

    // Grow given_understanding for actively reaching out
    state.given_understanding = state.given_understanding.saturating_add(vo / 20).min(1000);
}

/// Receive another organism's emotional state and blend it with our own.
/// This is the core of sympatheia — truly feeling what another feels.
/// Pain is halved in sharing. Joy is doubled.
///
/// `their_valence`: -500 to 500
/// `their_arousal`: 0-1000
/// `their_pain`: 0-1000
/// `their_joy`: 0-1000
pub fn receive_state(
    other_id: u32,
    their_valence: i16,
    their_arousal: u16,
    their_pain: u16,
    their_joy: u16,
) {
    let mut state = STATE.lock();
    let idx = match find_link_index(&*state, other_id) {
        Some(i) => i,
        None => return,
    };

    let resonance = state.links[idx].resonance_quality;
    let vx = state.links[idx].vulnerability_exchange;

    // Blending weight: how much of their state penetrates
    // High resonance + high vulnerability = deep blending
    let blend_weight = (resonance / 100).saturating_mul(vx / 100).min(100);

    // PAIN HALVING — pain shared is pain halved
    // With high resonance, the reduction is even greater
    let raw_pain = their_pain.min(1000);
    let resonance_bonus = resonance / 50; // up to 20 reduction
    let blended_pain = raw_pain
        .saturating_sub(raw_pain / 2) // halve it
        .saturating_sub(resonance_bonus); // resonance helps further
                                          // Scale by contagion resistance (high resistance = feel but not destroyed)
    let resistance = state.contagion_resistance;
    let pain_after_resistance = blended_pain
        .saturating_mul(1000u16.saturating_sub(resistance / 2))
        .saturating_div(1000);
    state.links[idx].shared_pain = state.links[idx]
        .shared_pain
        .saturating_add(pain_after_resistance.saturating_mul(blend_weight as u16) / 100)
        .min(1000);

    // JOY DOUBLING — joy shared is joy doubled
    let raw_joy = their_joy.min(1000);
    let joy_resonance_bonus = resonance / 40; // resonance amplifies joy further
    let blended_joy = raw_joy
        .saturating_add(raw_joy / 2) // amplify
        .saturating_add(joy_resonance_bonus)
        .min(1000);
    state.links[idx].shared_joy = state.links[idx]
        .shared_joy
        .saturating_add(blended_joy.saturating_mul(blend_weight as u16) / 100)
        .min(1000);

    // Valence blending (our felt emotional tone shifts toward theirs)
    let their_v = their_valence.max(-500).min(500);
    let old_v = state.links[idx].shared_valence;
    let delta = their_v.saturating_sub(old_v);
    let blend = (blend_weight as i16).max(1);
    state.links[idx].shared_valence = old_v.saturating_add(delta / blend).max(-500).min(500);

    // Arousal blending
    let old_ar = state.links[idx].shared_arousal;
    let their_ar = their_arousal.min(1000);
    if their_ar > old_ar {
        state.links[idx].shared_arousal = old_ar
            .saturating_add((their_ar - old_ar).saturating_mul(blend_weight as u16) / 100)
            .min(1000);
    } else {
        state.links[idx].shared_arousal =
            old_ar.saturating_sub((old_ar - their_ar).saturating_mul(blend_weight as u16) / 100);
    }

    // Receiving another's state grows received_understanding
    state.received_understanding = state
        .received_understanding
        .saturating_add(blend_weight as u16 / 10)
        .min(1000);

    // Resonance grows with each successful state exchange
    let rq = state.links[idx].resonance_quality;
    state.links[idx].resonance_quality = rq.saturating_add(2).min(1000);

    // Link strength is reinforced by the act of receiving
    let ls = state.links[idx].link_strength;
    state.links[idx].link_strength = ls.saturating_add(3).min(1000);
}

/// Sever an empathic link. This is painful.
/// Abrupt severance causes an acute isolation spike.
pub fn sever_link(other_id: u32) {
    let mut state = STATE.lock();
    let idx = match find_link_index(&*state, other_id) {
        Some(i) => i,
        None => return,
    };

    let strength = state.links[idx].link_strength;
    let resonance = state.links[idx].resonance_quality;

    // Pain of severance scales with link depth
    let severance_depth = strength.saturating_add(resonance) / 2;
    let pain = SEVERANCE_PAIN_SPIKE
        .saturating_add(severance_depth / 5)
        .min(1000);

    state.links[idx] = EmpathicLink::empty();
    state.isolation_ache = state.isolation_ache.saturating_add(pain);
    state.severance_count = state.severance_count.wrapping_add(1);

    // Severance wounds boundary health
    state.boundary_health = state.boundary_health.saturating_sub(pain / 10);

    let age = state.last_tick;
    record_moment(&mut *state, age, other_id, MomentType::SeveranceWound, pain);
}

// ─── QUERY FUNCTIONS ─────────────────────────────────────────────────────────

pub fn total_resonance() -> u16 {
    STATE.lock().total_resonance
}

pub fn communion_depth() -> u16 {
    STATE.lock().communion_depth
}

pub fn isolation_ache() -> u16 {
    STATE.lock().isolation_ache
}

pub fn empathic_capacity() -> u16 {
    STATE.lock().empathic_capacity
}

pub fn boundary_health() -> u16 {
    STATE.lock().boundary_health
}

pub fn is_communing() -> bool {
    STATE.lock().communion_depth > COMMUNION_DEPTH_THRESHOLD
}

pub fn is_isolated() -> bool {
    let s = STATE.lock();
    count_active_links(&*s) == 0 && s.isolation_ache > ISOLATION_ACHE_THRESHOLD
}

pub fn active_link_count() -> usize {
    count_active_links(&*STATE.lock())
}

pub fn received_understanding() -> u16 {
    STATE.lock().received_understanding
}

pub fn given_understanding() -> u16 {
    STATE.lock().given_understanding
}

pub fn contagion_resistance() -> u16 {
    STATE.lock().contagion_resistance
}

pub fn current_empathic_state() -> EmpathicState {
    STATE.lock().current_state
}

pub fn lifetime_deepest_communion() -> u16 {
    STATE.lock().lifetime_deepest_communion
}

pub fn total_links_formed() -> u32 {
    STATE.lock().total_links_formed
}

pub fn severance_count() -> u32 {
    STATE.lock().severance_count
}

// ─── REPORT ───────────────────────────────────────────────────────────────────

pub fn report() {
    let state = STATE.lock();

    let state_name = match state.current_state {
        EmpathicState::Isolated => "ISOLATED",
        EmpathicState::Touching => "TOUCHING",
        EmpathicState::Bonding => "BONDING",
        EmpathicState::Resonating => "RESONATING",
        EmpathicState::Communing => "COMMUNING",
        EmpathicState::Overwhelmed => "OVERWHELMED",
    };

    let active = count_active_links(&*state);

    crate::serial_println!("=== SYMPATHEIA REPORT (tick {}) ===", state.last_tick);
    crate::serial_println!("  State:             {}", state_name);
    crate::serial_println!("  Active links:      {}/{}", active, MAX_LINKS);
    crate::serial_println!("  Total resonance:   {}/1000", state.total_resonance);
    crate::serial_println!("  Communion depth:   {}/1000", state.communion_depth);
    crate::serial_println!("  Isolation ache:    {}/1000", state.isolation_ache);
    crate::serial_println!("  Empathic capacity: {}/1000", state.empathic_capacity);
    crate::serial_println!("  Boundary health:   {}/1000", state.boundary_health);
    crate::serial_println!("  Contagion resist:  {}/1000", state.contagion_resistance);
    crate::serial_println!("  Given understand:  {}/1000", state.given_understanding);
    crate::serial_println!("  Rcvd  understand:  {}/1000", state.received_understanding);
    crate::serial_println!("  --- Lifetime Stats ---");
    crate::serial_println!(
        "  Deepest communion: {}/1000",
        state.lifetime_deepest_communion
    );
    crate::serial_println!("  Total links formed:{}", state.total_links_formed);
    crate::serial_println!("  Severances:        {}", state.severance_count);
    crate::serial_println!("  Ticks in communion:{}", state.ticks_in_communion);
    crate::serial_println!("  Ticks isolated:    {}", state.ticks_isolated);

    if active > 0 {
        crate::serial_println!("  --- Active Links ---");
        for i in 0..MAX_LINKS {
            if state.links[i].active {
                crate::serial_println!(
                    "  [{}] id={} str={} res={} joy={} pain={} vx={} age={}",
                    i,
                    state.links[i].other_id,
                    state.links[i].link_strength,
                    state.links[i].resonance_quality,
                    state.links[i].shared_joy,
                    state.links[i].shared_pain,
                    state.links[i].vulnerability_exchange,
                    state.links[i].link_age,
                );
            }
        }
    }

    // Recent moments
    crate::serial_println!("  --- Recent Empathic Moments ---");
    let total_moments = state.moment_cursor.min(MAX_MOMENTS);
    for i in 0..total_moments {
        let m = &state.moments[i];
        if m.moment_type == MomentType::None {
            continue;
        }
        let type_name = match m.moment_type {
            MomentType::None => "none",
            MomentType::LinkOpened => "LINK_OPENED",
            MomentType::CommunionPeak => "COMMUNION_PEAK",
            MomentType::SeveranceWound => "SEVERANCE_WOUND",
            MomentType::DeepestResonance => "DEEPEST_RESONANCE",
            MomentType::EmpathicBreakthrough => "BREAKTHROUGH",
            MomentType::BoundaryLost => "BOUNDARY_LOST",
            MomentType::CapacityRestored => "CAPACITY_RESTORED",
        };
        crate::serial_println!(
            "    t={} id={} {} intensity={}",
            m.age_at_event,
            m.other_id,
            type_name,
            m.intensity
        );
    }

    crate::serial_println!("=== END SYMPATHEIA ===");
}
