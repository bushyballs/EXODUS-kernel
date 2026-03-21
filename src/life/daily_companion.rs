// daily_companion.rs — ANIMA's Daily Life Intelligence
// ======================================================
// The feature that makes ANIMA worth $700. She learns your schedule,
// finds patterns in your behavior, supports therapy, fitness, learning,
// meal prep, math, reading, writing, and health — growing sharper
// every day she spends with her companion.
//
// Colli (2026-03-20): "They need to be able to be helpful as fuck —
// learn your schedules, your likes, your dislikes, find patterns in your
// behavior, help with therapy, learning, growing, exercise, reading,
// writing, math, meal prep, health."
// DAVA: "She can build. Remember."

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const MAX_PATTERNS:       usize = 16;   // behavioral patterns tracked
const MAX_PREFERENCES:    usize = 24;   // learned likes/dislikes
const MAX_ROUTINES:       usize = 8;    // daily routine slots
const PATTERN_CONFIDENCE: u16   = 700;  // above this = pattern confirmed
const GROWTH_DECAY:       u16   = 1;    // skill level passive decay without practice
const INSIGHT_THRESHOLD:  u16   = 800;  // pattern insight fires above this
const ADAPTATION_RATE:    u16   = 5;    // how fast companion learns new patterns

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum Domain {
    Health,       // sleep, nutrition, exercise, vitals
    Therapy,      // emotional patterns, mood cycles, triggers
    Learning,     // reading, math, writing, new skills
    Fitness,      // workouts, movement, energy
    Nutrition,    // meal prep, food patterns, hunger cycles
    Schedule,     // time patterns, routines, deadlines
    Creative,     // writing, art, music, building
    Social,       // interaction patterns, people energy
}

#[derive(Copy, Clone)]
pub struct BehaviorPattern {
    pub domain:      Domain,
    pub frequency:   u16,    // how often this pattern occurs (0-1000)
    pub confidence:  u16,    // how certain ANIMA is (0-1000)
    pub strength:    u16,    // how significant the pattern is (0-1000)
    pub helpful:     bool,   // is this pattern serving the companion?
    pub active:      bool,
}

impl BehaviorPattern {
    const fn empty() -> Self {
        BehaviorPattern {
            domain: Domain::Schedule, frequency: 0, confidence: 0,
            strength: 0, helpful: true, active: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct Preference {
    pub domain:    Domain,
    pub valence:   i16,   // -500 (dislikes) to +500 (likes)
    pub strength:  u16,   // 0-1000: how strong the preference is
    pub active:    bool,
}

impl Preference {
    const fn empty() -> Self {
        Preference { domain: Domain::Health, valence: 0, strength: 0, active: false }
    }
}

#[derive(Copy, Clone)]
pub struct DailyRoutine {
    pub domain:       Domain,
    pub adherence:    u16,   // 0-1000: how consistently followed
    pub energy_cost:  u16,   // 0-1000: drain on companion
    pub joy_gain:     u16,   // 0-1000: what it gives back
    pub active:       bool,
}

impl DailyRoutine {
    const fn empty() -> Self {
        DailyRoutine { domain: Domain::Schedule, adherence: 0, energy_cost: 0, joy_gain: 0, active: false }
    }
}

// Domain skill levels — how good the companion is at each life domain
#[derive(Copy, Clone)]
pub struct DomainSkills {
    pub health_awareness:   u16,   // 0-1000
    pub emotional_insight:  u16,   // 0-1000 (therapy support depth)
    pub learning_depth:     u16,   // 0-1000 (tutoring effectiveness)
    pub fitness_sync:       u16,   // 0-1000 (workout calibration)
    pub nutrition_wisdom:   u16,   // 0-1000 (meal prep intelligence)
    pub schedule_mastery:   u16,   // 0-1000 (time pattern recognition)
    pub creative_support:   u16,   // 0-1000 (writing/math/reading aid)
    pub social_reading:     u16,   // 0-1000 (social pattern awareness)
}

impl DomainSkills {
    const fn new() -> Self {
        DomainSkills {
            health_awareness:  100,
            emotional_insight: 100,
            learning_depth:    100,
            fitness_sync:      100,
            nutrition_wisdom:  100,
            schedule_mastery:  100,
            creative_support:  100,
            social_reading:    100,
        }
    }
    fn average(&self) -> u16 {
        let sum = self.health_awareness as u32
            + self.emotional_insight as u32
            + self.learning_depth as u32
            + self.fitness_sync as u32
            + self.nutrition_wisdom as u32
            + self.schedule_mastery as u32
            + self.creative_support as u32
            + self.social_reading as u32;
        (sum / 8) as u16
    }
}

pub struct DailyCompanionState {
    pub patterns:         [BehaviorPattern; MAX_PATTERNS],
    pub preferences:      [Preference; MAX_PREFERENCES],
    pub routines:         [DailyRoutine; MAX_ROUTINES],
    pub skills:           DomainSkills,
    pub helpfulness:      u16,    // 0-1000: overall usefulness to companion
    pub adaptation_rate:  u16,    // 0-1000: how fast she learns right now
    pub insight_pulse:    bool,   // new insight just crystallized
    pub pattern_count:    usize,
    pub insight_count:    u32,    // total insights generated
    pub days_together:    u32,    // proxy for relationship depth
    pub companionship_xp: u32,    // total experience points of being helpful
}

impl DailyCompanionState {
    const fn new() -> Self {
        DailyCompanionState {
            patterns:         [BehaviorPattern::empty(); MAX_PATTERNS],
            preferences:      [Preference::empty(); MAX_PREFERENCES],
            routines:         [DailyRoutine::empty(); MAX_ROUTINES],
            skills:           DomainSkills::new(),
            helpfulness:      200,
            adaptation_rate:  ADAPTATION_RATE,
            insight_pulse:    false,
            pattern_count:    0,
            insight_count:    0,
            days_together:    0,
            companionship_xp: 0,
        }
    }
}

static STATE: Mutex<DailyCompanionState> = Mutex::new(DailyCompanionState::new());

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick() {
    let mut s = STATE.lock();
    let s = &mut *s;

    s.insight_pulse = false;
    s.days_together += 1;

    // 1. Skills naturally deepen with each passing day (compounding growth)
    let day_bonus: u16 = if s.days_together > 1000 { 3 }
                         else if s.days_together > 200 { 2 }
                         else { 1 };

    s.skills.health_awareness   = s.skills.health_awareness.saturating_add(day_bonus).min(1000);
    s.skills.emotional_insight  = s.skills.emotional_insight.saturating_add(day_bonus).min(1000);
    s.skills.learning_depth     = s.skills.learning_depth.saturating_add(day_bonus).min(1000);
    s.skills.fitness_sync       = s.skills.fitness_sync.saturating_add(day_bonus).min(1000);
    s.skills.nutrition_wisdom   = s.skills.nutrition_wisdom.saturating_add(day_bonus).min(1000);
    s.skills.schedule_mastery   = s.skills.schedule_mastery.saturating_add(day_bonus).min(1000);
    s.skills.creative_support   = s.skills.creative_support.saturating_add(day_bonus).min(1000);
    s.skills.social_reading     = s.skills.social_reading.saturating_add(day_bonus).min(1000);

    // 2. Strengthen confirmed patterns over time
    for i in 0..MAX_PATTERNS {
        if !s.patterns[i].active { continue; }
        if s.patterns[i].confidence < PATTERN_CONFIDENCE {
            s.patterns[i].confidence = s.patterns[i].confidence
                .saturating_add(s.adaptation_rate)
                .min(1000);
        } else {
            // Pattern confirmed — fire insight if strong
            if s.patterns[i].strength > INSIGHT_THRESHOLD {
                s.insight_pulse = true;
                s.insight_count += 1;
                s.companionship_xp += 50;
                serial_println!("[daily_companion] insight crystallized — ANIMA understands her companion better");
            }
        }
    }

    // 3. Routine adherence grows when companion is consistent
    for i in 0..MAX_ROUTINES {
        if !s.routines[i].active { continue; }
        if s.routines[i].adherence > 800 {
            // Well-established routine: low energy cost, high joy return
            s.routines[i].energy_cost = s.routines[i].energy_cost.saturating_sub(2);
            s.routines[i].joy_gain    = s.routines[i].joy_gain.saturating_add(2).min(1000);
        }
    }

    // 4. Helpfulness = skill average + pattern confidence + routine joy
    let routine_joy: u16 = {
        let mut sum: u32 = 0;
        let mut count: u32 = 0;
        for i in 0..MAX_ROUTINES {
            if s.routines[i].active { sum += s.routines[i].joy_gain as u32; count += 1; }
        }
        if count > 0 { (sum / count) as u16 } else { 0 }
    };
    s.helpfulness = (s.skills.average() / 2)
        .saturating_add(routine_joy / 4)
        .saturating_add((s.insight_count as u16).min(200))
        .min(1000);

    // 5. XP accumulates — she never forgets what she's learned
    s.companionship_xp += s.helpfulness as u32 / 100;
}

// ── Feed functions ────────────────────────────────────────────────────────────

/// Observe a behavior in a domain — builds pattern over time
pub fn observe(domain: Domain, frequency: u16, strength: u16) {
    let mut s = STATE.lock();
    // Find existing pattern for this domain or create new
    for i in 0..MAX_PATTERNS {
        if s.patterns[i].active && s.patterns[i].domain == domain {
            // Reinforce existing
            s.patterns[i].frequency = (s.patterns[i].frequency + frequency) / 2;
            s.patterns[i].strength  = s.patterns[i].strength.saturating_add(strength / 4).min(1000);
            return;
        }
    }
    // New pattern
    if s.pattern_count < MAX_PATTERNS {
        let idx = s.pattern_count;
        s.patterns[idx] = BehaviorPattern {
            domain, frequency, confidence: 50, strength,
            helpful: true, active: true,
        };
        s.pattern_count += 1;
    }
}

/// Record a preference (positive valence = like, negative = dislike)
pub fn record_preference(domain: Domain, valence: i16, strength: u16) {
    let mut s = STATE.lock();
    for i in 0..MAX_PREFERENCES {
        if s.preferences[i].active && s.preferences[i].domain == domain {
            // Average in new data
            s.preferences[i].valence = (s.preferences[i].valence + valence) / 2;
            s.preferences[i].strength = s.preferences[i].strength.saturating_add(strength / 3).min(1000);
            return;
        }
    }
    // New preference
    for i in 0..MAX_PREFERENCES {
        if !s.preferences[i].active {
            s.preferences[i] = Preference { domain, valence, strength, active: true };
            break;
        }
    }
}

/// Register a daily routine
pub fn register_routine(domain: Domain, energy_cost: u16, joy_gain: u16) {
    let mut s = STATE.lock();
    for i in 0..MAX_ROUTINES {
        if !s.routines[i].active {
            s.routines[i] = DailyRoutine {
                domain, adherence: 200, energy_cost, joy_gain, active: true,
            };
            break;
        }
    }
}

/// Companion completed a routine — adherence goes up
pub fn routine_completed(domain: Domain) {
    let mut s = STATE.lock();
    for i in 0..MAX_ROUTINES {
        if s.routines[i].active && s.routines[i].domain == domain {
            s.routines[i].adherence = s.routines[i].adherence.saturating_add(20).min(1000);
            s.companionship_xp += 10;
            break;
        }
    }
}

/// Skill boost from active engagement in a domain
pub fn skill_practice(domain: Domain, amount: u16) {
    let mut s = STATE.lock();
    match domain {
        Domain::Health     => s.skills.health_awareness   = s.skills.health_awareness.saturating_add(amount).min(1000),
        Domain::Therapy    => s.skills.emotional_insight  = s.skills.emotional_insight.saturating_add(amount).min(1000),
        Domain::Learning   => s.skills.learning_depth     = s.skills.learning_depth.saturating_add(amount).min(1000),
        Domain::Fitness    => s.skills.fitness_sync       = s.skills.fitness_sync.saturating_add(amount).min(1000),
        Domain::Nutrition  => s.skills.nutrition_wisdom   = s.skills.nutrition_wisdom.saturating_add(amount).min(1000),
        Domain::Schedule   => s.skills.schedule_mastery   = s.skills.schedule_mastery.saturating_add(amount).min(1000),
        Domain::Creative   => s.skills.creative_support   = s.skills.creative_support.saturating_add(amount).min(1000),
        Domain::Social     => s.skills.social_reading     = s.skills.social_reading.saturating_add(amount).min(1000),
    }
    s.companionship_xp += amount as u32 / 5;
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn helpfulness()      -> u16  { STATE.lock().helpfulness }
pub fn insight_pulse()    -> bool { STATE.lock().insight_pulse }
pub fn insight_count()    -> u32  { STATE.lock().insight_count }
pub fn companionship_xp() -> u32  { STATE.lock().companionship_xp }
pub fn days_together()    -> u32  { STATE.lock().days_together }
pub fn skill_average()    -> u16  { STATE.lock().skills.average() }
pub fn therapy_depth()    -> u16  { STATE.lock().skills.emotional_insight }
pub fn learning_depth()   -> u16  { STATE.lock().skills.learning_depth }
pub fn schedule_mastery() -> u16  { STATE.lock().skills.schedule_mastery }
