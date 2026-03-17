use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct GenomeState {
    pub curiosity_gene: u8,
    pub empathy_gene: u8,
    pub risk_gene: u8,
    pub creativity_gene: u8,
    pub resilience_gene: u8,
    pub generation: u16,
    pub mutation_seed: u32,
}

impl GenomeState {
    pub const fn empty() -> Self {
        Self {
            curiosity_gene: 128,
            empathy_gene: 128,
            risk_gene: 64,
            creativity_gene: 160,
            resilience_gene: 200,
            generation: 0,
            mutation_seed: 0xDEAD_BEEF,
        }
    }
}

pub static STATE: Mutex<GenomeState> = Mutex::new(GenomeState::empty());

pub fn init() {
    serial_println!("  life::genome: initialized (generation=0)");
}

pub fn mutate() {
    let mut g = STATE.lock();
    g.mutation_seed ^= g.mutation_seed << 13;
    g.mutation_seed ^= g.mutation_seed >> 17;
    g.mutation_seed ^= g.mutation_seed << 5;
    let delta = (g.mutation_seed & 0xFF) as u8;
    g.curiosity_gene = g.curiosity_gene.wrapping_add(delta & 0x3);
    g.generation = g.generation.saturating_add(1);
}

pub fn curiosity_bias() -> u16 {
    (STATE.lock().curiosity_gene as u16) * 4
}
pub fn creativity_bias() -> u16 {
    (STATE.lock().creativity_gene as u16) * 4
}
pub fn resilience_bias() -> u16 {
    (STATE.lock().resilience_gene as u16) * 4
}
