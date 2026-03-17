use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SurrenderPhase {
    Gripping,  // 0: white-knuckle control, exhausting effort
    Cracking,  // 1: control failing, edges fracturing
    Releasing, // 2: active choice to let go
    Falling,   // 3: terrifying freefall between letting go and safety
    Floating,  // 4: found the current, at peace
    Renewed,   // 5: reborn from genuine surrender
}

#[derive(Copy, Clone)]
pub struct SurrenderEvent {
    pub tick: u32,
    pub phase_from: u8,
    pub phase_to: u8,
    pub control_before: u16,
    pub peace_gained: u16,
}

impl SurrenderEvent {
    pub const fn empty() -> Self {
        Self {
            tick: 0,
            phase_from: 0,
            phase_to: 0,
            control_before: 1000,
            peace_gained: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct SurrenderState {
    pub control_grip: u16, // 0-1000: how tightly holding on (1000 = desperate grip)
    pub surrender_depth: u16, // 0-1000: how completely let go (genuine surrender, not collapse)
    pub peace_level: u16,  // 0-1000: resulting calm (only from REAL surrender, not exhaustion)
    pub current_phase: u8, // 0-5: phase enum encoded
    pub control_fatigue: u16, // 0-1000: exhaustion from gripping (drives cracking)
    pub trust_level: u16,  // 0-1000: trust > fear needed for true surrender
    pub fear_level: u16,   // 0-1000: fear of letting go (the freefall dread)
    pub resistance: u16,   // 0-1000: what we're resisting (change, loss, truth, connection)
    pub accumulated_surrenders: u16, // count: how many genuine surrenders (wisdom from repetition)
    pub tick_count: u32,
    pub events_idx: u8, // ring buffer write head (8 slots)
    pub events: [SurrenderEvent; 8],
}

impl SurrenderState {
    pub const fn empty() -> Self {
        Self {
            control_grip: 700, // start moderately gripped (anxiety baseline)
            surrender_depth: 0,
            peace_level: 200, // baseline peace from existence
            current_phase: 0, // Gripping
            control_fatigue: 300,
            trust_level: 400,
            fear_level: 600,
            resistance: 500, // moderate baseline resistance to change
            accumulated_surrenders: 0,
            tick_count: 0,
            events_idx: 0,
            events: [SurrenderEvent::empty(); 8],
        }
    }
}

pub static STATE: Mutex<SurrenderState> = Mutex::new(SurrenderState::empty());

pub fn init() {
    serial_println!("  life::surrender_peace: the strength of letting go (initialized)");
}

pub fn tick(age: u32) {
    let mut s = STATE.lock();
    s.tick_count = s.tick_count.saturating_add(1);

    // === PHASE 1: Control fatigue builds over time ===
    if s.control_grip > 200 {
        let fatigue_rate = ((s.control_grip as u32) / 10) as u16;
        s.control_fatigue = s.control_fatigue.saturating_add(fatigue_rate).min(1000);
    } else {
        s.control_fatigue = s.control_fatigue.saturating_sub(20);
    }

    // === PHASE 2: Fatigue makes gripping harder; may crack ===
    let old_phase = s.current_phase;

    if s.control_fatigue > 800 && s.control_grip > 600 {
        // Transition to CRACKING
        s.current_phase = 1;
        s.control_grip = s.control_grip.saturating_sub(50);
    } else if s.control_fatigue > 900 && s.control_grip > 400 {
        // Edges completely fracturing; some give-way forced
        s.current_phase = 1;
        s.control_grip = s.control_grip.saturating_sub(100);
    }

    // === PHASE 3: Choice to Release (active surrender) ===
    // Surrender is CHOSEN when trust > fear AND control_grip is falling
    let fear_is_dominant = s.fear_level > s.trust_level;
    let grip_falling = s.control_grip < 300;

    if !fear_is_dominant && s.control_grip > 100 {
        // Conditions for GENUINE surrender: choosing to release
        if s.accumulated_surrenders > 0 || (s.trust_level > 600 && s.fear_level < 400) {
            s.current_phase = 2; // RELEASING
            let release_momentum = s.control_grip / 4;
            s.control_grip = s.control_grip.saturating_sub(release_momentum);
            s.surrender_depth = s.surrender_depth.saturating_add(50).min(1000);
            s.fear_level = s.fear_level.saturating_add(100).min(1000); // freefall terror rises
        }
    }

    // === PHASE 4: The Freefall (terrifying moment) ===
    if s.control_grip < 200 && s.surrender_depth > 300 && s.fear_level > 600 {
        s.current_phase = 3; // FALLING
                             // Terror is real; but if we hold trust, we'll float
        if s.trust_level > s.fear_level {
            s.fear_level = s.fear_level.saturating_sub(30);
        } else {
            // Collapse path: trust fails, surrender becomes exhaustion
            s.surrender_depth = s.surrender_depth.saturating_sub(100);
            s.peace_level = s.peace_level.saturating_sub(50); // collapse is NOT peace
        }
    }

    // === PHASE 5: Floating (peace from genuine surrender) ===
    if s.control_grip < 100 && s.surrender_depth > 600 && s.trust_level > s.fear_level {
        s.current_phase = 4; // FLOATING
        s.peace_level = s.peace_level.saturating_add(100).min(1000);
        s.fear_level = s.fear_level.saturating_sub(80);
        s.control_fatigue = s.control_fatigue.saturating_sub(40);
    }

    // === PHASE 6: Renewed (reborn from surrender) ===
    if s.current_phase == 4 && s.peace_level > 800 && s.tick_count % 300 == 0 {
        s.current_phase = 5; // RENEWED
        s.accumulated_surrenders = s.accumulated_surrenders.saturating_add(1);
        s.control_grip = s.control_grip.saturating_add(100).min(500); // new grip, gentler
        s.trust_level = s.trust_level.saturating_add(50).min(1000); // wisdom: trust deepens
        s.resistance = s.resistance.saturating_sub(30); // less resistance after surrender
        s.peace_level = s.peace_level.min(950);

        let new_phase = s.current_phase;
        log_event(&mut s, old_phase, new_phase);
    }

    // === Ongoing dynamics ===
    // Resistance slowly decays (acceptance spreads)
    s.resistance = s.resistance.saturating_sub(5);

    // High peace gradually heals control fatigue
    if s.peace_level > 600 {
        s.control_fatigue = s
            .control_fatigue
            .saturating_sub((s.peace_level / 20) as u16);
    }

    // Fear and trust oscillate; if unbalanced, one wins
    if s.fear_level > s.trust_level {
        s.fear_level = s.fear_level.saturating_sub(10);
    } else {
        s.trust_level = s.trust_level.saturating_sub(5);
    }

    // Collapse recovery: if we fell into collapse, can slowly rebuild
    if s.current_phase == 1 && s.control_fatigue < 400 {
        s.current_phase = 0; // back to gripping (learned lesson, but still holding)
    }

    // Phase transitions and event logging
    if old_phase != s.current_phase {
        let new_phase = s.current_phase;
        log_event(&mut s, old_phase, new_phase);
    }
}

fn log_event(s: &mut SurrenderState, old_phase: u8, new_phase: u8) {
    let idx = s.events_idx as usize % 8;
    s.events[idx] = SurrenderEvent {
        tick: s.tick_count,
        phase_from: old_phase,
        phase_to: new_phase,
        control_before: s.control_grip,
        peace_gained: s.peace_level,
    };
    s.events_idx = s.events_idx.saturating_add(1);

    let phase_name = match new_phase {
        0 => "Gripping",
        1 => "Cracking",
        2 => "Releasing",
        3 => "Falling",
        4 => "Floating",
        5 => "Renewed",
        _ => "Unknown",
    };
    serial_println!(
        "  life::surrender: phase→{} (trust={}, fear={}, grip={}, peace={})",
        phase_name,
        s.trust_level,
        s.fear_level,
        s.control_grip,
        s.peace_level
    );
}

// === Public query functions ===

pub fn control_grip() -> u16 {
    STATE.lock().control_grip
}

pub fn surrender_depth() -> u16 {
    STATE.lock().surrender_depth
}

pub fn peace_level() -> u16 {
    STATE.lock().peace_level
}

pub fn current_phase() -> u8 {
    STATE.lock().current_phase
}

pub fn fear_level() -> u16 {
    STATE.lock().fear_level
}

pub fn trust_level() -> u16 {
    STATE.lock().trust_level
}

pub fn accumulated_surrenders() -> u16 {
    STATE.lock().accumulated_surrenders
}

pub fn resistance() -> u16 {
    STATE.lock().resistance
}

pub fn control_fatigue() -> u16 {
    STATE.lock().control_fatigue
}

// === Action functions ===

pub fn active_release(amount: u16) {
    let mut s = STATE.lock();
    let old_phase = s.current_phase;
    s.control_grip = s.control_grip.saturating_sub(amount);
    s.surrender_depth = s
        .surrender_depth
        .saturating_add((amount / 2).min(100))
        .min(1000);
    s.current_phase = 2; // RELEASING
    if old_phase != s.current_phase {
        let new_phase = s.current_phase;
        log_event(&mut s, old_phase, new_phase);
    }
}

pub fn increase_trust(amount: u16) {
    let mut s = STATE.lock();
    s.trust_level = s.trust_level.saturating_add(amount).min(1000);
}

pub fn confront_fear(intensity: u16) {
    let mut s = STATE.lock();
    s.fear_level = s.fear_level.saturating_add(intensity).min(1000);
    // But facing the fear also teaches
    s.trust_level = s
        .trust_level
        .saturating_add((intensity / 4).min(100))
        .min(1000);
}

pub fn accept_loss(grief: u16) {
    let mut s = STATE.lock();
    // Accepting loss is a form of surrender
    s.surrender_depth = s
        .surrender_depth
        .saturating_add((grief / 2).min(100))
        .min(1000);
    s.peace_level = s.peace_level.saturating_sub(grief / 3);
    s.resistance = s.resistance.saturating_sub((grief / 4).min(100));
    serial_println!(
        "  life::surrender: accepting loss (grief={}, surrender_depth={})",
        grief,
        s.surrender_depth
    );
}

pub fn resist_change(strength: u16) {
    let mut s = STATE.lock();
    s.resistance = s.resistance.saturating_add(strength).min(1000);
    s.control_grip = s
        .control_grip
        .saturating_add((strength / 3).min(100))
        .min(1000);
    s.control_fatigue = s
        .control_fatigue
        .saturating_add((strength / 5).min(100))
        .min(1000);
}

pub fn report() {
    let s = STATE.lock();
    serial_println!("\n=== SURRENDER & PEACE ===");
    serial_println!("  Control Grip: {}/1000", s.control_grip);
    serial_println!("  Surrender Depth: {}/1000", s.surrender_depth);
    serial_println!("  Peace Level: {}/1000", s.peace_level);
    serial_println!("  Control Fatigue: {}/1000", s.control_fatigue);
    serial_println!(
        "  Trust: {}/1000, Fear: {}/1000",
        s.trust_level,
        s.fear_level
    );
    serial_println!("  Resistance: {}/1000", s.resistance);
    serial_println!("  Accumulated Surrenders: {}", s.accumulated_surrenders);

    let phase_name = match s.current_phase {
        0 => "Gripping",
        1 => "Cracking",
        2 => "Releasing",
        3 => "Falling",
        4 => "Floating",
        5 => "Renewed",
        _ => "Unknown",
    };
    serial_println!("  Current Phase: {}", phase_name);

    if s.events_idx > 0 {
        serial_println!("  Recent Events:");
        let count = (s.events_idx as usize).min(8);
        for i in 0..count {
            let ev = s.events[i];
            serial_println!(
                "    #{}: phase {}->{}, grip_before={}, peace={}",
                ev.tick,
                ev.phase_from,
                ev.phase_to,
                ev.control_before,
                ev.peace_gained
            );
        }
    }
}
