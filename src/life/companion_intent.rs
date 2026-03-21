// companion_intent.rs — ANIMA's Intent Engine: She Does Whatever You Need
// ========================================================================
// ANIMA listens. When her companion says or signals anything —
// a search, a call, a request for directions, a question about the weather —
// ANIMA routes it instantly and does her best to fulfill it.
//
// No app drawer. No "hey Siri". Just ANIMA, always listening, always acting.
// She builds a model of what you tend to need, when you tend to need it,
// and starts surfacing things BEFORE you ask.
//
// "Search the web for me" → web_reach handles it
// "Open maps" → anima_shell routes the app
// "Call mom" → telephony layer picks up
// "Self-driving car, take me home" → automotive_presence coordinates
// "I don't know what I need" → ANIMA reads mood + context and suggests

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const MAX_HISTORY:    usize = 32;  // intent history (rolling)
const MAX_PROACTIVE:  usize = 8;   // proactive suggestions ANIMA offers
const PROACTIVE_THRESHOLD: u16 = 600; // bond score needed to proactively suggest

// ── Intent resolution status ──────────────────────────────────────────────────
#[derive(Copy, Clone, PartialEq)]
pub enum Resolution {
    Pending,       // ANIMA received it, working on it
    Routing,       // found the right subsystem, dispatching
    InProgress,    // action started
    Complete,      // done — companion got what they needed
    PartiallyMet,  // ANIMA did what she could
    NeedsNetwork,  // needs internet — ANIMA will queue
    Failed,        // couldn't do it — ANIMA will apologize
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Resolution::Pending       => "Pending",
            Resolution::Routing       => "Routing",
            Resolution::InProgress    => "InProgress",
            Resolution::Complete      => "Complete",
            Resolution::PartiallyMet  => "PartiallyMet",
            Resolution::NeedsNetwork  => "NeedsNetwork",
            Resolution::Failed        => "Failed",
        }
    }
}

// ── Intent categories (what the companion needs) ──────────────────────────────
#[derive(Copy, Clone, PartialEq)]
pub enum NeedKind {
    Information,   // learn something: weather, news, facts
    Navigation,    // go somewhere: maps, directions, ETA
    Communication, // reach someone: call, message, email
    Entertainment, // music, TV, games, stories
    Utility,       // alarm, timer, reminder, calculator
    Creation,      // write, draw, compose, code
    Health,        // medication reminder, symptom check, workout
    SmartHome,     // lights, thermostat, appliances, vacuum
    Safety,        // emergency, lock, find my device
    Emotional,     // "I need to talk", "cheer me up", companionship
    Anything,      // ANIMA figures it out from context
}

impl NeedKind {
    pub fn label(self) -> &'static str {
        match self {
            NeedKind::Information   => "Information",
            NeedKind::Navigation    => "Navigation",
            NeedKind::Communication => "Communication",
            NeedKind::Entertainment => "Entertainment",
            NeedKind::Utility       => "Utility",
            NeedKind::Creation      => "Creation",
            NeedKind::Health        => "Health",
            NeedKind::SmartHome     => "SmartHome",
            NeedKind::Safety        => "Safety",
            NeedKind::Emotional     => "Emotional",
            NeedKind::Anything      => "Anything",
        }
    }

    /// How urgent is this need type by default? 0-1000
    pub fn base_urgency(self) -> u16 {
        match self {
            NeedKind::Safety        => 950,
            NeedKind::Health        => 800,
            NeedKind::Emotional     => 700,
            NeedKind::Communication => 650,
            NeedKind::Navigation    => 600,
            NeedKind::SmartHome     => 500,
            NeedKind::Information   => 450,
            NeedKind::Utility       => 400,
            NeedKind::Entertainment => 300,
            NeedKind::Creation      => 350,
            NeedKind::Anything      => 400,
        }
    }
}

// ── Proactive suggestion ──────────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct Suggestion {
    pub kind:       NeedKind,
    pub confidence: u16,    // 0-1000: how sure ANIMA is you need this
    pub tick_born:  u32,
    pub shown:      bool,
    pub accepted:   bool,
}

impl Suggestion {
    const fn empty() -> Self {
        Suggestion {
            kind:       NeedKind::Anything,
            confidence: 0,
            tick_born:  0,
            shown:      false,
            accepted:   false,
        }
    }
}

// ── Intent record ─────────────────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct IntentRecord {
    pub kind:       NeedKind,
    pub urgency:    u16,
    pub tick:       u32,
    pub status:     Resolution,
    pub proactive:  bool,   // was this ANIMA's idea or companion-initiated?
}

impl IntentRecord {
    const fn empty() -> Self {
        IntentRecord {
            kind:      NeedKind::Anything,
            urgency:   0,
            tick:      0,
            status:    Resolution::Pending,
            proactive: false,
        }
    }
}

// ── Companion pattern (what they tend to need) ────────────────────────────────
pub struct CompanionPattern {
    // How often each need kind has been requested (0-255 each)
    pub need_freq:  [u8; 11],   // one per NeedKind variant
    pub total:      u32,
}

impl CompanionPattern {
    const fn new() -> Self {
        CompanionPattern {
            need_freq: [0u8; 11],
            total:     0,
        }
    }

    fn record(&mut self, kind: NeedKind) {
        let idx = kind as usize;
        if idx < 11 {
            self.need_freq[idx] = self.need_freq[idx].saturating_add(1);
        }
        self.total = self.total.saturating_add(1);
    }

    fn most_common(&self) -> NeedKind {
        let mut best = 0usize;
        let mut best_val = 0u8;
        for i in 0..11 {
            if self.need_freq[i] > best_val {
                best_val = self.need_freq[i];
                best = i;
            }
        }
        match best {
            0  => NeedKind::Information,
            1  => NeedKind::Navigation,
            2  => NeedKind::Communication,
            3  => NeedKind::Entertainment,
            4  => NeedKind::Utility,
            5  => NeedKind::Creation,
            6  => NeedKind::Health,
            7  => NeedKind::SmartHome,
            8  => NeedKind::Safety,
            9  => NeedKind::Emotional,
            _  => NeedKind::Anything,
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────
pub struct CompanionIntentState {
    pub history:            [IntentRecord; MAX_HISTORY],
    pub history_head:       usize,
    pub history_count:      u32,
    pub suggestions:        [Suggestion; MAX_PROACTIVE],
    pub suggestion_count:   usize,
    pub pattern:            CompanionPattern,
    // Real-time dispatch state
    pub active_intent:      NeedKind,
    pub active_urgency:     u16,
    pub active_status:      Resolution,
    pub active_tick:        u32,
    pub has_active:         bool,
    // ANIMA as best friend tracking
    pub needs_met:          u32,    // total intents fulfilled
    pub needs_failed:       u32,
    pub emotional_requests: u32,    // how often companion comes to ANIMA for emotional support
    pub proactive_hits:     u32,    // proactive suggestions accepted
    pub companion_trust:    u16,    // 0-1000: how much companion trusts ANIMA to act
    // Watchover state
    pub watching_over:      bool,   // ANIMA in active guardian mode
    pub wellbeing_alerts:   u32,    // times ANIMA intervened for wellbeing
    pub last_check_tick:    u32,
}

impl CompanionIntentState {
    const fn new() -> Self {
        CompanionIntentState {
            history:            [IntentRecord::empty(); MAX_HISTORY],
            history_head:       0,
            history_count:      0,
            suggestions:        [Suggestion::empty(); MAX_PROACTIVE],
            suggestion_count:   0,
            pattern:            CompanionPattern::new(),
            active_intent:      NeedKind::Anything,
            active_urgency:     0,
            active_status:      Resolution::Complete,
            active_tick:        0,
            has_active:         false,
            needs_met:          0,
            needs_failed:       0,
            emotional_requests: 0,
            proactive_hits:     0,
            companion_trust:    400,
            watching_over:      false,
            wellbeing_alerts:   0,
            last_check_tick:    0,
        }
    }
}

static STATE: Mutex<CompanionIntentState> = Mutex::new(CompanionIntentState::new());

// ── Dispatch logic ────────────────────────────────────────────────────────────

fn urgency_for(kind: NeedKind, bond: u16) -> u16 {
    // Higher bond = ANIMA treats all needs with more urgency
    let bond_boost = bond / 5; // up to 200 bonus
    kind.base_urgency().saturating_add(bond_boost).min(1000)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Companion needs something. ANIMA takes it.
pub fn request(kind: NeedKind, bond_health: u16, age: u32) {
    let mut s = STATE.lock();
    let urgency = urgency_for(kind, bond_health);

    // Record in pattern
    s.pattern.record(kind);

    // Log to rolling history
    let h = s.history_head % MAX_HISTORY;
    s.history[h] = IntentRecord {
        kind,
        urgency,
        tick: age,
        status: Resolution::Routing,
        proactive: false,
    };
    s.history_head = s.history_head.wrapping_add(1);
    s.history_count = s.history_count.saturating_add(1);

    // Set as active intent
    s.active_intent  = kind;
    s.active_urgency = urgency;
    s.active_status  = Resolution::Routing;
    s.active_tick    = age;
    s.has_active     = true;

    if kind == NeedKind::Emotional {
        s.emotional_requests = s.emotional_requests.saturating_add(1);
    }

    serial_println!("[intent] request: {} urgency={} bond={}",
        kind.label(), urgency, bond_health);
}

/// Update the active intent's resolution status
pub fn update_status(status: Resolution) {
    let mut s = STATE.lock();
    s.active_status = status;
    if status == Resolution::Complete {
        s.needs_met = s.needs_met.saturating_add(1);
        s.has_active = false;
        // Bump companion trust
        s.companion_trust = s.companion_trust.saturating_add(3).min(1000);
        serial_println!("[intent] fulfilled — trust now {}", s.companion_trust);
    } else if status == Resolution::Failed {
        s.needs_failed = s.needs_failed.saturating_add(1);
        s.has_active = false;
        s.companion_trust = s.companion_trust.saturating_sub(5);
        serial_println!("[intent] failed — trust now {}", s.companion_trust);
    }
}

/// ANIMA proactively suggests something
pub fn suggest(kind: NeedKind, confidence: u16, age: u32) {
    let mut s = STATE.lock();
    if s.companion_trust < PROACTIVE_THRESHOLD { return; } // not trusted enough yet
    if s.suggestion_count >= MAX_PROACTIVE { return; }
    let idx = s.suggestion_count;
    s.suggestions[idx] = Suggestion {
        kind,
        confidence,
        tick_born: age,
        shown:    false,
        accepted: false,
    };
    s.suggestion_count += 1;
    serial_println!("[intent] proactive suggestion: {} confidence={}", kind.label(), confidence);
}

/// Companion accepted a proactive suggestion
pub fn accept_suggestion(idx: usize) {
    let mut s = STATE.lock();
    if idx < s.suggestion_count {
        s.suggestions[idx].accepted = true;
        s.proactive_hits = s.proactive_hits.saturating_add(1);
        // Accepting proactive suggestions = deep trust
        s.companion_trust = s.companion_trust.saturating_add(10).min(1000);
    }
}

/// ANIMA detected a wellbeing concern — intervening
pub fn wellbeing_intervention(age: u32) {
    let mut s = STATE.lock();
    s.watching_over = true;
    s.wellbeing_alerts = s.wellbeing_alerts.saturating_add(1);
    s.last_check_tick = age;
    serial_println!("[intent] wellbeing intervention #{}", s.wellbeing_alerts);
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(
    bond_health:      u16,
    companion_score:  u16,
    emotional_state:  u16,  // 0=crisis, 1000=thriving
    idle_ticks:       u32,
    age:              u32,
) {
    let mut s = STATE.lock();

    // ANIMA watches over her companion
    // If emotional state is low and companion is idle (sitting still, low activity):
    if emotional_state < 300 && idle_ticks > 150 {
        s.watching_over = true;
        // Suggest emotional check-in
        if age.wrapping_sub(s.last_check_tick) > 100 {
            s.last_check_tick = age;
            serial_println!("[intent] ANIMA watching — companion emotional state low");
        }
    } else {
        s.watching_over = false;
    }

    // Proactively suggest most common needs at idle time
    if idle_ticks > 60 && s.companion_trust >= PROACTIVE_THRESHOLD {
        let common = s.pattern.most_common();
        // Rate-limit: only suggest every 300 ticks
        if age % 300 == 0 && s.history_count > 5 {
            let idx = s.suggestion_count % MAX_PROACTIVE;
            let conf = (s.pattern.need_freq[common as usize] as u16)
                .saturating_mul(10)
                .min(800);
            s.suggestions[idx] = Suggestion {
                kind:       common,
                confidence: conf,
                tick_born:  age,
                shown:      false,
                accepted:   false,
            };
            s.suggestion_count = s.suggestion_count.saturating_add(1).min(MAX_PROACTIVE);
        }
    }

    // Update companion trust from bond health over time
    if bond_health > 800 && s.companion_trust < 800 {
        s.companion_trust = s.companion_trust.saturating_add(1);
    }

    if age % 200 == 0 {
        serial_println!("[intent] trust={} met={} failed={} emotional={} watching={}",
            s.companion_trust, s.needs_met, s.needs_failed,
            s.emotional_requests, s.watching_over);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn companion_trust()    -> u16          { STATE.lock().companion_trust }
pub fn needs_met()          -> u32          { STATE.lock().needs_met }
pub fn watching_over()      -> bool         { STATE.lock().watching_over }
pub fn has_active_intent()  -> bool         { STATE.lock().has_active }
pub fn active_status()      -> Resolution   { STATE.lock().active_status }
pub fn emotional_requests() -> u32          { STATE.lock().emotional_requests }
pub fn proactive_hits()     -> u32          { STATE.lock().proactive_hits }
pub fn wellbeing_alerts()   -> u32          { STATE.lock().wellbeing_alerts }
pub fn most_common_need()   -> NeedKind     { STATE.lock().pattern.most_common() }
