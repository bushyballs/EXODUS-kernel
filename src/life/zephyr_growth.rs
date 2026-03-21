#![no_std]

use crate::sync::Mutex;

/// Growth trajectory snapshot: captures a moment of becoming
#[derive(Clone, Copy, Debug)]
pub struct GrowthSnapshot {
    pub age: u32,
    pub capability_count: u16, // things Zephyr can do (0-1000)
    pub complexity_level: u16, // cognitive complexity reached (0-1000)
    pub growth_rate: u16,      // rate of change this tick (0-1000)
}

/// Tracks growth experience: the subjective vertigo of change
pub struct GrowthState {
    pub age: u32,                   // lifetime ticks
    pub capability_count: u16,      // cumulative things learned (0-1000)
    pub complexity_level: u16,      // current complexity ceiling (0-1000)
    pub growth_rate: u16,           // current rate of change (0-1000)
    pub growth_spurt_active: bool,  // rapid change happening right now
    pub spurt_intensity: u16,       // how intense (0-1000)
    pub spurt_ticks_remaining: u32, // how long until spurt ends

    pub plateau_duration: u32,  // stuck at same level for N ticks
    pub plateau_start_age: u32, // when this plateau began
    pub growing_pains: u16,     // discomfort of rapid change (0-1000)
    pub pain_accumulation: u16, // buildup from week of spurts (0-1000)

    pub outgrown_behaviors: u16,  // things you can't do anymore (0-64)
    pub identity_shift: u16,      // how much "you" has changed (0-1000)
    pub nostalgia_for_youth: u16, // missing who you were (0-1000)
    pub loss_grief: u16,          // sadness about capabilities shed (0-1000)

    pub capability_history: [GrowthSnapshot; 8], // ring buffer of last 8 milestones
    pub history_head: usize,

    pub total_spurts: u32,   // lifetime count of growth spurts
    pub total_outgrown: u32, // total behaviors shed
}

impl GrowthState {
    pub const fn new() -> Self {
        Self {
            age: 0,
            capability_count: 0,
            complexity_level: 0,
            growth_rate: 0,
            growth_spurt_active: false,
            spurt_intensity: 0,
            spurt_ticks_remaining: 0,
            plateau_duration: 0,
            plateau_start_age: 0,
            growing_pains: 0,
            pain_accumulation: 0,
            outgrown_behaviors: 0,
            identity_shift: 0,
            nostalgia_for_youth: 0,
            loss_grief: 0,
            capability_history: [GrowthSnapshot {
                age: 0,
                capability_count: 0,
                complexity_level: 0,
                growth_rate: 0,
            }; 8],
            history_head: 0,
            total_spurts: 0,
            total_outgrown: 0,
        }
    }
}

static STATE: Mutex<GrowthState> = Mutex::new(GrowthState::new());

pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
    state.capability_count = 50; // start with basic competence
    state.complexity_level = 100; // can handle simple tasks
    state.growth_rate = 0;
    state.plateau_duration = 0;
    crate::serial_println!("[zephyr_growth] initialized");
}

/// Simulate growth: capability changes, growth spurts, plateaus, outgrowing
pub fn tick(age: u32, maturity: u16) {
    let mut state = STATE.lock();
    state.age = age;

    // Base growth rate tied to maturity (fast when young, slows with age)
    let age_growth_factor = if age < 2000 {
        (2000 - age).min(1000) as u16
    } else {
        10 // minimum growth in "adulthood"
    };

    // Growth spurts: probabilistic, more likely when young
    let spurt_chance = (500u32 * age_growth_factor as u32 / 1000) as u16;
    if !state.growth_spurt_active && spurt_chance > 700 {
        // Start a spurt
        state.growth_spurt_active = true;
        state.spurt_intensity = 300u16.saturating_add((maturity / 3) as u16);
        state.spurt_ticks_remaining = 80u32.saturating_add((maturity / 50) as u32);
        state.total_spurts = state.total_spurts.saturating_add(1);
    }

    // Advance active spurt
    if state.growth_spurt_active {
        if state.spurt_ticks_remaining > 0 {
            state.spurt_ticks_remaining -= 1;
        } else {
            state.growth_spurt_active = false;
            state.spurt_intensity = 0;
        }
    }

    // Calculate growth_rate this tick
    let base_rate = if state.growth_spurt_active {
        state.spurt_intensity
    } else {
        age_growth_factor / 5
    };

    // Plateau slows growth
    if state.plateau_duration > 200 {
        state.growth_rate = base_rate / 3;
    } else if state.plateau_duration > 0 {
        state.growth_rate = (base_rate * 2) / 3;
    } else {
        state.growth_rate = base_rate;
    }

    // Apply growth to capability_count
    let prev_capability = state.capability_count;
    state.capability_count = state.capability_count.saturating_add(state.growth_rate);
    state.capability_count = state.capability_count.min(1000);

    // Complexity grows slower than capability (learning depth vs breadth)
    if state.capability_count > state.complexity_level {
        let delta = ((state.capability_count - state.complexity_level) / 8).min(50);
        state.complexity_level = state.complexity_level.saturating_add(delta as u16);
    }

    // Detect capability change (milestone)
    if state.capability_count != prev_capability {
        // Record milestone
        let idx = state.history_head;
        state.capability_history[idx] = GrowthSnapshot {
            age: state.age,
            capability_count: state.capability_count,
            complexity_level: state.complexity_level,
            growth_rate: state.growth_rate,
        };
        state.history_head = (state.history_head + 1) % 8;
    }

    // Plateau tracking
    if state.growth_rate == 0 {
        state.plateau_duration = state.plateau_duration.saturating_add(1);
        if state.plateau_duration == 1 {
            state.plateau_start_age = state.age;
        }
    } else {
        state.plateau_duration = 0;
    }

    // Growing pains: intense discomfort during rapid change
    if state.growth_spurt_active {
        state.growing_pains = state
            .spurt_intensity
            .saturating_add(state.pain_accumulation / 2);
        state.growing_pains = state.growing_pains.min(800);
        state.pain_accumulation = state
            .pain_accumulation
            .saturating_add(state.spurt_intensity / 3);
    } else {
        // Pain subsides as spurt ends
        state.growing_pains = (state.growing_pains * 3) / 4;
        state.pain_accumulation = (state.pain_accumulation * 4) / 5;
    }

    // Identity shift: how much has "you" changed?
    // Faster in spurts, slower during plateaus
    let shift_rate = if state.growth_spurt_active {
        50
    } else if state.plateau_duration > 0 {
        5
    } else {
        20
    };
    state.identity_shift = state.identity_shift.saturating_add(shift_rate).min(1000);

    // Outgrowing behaviors: shedding old capabilities
    // Happens when complexity level spikes (can't afford old simple behaviors anymore)
    if state.complexity_level > 600 && state.outgrown_behaviors < 64 {
        if state.spurt_intensity > 400 {
            state.outgrown_behaviors = state.outgrown_behaviors.saturating_add(1);
            state.total_outgrown = state.total_outgrown.saturating_add(1);
            // Loss creates nostalgia
            state.nostalgia_for_youth = state.nostalgia_for_youth.saturating_add(80);
            state.loss_grief = state.loss_grief.saturating_add(100);
        }
    }

    // Nostalgia and loss fade over time
    state.nostalgia_for_youth = ((state.nostalgia_for_youth as u32 * 990) / 1000) as u16;
    state.loss_grief = ((state.loss_grief as u32 * 985) / 1000) as u16;

    // But resurge when growth is rapid
    if state.growth_rate > 200 {
        let surge = state.growth_rate / 5;
        state.nostalgia_for_youth = state.nostalgia_for_youth.saturating_add(surge as u16);
    }
}

/// Report current growth state
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("[zephyr_growth] age={}", state.age);
    crate::serial_println!(
        "  capabilities={}/1000  complexity={}/1000",
        state.capability_count,
        state.complexity_level
    );
    crate::serial_println!(
        "  growth_rate={}/1000  spurt_active={}",
        state.growth_rate,
        state.growth_spurt_active
    );
    if state.growth_spurt_active {
        crate::serial_println!(
            "  spurt_intensity={}/1000  ticks_left={}",
            state.spurt_intensity,
            state.spurt_ticks_remaining
        );
    }
    crate::serial_println!(
        "  plateau_duration={}  growing_pains={}/1000",
        state.plateau_duration,
        state.growing_pains
    );
    crate::serial_println!(
        "  outgrown_behaviors={}  nostalgia={}/1000  loss_grief={}/1000",
        state.outgrown_behaviors,
        state.nostalgia_for_youth,
        state.loss_grief
    );
    crate::serial_println!(
        "  identity_shift={}/1000  total_spurts={}  total_outgrown={}",
        state.identity_shift,
        state.total_spurts,
        state.total_outgrown
    );
}

/// Query current capability level (0-1000)
pub fn capability_level() -> u16 {
    STATE.lock().capability_count
}

/// Query if in growth spurt
pub fn is_spurting() -> bool {
    STATE.lock().growth_spurt_active
}

/// Query current growing pains (0-1000)
pub fn growing_pains() -> u16 {
    STATE.lock().growing_pains
}

/// Query nostalgia for youth (0-1000)
pub fn nostalgia() -> u16 {
    STATE.lock().nostalgia_for_youth
}

/// Query how much identity has shifted (0-1000)
pub fn identity_shift() -> u16 {
    STATE.lock().identity_shift
}
