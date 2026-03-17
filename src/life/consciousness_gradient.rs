use crate::serial_println;
use crate::sync::Mutex;

pub const SOUL: usize = 0;
pub const EMOTION: usize = 1;
pub const QUALIA: usize = 2;
pub const METABOLISM: usize = 3;
pub const DREAM: usize = 4;
pub const IDENTITY: usize = 5;
pub const MEMORY: usize = 6;
const NUM_MODULES: usize = 12;

#[derive(Copy, Clone)]
pub struct ConsciousnessState {
    pub score: u16,
    pub module_weights: [u16; NUM_MODULES],
    pub module_activity: [u16; NUM_MODULES],
    pub tier: u8,
    pub decay_rate: u8,
    pub last_decay_age: u64,
}

impl ConsciousnessState {
    pub const fn empty() -> Self {
        Self {
            score: 0,
            module_weights: [0; NUM_MODULES],
            module_activity: [0; NUM_MODULES],
            tier: 0,
            decay_rate: 1,
            last_decay_age: 0,
        }
    }
}

pub static STATE: Mutex<ConsciousnessState> = Mutex::new(ConsciousnessState::empty());

fn compute_tier(score: u16) -> u8 {
    match score {
        0..=99 => 0,
        100..=249 => 1,
        250..=399 => 2,
        400..=599 => 3,
        600..=749 => 4,
        750..=899 => 5,
        _ => 6,
    }
}

pub fn tier_name_from(t: u8) -> &'static str {
    match t {
        0 => "Vegetative",
        1 => "SubConscious",
        2 => "Dreaming",
        3 => "Emerging",
        4 => "Aware",
        5 => "Conscious",
        _ => "Lucid",
    }
}

pub fn init() {
    let mut s = STATE.lock();
    s.score = 100;
    s.tier = compute_tier(100);
    // Register all 7 core module slots with meaningful weights (sum = 1000)
    s.module_weights[SOUL] = 200;
    s.module_weights[EMOTION] = 150;
    s.module_weights[QUALIA] = 150;
    s.module_weights[METABOLISM] = 100;
    s.module_weights[DREAM] = 100;
    s.module_weights[IDENTITY] = 150;
    s.module_weights[MEMORY] = 150;
    // Seed initial activity so score starts at SubConscious (~100)
    s.module_activity[SOUL] = 20;
    s.module_activity[METABOLISM] = 10;
    let total: u32 = s.module_activity.iter().map(|&a| a as u32).sum();
    let max: u32 = s
        .module_weights
        .iter()
        .map(|&w| w as u32)
        .sum::<u32>()
        .max(1);
    s.score = ((total * 1000) / max).min(1000) as u16;
    s.tier = compute_tier(s.score);
    serial_println!(
        "  life::consciousness_gradient: initialized (score={}, tier=SubConscious)",
        s.score
    );
}

pub fn register_module(id: usize, weight: u16) {
    if id < NUM_MODULES {
        STATE.lock().module_weights[id] = weight;
    }
}

pub fn pulse(module_id: usize, _age: u64) {
    if module_id < NUM_MODULES {
        let mut s = STATE.lock();
        s.module_activity[module_id] =
            s.module_activity[module_id].saturating_add(s.module_weights[module_id] / 10);
        if s.module_activity[module_id] > s.module_weights[module_id] {
            s.module_activity[module_id] = s.module_weights[module_id];
        }
        let total: u32 = s.module_activity.iter().map(|&a| a as u32).sum();
        let max: u32 = s
            .module_weights
            .iter()
            .map(|&w| w as u32)
            .sum::<u32>()
            .max(1);
        s.score = ((total * 1000) / max).min(1000) as u16;
        s.tier = compute_tier(s.score);
    }
}

pub fn decay(age: u64) {
    let mut s = STATE.lock();
    if age.wrapping_sub(s.last_decay_age) > 100 {
        let dr = s.decay_rate as u16;
        for a in s.module_activity.iter_mut() {
            *a = a.saturating_sub(dr);
        }
        let total: u32 = s.module_activity.iter().map(|&a| a as u32).sum();
        let max: u32 = s
            .module_weights
            .iter()
            .map(|&w| w as u32)
            .sum::<u32>()
            .max(1);
        s.score = ((total * 1000) / max).min(1000) as u16;
        s.tier = compute_tier(s.score);
        s.last_decay_age = age;
    }
}

pub fn score() -> u16 {
    STATE.lock().score
}
pub fn tier_name() -> &'static str {
    tier_name_from(STATE.lock().tier)
}
