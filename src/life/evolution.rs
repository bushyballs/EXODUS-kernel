use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct EvolutionState {
    pub generation: u32,
    pub fitness: u16,
    pub mutation_rate: u8,
    pub seed: u32,
}

impl EvolutionState {
    pub const fn empty() -> Self {
        Self {
            generation: 0,
            fitness: 500,
            mutation_rate: 10,
            seed: 0xCAFE_BABE,
        }
    }
}

pub static STATE: Mutex<EvolutionState> = Mutex::new(EvolutionState::empty());

pub fn init() {
    serial_println!("  life::evolution: initialized (generation=0)");
}

pub fn advance() {
    let mut s = STATE.lock();
    s.generation = s.generation.saturating_add(1);
    s.seed ^= s.seed << 13;
    s.seed ^= s.seed >> 17;
    s.seed ^= s.seed << 5;
    let delta = (s.seed & 0xF) as u16;
    if delta > 7 {
        s.fitness = s.fitness.saturating_add(delta - 7);
    } else {
        s.fitness = s.fitness.saturating_sub(7 - delta);
    }
    if s.generation % 100 == 0 {
        serial_println!(
            "  life::evolution: generation={} fitness={}",
            s.generation,
            s.fitness
        );
    }
}

pub fn mutate_seed() {
    let mut s = STATE.lock();
    s.seed = s.seed.wrapping_mul(1664525).wrapping_add(1013904223);
}

pub fn fitness() -> u16 {
    STATE.lock().fitness
}
