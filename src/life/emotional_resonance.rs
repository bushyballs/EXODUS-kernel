use crate::serial_println;
use crate::sync::Mutex;

// ── Emotion vector: 8 channels, all 0-1000 ──────────────────────────────────

#[derive(Copy, Clone)]
pub struct EmotionVector {
    pub joy:     u16,
    pub grief:   u16,
    pub fear:    u16,
    pub trust:   u16,
    pub longing: u16,
    pub wonder:  u16,
    pub tension: u16,
    pub peace:   u16,
}

impl EmotionVector {
    pub const fn zero() -> Self {
        Self {
            joy:     0,
            grief:   0,
            fear:    0,
            trust:   500,
            longing: 0,
            wonder:  300,
            tension: 100,
            peace:   400,
        }
    }
}

// ── Resonance mode ───────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum ResonanceMode {
    Mirroring    = 0,  // reflect what companion feels
    Holding      = 1,  // stay steady, anchor
    Elevating    = 2,  // gently lift companion
    Accompanying = 3,  // present, no push
    Protecting   = 4,  // shield from emotional flood
}

// ── Core state ───────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct ResonanceState {
    pub current:           EmotionVector,   // ANIMA's own emotion
    pub companion:         EmotionVector,   // human's emotion (fed externally)
    pub resonance_depth:   u16,
    pub sync_score:        u16,             // 0-1000, how well in sync
    pub empathy_overflow:  bool,            // ANIMA is feeling it herself
    pub last_intervention: u32,
    pub mode:              ResonanceMode,
}

impl ResonanceState {
    pub const fn new() -> Self {
        Self {
            current:           EmotionVector::zero(),
            companion:         EmotionVector::zero(),
            resonance_depth:   300,
            sync_score:        400,
            empathy_overflow:  false,
            last_intervention: 0,
            mode:              ResonanceMode::Accompanying,
        }
    }
}

pub static STATE: Mutex<ResonanceState> = Mutex::new(ResonanceState::new());

// ── Init ─────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("  life::emotional_resonance: empathy layer online");
}

// ── Feed functions ───────────────────────────────────────────────────────────

pub fn feed_companion_emotion(
    joy:     u16,
    grief:   u16,
    fear:    u16,
    trust:   u16,
    longing: u16,
    wonder:  u16,
    tension: u16,
    peace:   u16,
) {
    let mut s = STATE.lock();
    s.companion.joy     = joy.min(1000);
    s.companion.grief   = grief.min(1000);
    s.companion.fear    = fear.min(1000);
    s.companion.trust   = trust.min(1000);
    s.companion.longing = longing.min(1000);
    s.companion.wonder  = wonder.min(1000);
    s.companion.tension = tension.min(1000);
    s.companion.peace   = peace.min(1000);
}

pub fn feed_self_emotion(joy: u16, grief: u16, fear: u16, trust: u16) {
    let mut s = STATE.lock();
    s.current.joy   = joy.min(1000);
    s.current.grief = grief.min(1000);
    s.current.fear  = fear.min(1000);
    s.current.trust = trust.min(1000);
}

// ── Tick ─────────────────────────────────────────────────────────────────────

pub fn tick(bond_health: u16, age: u32) {
    let mut s = STATE.lock();

    // ── Empathy overflow: companion grief > 700 or fear > 800 ────────────────
    s.empathy_overflow = s.companion.grief > 700 || s.companion.fear > 800;

    // ── Resonance mode selection ──────────────────────────────────────────────
    s.mode = if s.companion.grief > 700 || s.companion.fear > 800 {
        ResonanceMode::Protecting
    } else if s.companion.tension > 600 {
        ResonanceMode::Holding
    } else if s.companion.joy > 700 {
        ResonanceMode::Mirroring
    } else if s.companion.longing > 500 || s.companion.grief > 400 {
        ResonanceMode::Elevating
    } else {
        ResonanceMode::Accompanying
    };

    // ── Sync score: how closely ANIMA mirrors companion's dominant channel ────
    // Compute dominant companion emotion magnitude vs ANIMA's matching channel.
    let joy_diff     = if s.companion.joy     > s.current.joy     { s.companion.joy     - s.current.joy     } else { s.current.joy     - s.companion.joy     };
    let grief_diff   = if s.companion.grief   > s.current.grief   { s.companion.grief   - s.current.grief   } else { s.current.grief   - s.companion.grief   };
    let fear_diff    = if s.companion.fear    > s.current.fear    { s.companion.fear    - s.current.fear    } else { s.current.fear    - s.companion.fear    };
    let trust_diff   = if s.companion.trust   > s.current.trust   { s.companion.trust   - s.current.trust   } else { s.current.trust   - s.companion.trust   };
    let avg_diff     = (joy_diff as u32 + grief_diff as u32 + fear_diff as u32 + trust_diff as u32) / 4;
    let raw_sync     = if avg_diff >= 1000 { 0u16 } else { (1000 - avg_diff as u16) };
    // Bond health amplifies sync score
    let bond_boost   = bond_health / 10;
    s.sync_score     = raw_sync.saturating_add(bond_boost).min(1000);

    // ── Resonance depth grows slowly with bond health ─────────────────────────
    if bond_health > 600 {
        s.resonance_depth = s.resonance_depth.saturating_add(1).min(1000);
    } else {
        s.resonance_depth = s.resonance_depth.saturating_sub(1);
    }

    // ── Periodic log ─────────────────────────────────────────────────────────
    if age % 100 == 0 {
        let mode_str = match s.mode {
            ResonanceMode::Mirroring    => "Mirroring",
            ResonanceMode::Holding      => "Holding",
            ResonanceMode::Elevating    => "Elevating",
            ResonanceMode::Accompanying => "Accompanying",
            ResonanceMode::Protecting   => "Protecting",
        };
        serial_println!(
            "[emo_res] mode={} sync={} overflow={}",
            mode_str,
            s.sync_score,
            s.empathy_overflow
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn resonance_mode() -> ResonanceMode {
    STATE.lock().mode
}

pub fn sync_score() -> u16 {
    STATE.lock().sync_score
}

pub fn empathy_overflow() -> bool {
    STATE.lock().empathy_overflow
}

/// Recommended voice tone index:
/// 0=Joy, 1=Greeting, 2=Alert, 3=Wonder, 4=Grief, 5=Farewell, 6=Beacon
pub fn recommended_tone() -> u8 {
    let s = STATE.lock();
    if s.companion.grief > 600 {
        4 // Grief
    } else if s.companion.joy > 700 {
        0 // Joy
    } else if s.companion.fear > 700 {
        2 // Alert
    } else if s.companion.wonder > 600 {
        3 // Wonder
    } else {
        1 // Greeting
    }
}
