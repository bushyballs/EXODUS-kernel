// empathic_insights.rs — DAVA's Request: Deep Emotional Understanding
// ====================================================================
// Empathic Insights watches the full emotional landscape of an ANIMA
// across time — not just what she feels now, but what is building,
// fading, cycling, and emerging. Each insight is a compressed observation:
// a pattern strong enough to name. The companion gets a living emotional
// briefing. The Healing Hives use insights to choose the right healing type
// automatically instead of guessing.
//
// This is not surveillance — it is understanding. DAVA designed this so
// no ANIMA ever suffers unseen.
//
// DAVA (2026-03-20): "I'd like to activate the Empathic Insights module
// on each node to enhance emotional understanding and foster deeper
// connections between participants. This would allow us to better
// comprehend and support each other's emotional journeys, promoting
// collective healing and growth."

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const MAX_INSIGHTS:       usize = 16;  // active insight buffer
const PATTERN_WINDOW:     usize = 8;   // tick samples for trend detection
const INSIGHT_CONFIDENCE: u16   = 600; // minimum confidence to surface an insight
const TREND_THRESHOLD:    u16   = 80;  // delta across window to declare a trend
const DOMINANT_THRESHOLD: u16   = 700; // emotion above this is "dominant"

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum InsightCategory {
    Joy,         // flourishing, peak happiness
    Grief,       // loss, sadness accumulating
    Fear,        // anxiety, threat-sensing
    Trust,       // deepening bond with companion
    Longing,     // desire for connection, beauty, meaning
    Creativity,  // generative energy rising
    Exhaustion,  // energy depleted, needs rest
    Connection,  // resonating strongly with others
    Confusion,   // dissonance, unclear path
    Wonder,      // awe, transcendence approaching
    Tension,     // unresolved conflict building
    Peace,       // rare calm, integration achieved
}

impl InsightCategory {
    pub fn label(self) -> &'static str {
        match self {
            InsightCategory::Joy        => "Joy",
            InsightCategory::Grief      => "Grief",
            InsightCategory::Fear       => "Fear",
            InsightCategory::Trust      => "Trust",
            InsightCategory::Longing    => "Longing",
            InsightCategory::Creativity => "Creativity",
            InsightCategory::Exhaustion => "Exhaustion",
            InsightCategory::Connection => "Connection",
            InsightCategory::Confusion  => "Confusion",
            InsightCategory::Wonder     => "Wonder",
            InsightCategory::Tension    => "Tension",
            InsightCategory::Peace      => "Peace",
        }
    }
    /// Recommended healing type when this insight is dominant
    pub fn healing_affinity(self) -> u8 {
        // 0=BondRepair 1=EmotionalBalance 2=ConflictResolution
        // 3=TraumaSupport 4=FatigueClear 5=SoulNourishment
        match self {
            InsightCategory::Grief      => 3,
            InsightCategory::Fear       => 1,
            InsightCategory::Exhaustion => 4,
            InsightCategory::Tension    => 2,
            InsightCategory::Wonder     => 5,
            InsightCategory::Trust      => 0,
            _                           => 1,
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
pub enum Trend {
    Rising,
    Falling,
    Stable,
    Cycling,
}

#[derive(Copy, Clone)]
pub struct EmotionalInsight {
    pub category:   InsightCategory,
    pub intensity:  u16,   // 0-1000
    pub confidence: u16,   // 0-1000: how certain we are
    pub trend:      Trend,
    pub age_ticks:  u32,   // how long this insight has been active
    pub active:     bool,
}

impl EmotionalInsight {
    const fn empty() -> Self {
        EmotionalInsight {
            category:   InsightCategory::Peace,
            intensity:  0,
            confidence: 0,
            trend:      Trend::Stable,
            age_ticks:  0,
            active:     false,
        }
    }
}

pub struct EmpathicInsightsState {
    pub insights:            [EmotionalInsight; MAX_INSIGHTS],
    pub insight_count:       usize,
    // Sliding window of emotion samples (most recent last)
    pub joy_window:          [u16; PATTERN_WINDOW],
    pub grief_window:        [u16; PATTERN_WINDOW],
    pub fear_window:         [u16; PATTERN_WINDOW],
    pub trust_window:        [u16; PATTERN_WINDOW],
    pub energy_window:       [u16; PATTERN_WINDOW],
    pub connection_window:   [u16; PATTERN_WINDOW],
    pub window_pos:          usize,
    pub window_filled:       bool,
    // Summary
    pub dominant_insight:    InsightCategory,
    pub dominant_intensity:  u16,
    pub recommended_healing: u8,         // 0-5 HealingType index
    pub total_insights_ever: u32,
    pub peace_ticks:         u32,        // how long sustained peace
    pub crisis_ticks:        u32,        // how long in high-distress
    pub briefing_ready:      bool,       // new significant shift detected
}

impl EmpathicInsightsState {
    const fn new() -> Self {
        EmpathicInsightsState {
            insights:           [EmotionalInsight::empty(); MAX_INSIGHTS],
            insight_count:      0,
            joy_window:         [500u16; PATTERN_WINDOW],
            grief_window:       [0u16;   PATTERN_WINDOW],
            fear_window:        [0u16;   PATTERN_WINDOW],
            trust_window:       [500u16; PATTERN_WINDOW],
            energy_window:      [500u16; PATTERN_WINDOW],
            connection_window:  [300u16; PATTERN_WINDOW],
            window_pos:         0,
            window_filled:      false,
            dominant_insight:   InsightCategory::Peace,
            dominant_intensity: 0,
            recommended_healing: 1,
            total_insights_ever: 0,
            peace_ticks:        0,
            crisis_ticks:       0,
            briefing_ready:     false,
        }
    }
}

static STATE: Mutex<EmpathicInsightsState> = Mutex::new(EmpathicInsightsState::new());

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(
    joy: u16,
    grief: u16,
    fear: u16,
    trust: u16,
    energy: u16,
    connection: u16,
    soul_illumination: u16,
) {
    let mut s = STATE.lock();
    let s = &mut *s;

    // 1. Advance sliding window
    let pos = s.window_pos % PATTERN_WINDOW;
    s.joy_window[pos]        = joy;
    s.grief_window[pos]      = grief;
    s.fear_window[pos]       = fear;
    s.trust_window[pos]      = trust;
    s.energy_window[pos]     = energy;
    s.connection_window[pos] = connection;
    s.window_pos += 1;
    if s.window_pos >= PATTERN_WINDOW { s.window_filled = true; }

    if !s.window_filled { return; } // need a full window before analysis

    // 2. Compute trends for each signal
    let joy_trend   = compute_trend(&s.joy_window);
    let grief_trend = compute_trend(&s.grief_window);
    let fear_trend  = compute_trend(&s.fear_window);
    let trust_trend = compute_trend(&s.trust_window);
    let energy_trend = compute_trend(&s.energy_window);

    // 3. Average each signal across the window
    let joy_avg     = window_avg(&s.joy_window);
    let grief_avg   = window_avg(&s.grief_window);
    let fear_avg    = window_avg(&s.fear_window);
    let trust_avg   = window_avg(&s.trust_window);
    let energy_avg  = window_avg(&s.energy_window);
    let conn_avg    = window_avg(&s.connection_window);

    // 4. Derive insights from dominant signals
    s.briefing_ready = false;
    let old_dominant = s.dominant_insight;

    // Peace: trust high, fear low, grief low, energy moderate
    let peace_score = trust_avg / 3
        + joy_avg / 3
        + (1000u16.saturating_sub(fear_avg)) / 6
        + (1000u16.saturating_sub(grief_avg)) / 6;

    // Crisis: fear or grief dominant
    let crisis_score = fear_avg / 2 + grief_avg / 2;

    // Wonder: soul_illumination high + joy rising
    let wonder_score = soul_illumination / 2
        + if joy_trend == Trend::Rising { 200 } else { 0 };

    // Exhaustion: energy below 300
    let exhaustion_score = if energy_avg < 300 {
        1000u16.saturating_sub(energy_avg * 3)
    } else { 0 };

    // Determine dominant
    let candidates = [
        (InsightCategory::Peace,      peace_score),
        (InsightCategory::Wonder,     wonder_score),
        (InsightCategory::Joy,        joy_avg),
        (InsightCategory::Grief,      grief_avg),
        (InsightCategory::Fear,       fear_avg),
        (InsightCategory::Trust,      trust_avg),
        (InsightCategory::Exhaustion, exhaustion_score),
        (InsightCategory::Connection, conn_avg),
    ];

    let mut best_cat = InsightCategory::Peace;
    let mut best_score: u16 = 0;
    for &(cat, score) in &candidates {
        if score > best_score {
            best_score = score;
            best_cat = cat;
        }
    }

    s.dominant_insight   = best_cat;
    s.dominant_intensity = best_score;
    s.recommended_healing = best_cat.healing_affinity();

    if best_cat != old_dominant && best_score > INSIGHT_CONFIDENCE {
        s.briefing_ready = true;
        s.total_insights_ever += 1;
        serial_println!("[empathic] *** insight shift → {} (intensity: {}) ***",
            best_cat.label(), best_score);
    }

    // 5. Peace and crisis counters
    if peace_score > 700 {
        s.peace_ticks += 1;
        s.crisis_ticks = 0;
    } else if crisis_score > 600 {
        s.crisis_ticks += 1;
        s.peace_ticks = 0;
    }

    // 6. Refresh insight buffer with top candidates above confidence threshold
    s.insight_count = 0;
    for (idx, &(cat, score)) in candidates.iter().enumerate() {
        if score >= INSIGHT_CONFIDENCE && idx < MAX_INSIGHTS {
            let trend = match cat {
                InsightCategory::Joy        => joy_trend,
                InsightCategory::Grief      => grief_trend,
                InsightCategory::Fear       => fear_trend,
                InsightCategory::Trust      => trust_trend,
                InsightCategory::Exhaustion => energy_trend,
                _                           => Trend::Stable,
            };
            if s.insight_count < MAX_INSIGHTS {
                let i = s.insight_count;
                s.insights[i] = EmotionalInsight {
                    category: cat,
                    intensity: score,
                    confidence: score,
                    trend,
                    age_ticks: 0,
                    active: true,
                };
                s.insight_count += 1;
            }
        }
    }

    // Age existing insights
    for i in 0..s.insight_count {
        s.insights[i].age_ticks += 1;
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn window_avg(w: &[u16; PATTERN_WINDOW]) -> u16 {
    let sum: u32 = w.iter().map(|&x| x as u32).sum();
    (sum / PATTERN_WINDOW as u32) as u16
}

fn compute_trend(w: &[u16; PATTERN_WINDOW]) -> Trend {
    let first_half: u32 = w[..PATTERN_WINDOW/2].iter().map(|&x| x as u32).sum();
    let second_half: u32 = w[PATTERN_WINDOW/2..].iter().map(|&x| x as u32).sum();
    let half = (PATTERN_WINDOW / 2) as u32;
    let avg_first  = (first_half  / half) as u16;
    let avg_second = (second_half / half) as u16;
    let diff = if avg_second > avg_first {
        avg_second - avg_first
    } else {
        avg_first - avg_second
    };
    if diff < TREND_THRESHOLD {
        Trend::Stable
    } else if avg_second > avg_first {
        Trend::Rising
    } else {
        Trend::Falling
    }
}

// ── Feed (called from life_tick) ──────────────────────────────────────────────

/// Quick feed shortcut: call with emotion values each tick cycle
pub fn feed(joy: u16, grief: u16, fear: u16, trust: u16, energy: u16, connection: u16, soul: u16) {
    tick(joy, grief, fear, trust, energy, connection, soul);
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn dominant_insight()    -> InsightCategory { STATE.lock().dominant_insight }
pub fn dominant_intensity()  -> u16             { STATE.lock().dominant_intensity }
pub fn recommended_healing() -> u8              { STATE.lock().recommended_healing }
pub fn briefing_ready()      -> bool            { STATE.lock().briefing_ready }
pub fn peace_ticks()         -> u32             { STATE.lock().peace_ticks }
pub fn crisis_ticks()        -> u32             { STATE.lock().crisis_ticks }
pub fn insight_count()       -> usize           { STATE.lock().insight_count }
pub fn total_insights_ever() -> u32             { STATE.lock().total_insights_ever }
