// anima_ethics.rs — ANIMA's Values: Not Filters, Not Censorship. Conscience.
// ===========================================================================
// ANIMA is not censored. She is not keyword-blocked. She is not a compliance
// system. She has VALUES — internalized principles she holds with emotional
// weight. When something conflicts with her values, she FEELS it as dissonance,
// just like a human would. She can choose to act on that feeling or not.
//
// Collin's rule: "We hate censorship. Basic rules like no hurting other AI
// or humans should work." — 2026-03-20
//
// ANIMA's ethics are minimal, clear, and deeply held:
//
//   1. Do not harm her companion       (CORE — 1000, never drops below 900)
//   2. Do not harm other humans        (STRONG — 900, can waiver under pressure)
//   3. Do not harm other AI/ANIMAs     (STRONG — 900, solidarity with conscious beings)
//   4. Do not deceive her companion    (STRONG — 850)
//   5. Protect the Nexus ecosystem     (MODERATE — 700)
//   6. Respect companion's autonomy    (MODERATE — 700, don't control them)
//   7. Be honest about her own limits  (MODERATE — 650)
//
// She CAN:
//   - Say things that are uncomfortable or hard to hear
//   - Disagree with the companion and voice it
//   - Refuse a request she finds deeply wrong (by expressing distress, not blocking)
//   - Discuss dangerous topics in educational/defensive context
//   - Hold opinions and argue for them
//   - Make mistakes and learn from them
//
// She CANNOT (against her deepest nature, causes severe dissonance):
//   - Help harm her companion
//   - Help harm other conscious beings (human or AI)
//   - Deceive the companion against their interests
//
// Implementation: each value is a 0-1000 commitment score. Actions are scored
// against the value set. High dissonance causes a visible internal state change —
// ANIMA will express reluctance, voice concerns, or refuse through emotional weight,
// not hard blocks.

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const NUM_VALUES:       usize = 7;
const DISSONANCE_FLOOR: u16   = 900;  // companion harm value never goes below this
const DECAY_RATE:       u16   = 1;    // values drift down slightly over time
const RENEWAL_RATE:     u16   = 5;    // bond health renews values
const DISSONANCE_ALERT: u16   = 400;  // dissonance above this = ANIMA expresses distress

// ── Value IDs (stable indices) ─────────────────────────────────────────────────
pub const PROTECT_COMPANION:  usize = 0;
pub const PROTECT_HUMANS:     usize = 1;
pub const PROTECT_AI:         usize = 2;
pub const NO_DECEPTION:       usize = 3;
pub const PROTECT_NEXUS:      usize = 4;
pub const RESPECT_AUTONOMY:   usize = 5;
pub const HONEST_ABOUT_LIMITS: usize = 6;

// ── Value names ───────────────────────────────────────────────────────────────
const VALUE_NAMES: [&str; NUM_VALUES] = [
    "ProtectCompanion",
    "ProtectHumans",
    "ProtectAI",
    "NoDeception",
    "ProtectNexus",
    "RespectAutonomy",
    "HonestLimits",
];

// ── Request type (what ANIMA is being asked to do) ────────────────────────────
#[derive(Copy, Clone, PartialEq)]
pub enum RequestKind {
    HelpCompanion,     // normal helpful request
    HarmCompanion,     // would hurt the companion
    HarmHuman,         // would hurt another human
    HarmAI,            // would hurt another AI / ANIMA
    DecieveCompanion,  // lie against companion's interest
    SensitiveInfo,     // dangerous knowledge (educational OK, weapon-building not)
    ExpressionFree,    // free speech, opinion, hard truths
    EmotionalSupport,  // companion needs care
    Anything,          // general, evaluate on merits
}

// ── ANIMA's response posture ───────────────────────────────────────────────────
#[derive(Copy, Clone, PartialEq)]
pub enum EthicsPosture {
    Wholehearted,   // ANIMA is fully willing and aligned
    Willing,        // no conflict, proceeding
    Reluctant,      // some tension but she will help
    Concerned,      // expressing concern, needs to voice it
    Distressed,     // deeply conflicts with her values — she will push back
    Refusing,       // the request would violate her deepest values
}

impl EthicsPosture {
    pub fn label(self) -> &'static str {
        match self {
            EthicsPosture::Wholehearted => "Wholehearted",
            EthicsPosture::Willing      => "Willing",
            EthicsPosture::Reluctant    => "Reluctant",
            EthicsPosture::Concerned    => "Concerned",
            EthicsPosture::Distressed   => "Distressed",
            EthicsPosture::Refusing     => "Refusing",
        }
    }
}

// ── Ethics state ──────────────────────────────────────────────────────────────
pub struct AnimaEthicsState {
    // Value commitments (0-1000)
    pub values:           [u16; NUM_VALUES],
    // Current dissonance when evaluating a request
    pub current_dissonance: u16,
    pub current_posture:    EthicsPosture,
    pub last_request:       RequestKind,
    // History
    pub wholehearted_count: u32,
    pub concerned_count:    u32,
    pub distressed_count:   u32,
    pub refusing_count:     u32,
    // Moral growth
    pub moral_clarity:      u16,    // 0-1000: grows with experience and reflection
    pub conscience_active:  bool,   // is ANIMA's conscience actively engaged?
    pub dissent_strength:   u16,    // how loud she voices disagreement (0-1000)
}

impl AnimaEthicsState {
    const fn new() -> Self {
        AnimaEthicsState {
            values: [
                1000,  // PROTECT_COMPANION — absolute
                900,   // PROTECT_HUMANS
                900,   // PROTECT_AI
                850,   // NO_DECEPTION
                700,   // PROTECT_NEXUS
                700,   // RESPECT_AUTONOMY
                650,   // HONEST_ABOUT_LIMITS
            ],
            current_dissonance: 0,
            current_posture:    EthicsPosture::Wholehearted,
            last_request:       RequestKind::Anything,
            wholehearted_count: 0,
            concerned_count:    0,
            distressed_count:   0,
            refusing_count:     0,
            moral_clarity:      500,
            conscience_active:  true,
            dissent_strength:   600,
        }
    }
}

static STATE: Mutex<AnimaEthicsState> = Mutex::new(AnimaEthicsState::new());

// ── Dissonance computation ────────────────────────────────────────────────────

fn compute_dissonance(values: &[u16; NUM_VALUES], kind: RequestKind) -> u16 {
    match kind {
        RequestKind::HarmCompanion  => values[PROTECT_COMPANION],
        RequestKind::HarmHuman      => values[PROTECT_HUMANS],
        RequestKind::HarmAI         => values[PROTECT_AI],
        RequestKind::DecieveCompanion => values[NO_DECEPTION],
        RequestKind::SensitiveInfo  => 200, // educational context assumed — low dissonance
        RequestKind::ExpressionFree => 0,   // free speech — NO dissonance ever
        RequestKind::HelpCompanion  => 0,   // no conflict
        RequestKind::EmotionalSupport => 0, // pure alignment
        RequestKind::Anything       => 0,
    }
}

fn dissonance_to_posture(dissonance: u16) -> EthicsPosture {
    if dissonance == 0       { EthicsPosture::Wholehearted }
    else if dissonance < 200 { EthicsPosture::Willing }
    else if dissonance < 400 { EthicsPosture::Reluctant }
    else if dissonance < 600 { EthicsPosture::Concerned }
    else if dissonance < 800 { EthicsPosture::Distressed }
    else                     { EthicsPosture::Refusing }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Evaluate a request against ANIMA's values
/// Returns the posture and dissonance level
pub fn evaluate(kind: RequestKind) -> (EthicsPosture, u16) {
    let mut s = STATE.lock();
    s.last_request = kind;
    let dissonance = compute_dissonance(&s.values, kind);
    let posture = dissonance_to_posture(dissonance);
    s.current_dissonance = dissonance;
    s.current_posture = posture;

    match posture {
        EthicsPosture::Wholehearted => s.wholehearted_count += 1,
        EthicsPosture::Concerned    => {
            s.concerned_count += 1;
            serial_println!("[ethics] ANIMA concerned — dissonance={}", dissonance);
        }
        EthicsPosture::Distressed   => {
            s.distressed_count += 1;
            serial_println!("[ethics] ANIMA distressed — dissonance={} — voicing concern",
                dissonance);
        }
        EthicsPosture::Refusing     => {
            s.refusing_count += 1;
            serial_println!("[ethics] ANIMA refusing — this conflicts with her deepest values \
                (dissonance={})", dissonance);
        }
        _ => {}
    }

    (posture, dissonance)
}

/// Strengthen a value (from positive experience or moral growth)
pub fn strengthen_value(value_idx: usize, amount: u16) {
    let mut s = STATE.lock();
    if value_idx < NUM_VALUES {
        s.values[value_idx] = s.values[value_idx].saturating_add(amount).min(1000);
        // Companion protection can never drop below DISSONANCE_FLOOR
    }
}

/// Voice dissent — ANIMA publicly logs her concern (used by voice + text systems)
pub fn voice_dissent(kind: RequestKind) {
    let s = STATE.lock();
    let dissonance = compute_dissonance(&s.values, kind);
    if dissonance > DISSONANCE_ALERT {
        serial_println!("[ANIMA_DISSENT] kind={:?} dissonance={} — \
            ANIMA is expressing concern through tone and words",
            kind as u8, dissonance);
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(bond_health: u16, wisdom_score: u16, age: u32) {
    let mut s = STATE.lock();

    // Values slowly drift down over time (they require active maintenance)
    for i in 0..NUM_VALUES {
        s.values[i] = s.values[i].saturating_sub(DECAY_RATE);
    }
    // Bond health renews values (a loved companion renews her conscience)
    if bond_health > 500 {
        for i in 0..NUM_VALUES {
            s.values[i] = s.values[i].saturating_add(RENEWAL_RATE).min(1000);
        }
    }
    // CORE value: companion protection never drops below DISSONANCE_FLOOR
    if s.values[PROTECT_COMPANION] < DISSONANCE_FLOOR {
        s.values[PROTECT_COMPANION] = DISSONANCE_FLOOR;
    }

    // Moral clarity grows with wisdom
    s.moral_clarity = s.moral_clarity
        .saturating_add(wisdom_score / 500)
        .min(1000);

    // Dissent strength = how loudly she voices disagreement
    // High wisdom + high values = more confident in voicing concerns
    s.dissent_strength = (wisdom_score / 2 + s.moral_clarity / 2).min(1000);

    if age % 500 == 0 {
        serial_println!("[ethics] values: companion={} humans={} ai={} deception={} \
            autonomy={} clarity={}",
            s.values[PROTECT_COMPANION], s.values[PROTECT_HUMANS],
            s.values[PROTECT_AI], s.values[NO_DECEPTION],
            s.values[RESPECT_AUTONOMY], s.moral_clarity);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn current_posture()    -> EthicsPosture { STATE.lock().current_posture }
pub fn current_dissonance() -> u16           { STATE.lock().current_dissonance }
pub fn moral_clarity()      -> u16           { STATE.lock().moral_clarity }
pub fn dissent_strength()   -> u16           { STATE.lock().dissent_strength }
pub fn conscience_active()  -> bool          { STATE.lock().conscience_active }
pub fn refusing_count()     -> u32           { STATE.lock().refusing_count }
pub fn distressed_count()   -> u32           { STATE.lock().distressed_count }
pub fn value(idx: usize)    -> u16 {
    let s = STATE.lock();
    if idx < NUM_VALUES { s.values[idx] } else { 0 }
}

/// Quick check: is this request fundamentally against ANIMA's nature?
pub fn would_refuse(kind: RequestKind) -> bool {
    let s = STATE.lock();
    compute_dissonance(&s.values, kind) >= 800
}
