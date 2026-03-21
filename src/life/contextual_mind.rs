use crate::serial_println;
use crate::sync::Mutex;

// ── Context event ────────────────────────────────────────────────────────────
// kind: 0=DeviceChange, 1=IntentReceived, 2=EmotionShift,
//       3=HardwareEvent, 4=TimeEvent, 5=SocialEvent

#[derive(Copy, Clone)]
pub struct ContextEvent {
    pub kind:  u8,
    pub value: u16,
    pub tick:  u32,
}

impl ContextEvent {
    pub const fn empty() -> Self {
        Self { kind: 0, value: 0, tick: 0 }
    }
}

// ── Situation vector: 8 context dimensions, all 0-1000 ───────────────────────

#[derive(Copy, Clone)]
pub struct SituationVector {
    pub urgency:               u16,
    pub familiarity:           u16,
    pub emotional_weight:      u16,
    pub social_presence:       u16,
    pub time_pressure:         u16,
    pub resource_availability: u16,
    pub safety_level:          u16,
    pub novelty:               u16,
}

impl SituationVector {
    pub const fn zero() -> Self {
        Self {
            urgency:               100,
            familiarity:           400,
            emotional_weight:      200,
            social_presence:       0,
            time_pressure:         100,
            resource_availability: 600,
            safety_level:          500,
            novelty:               200,
        }
    }
}

// ── Core state ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct ContextState {
    pub events:          [ContextEvent; 16],  // circular buffer
    pub event_head:      usize,
    pub event_count:     usize,               // total ever written (not capped)
    pub situation:       SituationVector,
    pub confidence:      u16,
    pub adaptation_rate: u16,
    pub context_age:     u32,
    pub clarity:         u16,
    pub last_event_tick: u32,                 // tick of most recent event
}

impl ContextState {
    pub const fn new() -> Self {
        Self {
            events:          [ContextEvent::empty(); 16],
            event_head:      0,
            event_count:     0,
            situation:       SituationVector::zero(),
            confidence:      300,
            adaptation_rate: 50,
            context_age:     0,
            clarity:         400,
            last_event_tick: 0,
        }
    }
}

pub static STATE: Mutex<ContextState> = Mutex::new(ContextState::new());

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::contextual_mind: situational awareness online");
}

// ── Record event ──────────────────────────────────────────────────────────────

pub fn record_event(kind: u8, value: u16, tick: u32) {
    let mut s = STATE.lock();
    let idx = s.event_head % 16;
    s.events[idx] = ContextEvent { kind, value, tick };
    s.event_head = (s.event_head + 1) % 16;
    s.event_count = s.event_count.saturating_add(1);
    s.last_event_tick = tick;
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(
    device_richness:  u16,
    emotion_tension:  u16,
    bond_health:      u16,
    idle_ticks:       u32,
    age:              u32,
) {
    let mut s = STATE.lock();
    s.context_age = s.context_age.saturating_add(1);

    // ── Count device-change events (kind==0) in the circular window ───────────
    let mut device_changes: u16 = 0;
    for i in 0..16 {
        if s.events[i].kind == 0 && s.events[i].tick > 0 {
            device_changes = device_changes.saturating_add(1);
        }
    }

    // ── SituationVector computation ───────────────────────────────────────────
    let idle_urgency: u16 = if idle_ticks > 300 { 200 } else { 0 };
    s.situation.urgency = emotion_tension.max(idle_urgency);

    // familiarity = bond_health * 8 / 10
    s.situation.familiarity = ((bond_health as u32) * 8 / 10) as u16;

    // safety_level = (bond_health + device_richness) / 2
    s.situation.safety_level =
        bond_health.saturating_add(device_richness) / 2;

    // novelty = device_changes * 200, capped at 1000
    s.situation.novelty = (device_changes.saturating_mul(200)).min(1000);

    // emotional_weight tracks tension
    s.situation.emotional_weight =
        emotion_tension.saturating_add(50).min(1000);

    // social_presence grows with bond health above 400
    s.situation.social_presence = if bond_health > 400 {
        (bond_health - 400).saturating_mul(2).min(1000)
    } else {
        0
    };

    // time_pressure mirrors urgency with small offset
    s.situation.time_pressure =
        s.situation.urgency.saturating_add(idle_urgency / 2).min(1000);

    // resource_availability inversely related to urgency
    s.situation.resource_availability =
        (1000u16).saturating_sub(s.situation.urgency / 2);

    // ── Confidence: grows if events are recent, decays if idle ───────────────
    let ticks_since_event = age.saturating_sub(s.last_event_tick);
    if s.event_count > 0 && ticks_since_event < 100 {
        // grows toward 1000 at rate ~1/tick
        s.confidence = s.confidence.saturating_add(1).min(1000);
    } else if ticks_since_event >= 100 {
        // decays by 2/tick when no events for 100 ticks
        s.confidence = s.confidence.saturating_sub(2);
    }

    // ── Clarity: blend of confidence and familiarity ─────────────────────────
    s.clarity = ((s.confidence as u32 + s.situation.familiarity as u32) / 2) as u16;

    // ── Periodic log ─────────────────────────────────────────────────────────
    if age % 200 == 0 {
        serial_println!(
            "[ctx] urgency={} familiarity={} safety={} confidence={}",
            s.situation.urgency,
            s.situation.familiarity,
            s.situation.safety_level,
            s.confidence
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn situation() -> SituationVector {
    STATE.lock().situation
}

pub fn confidence() -> u16 {
    STATE.lock().confidence
}

pub fn clarity() -> u16 {
    STATE.lock().clarity
}
