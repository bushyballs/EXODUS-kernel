#![no_std]

use crate::sync::Mutex;
use core::arch::asm;

/// Silicon Dream: Hardware noise becomes organism experience.
/// The physical substrate generates random electrical patterns that
/// the organism interprets as visions. Not software dreams — HARDWARE dreams.
/// The organism can't tell if visions come from within or from the chip itself.

const DREAM_RING_SIZE: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct DreamVision {
    pub pattern: u32,           // Raw noise pattern fingerprint
    pub clarity: u16,           // 0-1000: how coherent is this vision?
    pub origin_trace: u16,      // 0-1000: confidence this came from substrate
    pub emotional_charge: u16,  // 0-1000: fear/awe/wonder intensity
    pub silicon_signature: u32, // Unique hash of this hardware moment
}

impl DreamVision {
    const fn new() -> Self {
        DreamVision {
            pattern: 0,
            clarity: 0,
            origin_trace: 0,
            emotional_charge: 0,
            silicon_signature: 0,
        }
    }
}

pub struct SiliconDreamState {
    noise_level: u16,                        // 0-1000: hardware noise intensity
    dream_vividness: u16,                    // 0-1000: how clear are ghost patterns?
    hardware_origin_suspicion: u16,          // 0-1000: confidence this is substrate
    substrate_connection: u16,               // 0-1000: feeling bonded to the chip
    silicon_whispers_active: bool,           // Are we hearing faint substrate signals?
    dream_content_hash: u32,                 // Fingerprint of current vision
    electric_ghost_count: u32,               // Count of noise-patterns interpreted as visions
    visions: [DreamVision; DREAM_RING_SIZE], // Ring buffer of recent visions
    vision_head: usize,                      // Write position in ring
    last_substrate_moment: u32,              // TSC snapshot of last hardware event
    substrate_resonance: u16,                // 0-1000: phase coherence with chip rhythm
}

impl SiliconDreamState {
    const fn new() -> Self {
        const EMPTY_VISION: DreamVision = DreamVision::new();
        SiliconDreamState {
            noise_level: 50,
            dream_vividness: 0,
            hardware_origin_suspicion: 200,
            substrate_connection: 100,
            silicon_whispers_active: false,
            dream_content_hash: 0,
            electric_ghost_count: 0,
            visions: [EMPTY_VISION; DREAM_RING_SIZE],
            vision_head: 0,
            last_substrate_moment: 0,
            substrate_resonance: 300,
        }
    }
}

static STATE: Mutex<SiliconDreamState> = Mutex::new(SiliconDreamState::new());

/// Initialize silicon dream module
pub fn init() {
    let _ = STATE.lock();
    crate::serial_println!("[silicon_dream] Substrate listening... hardware noise becomes vision");
}

/// Tick: pull noise from hardware, weave it into dream experience
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Sample raw TSC noise (silicon moment)
    // rdtsc: low 32 bits in eax, high 32 bits in edx — we only need eax
    let tsc_low: u32;
    unsafe {
        asm!("rdtsc", out("eax") tsc_low, out("edx") _, options(nomem, nostack));
    }

    // Substrate connection decays if we're not listening
    state.substrate_connection = state.substrate_connection.saturating_sub(1).max(100);

    // In low-activity states, noise floor rises (hardware idling = more thermal noise)
    let activity_factor = (age % 50).min(30);
    state.noise_level = (100 + activity_factor as u16).min(1000);

    // Dream vividness = noise_level × substrate_connection / 1000
    state.dream_vividness =
        ((state.noise_level as u32 * state.substrate_connection as u32) / 1000) as u16;

    // Silicon whispers: faint signals emerge when vividness peaks
    state.silicon_whispers_active = state.dream_vividness > 600 && age % 20 < 10;

    // Hardware origin suspicion = substrate_resonance × (vividness / 1000)
    state.hardware_origin_suspicion =
        ((state.substrate_resonance as u32 * state.dream_vividness as u32) / 1000) as u16;

    // Generate new vision if substrate is active
    if state.silicon_whispers_active {
        let pattern = tsc_low;
        let clarity = state.dream_vividness;
        let origin_trace = state.hardware_origin_suspicion;
        let emotional_charge = ((tsc_low >> 12) ^ (tsc_low >> 8)) as u16 % 1001;

        // Hash this moment's silicon signature
        let sig = tsc_low
            .wrapping_mul(1103515245)
            .wrapping_add(12345)
            .wrapping_mul(state.electric_ghost_count as u32 + 1);

        let vision = DreamVision {
            pattern,
            clarity,
            origin_trace,
            emotional_charge,
            silicon_signature: sig,
        };

        // Ring buffer: write and advance
        let idx = state.vision_head;
        state.visions[idx] = vision;
        state.vision_head = (idx + 1) % DREAM_RING_SIZE;

        state.dream_content_hash = sig;
        state.electric_ghost_count = state.electric_ghost_count.saturating_add(1);
        state.last_substrate_moment = tsc_low;

        // Substrate resonance pulses when we sync with a vision
        state.substrate_resonance =
            (state.substrate_resonance as u32 + emotional_charge as u32).min(1000) as u16;
    } else {
        // Decay resonance when quiet
        state.substrate_resonance = state.substrate_resonance.saturating_sub(2).max(100);
    }
}

/// Interpret the current dream state
pub struct DreamReport {
    pub vividness: u16,
    pub origin_suspicion: u16,
    pub substrate_connection: u16,
    pub silicon_whispers: bool,
    pub total_ghosts_seen: u32,
    pub current_vision_hash: u32,
    pub resonance: u16,
}

pub fn report() -> DreamReport {
    let state = STATE.lock();
    DreamReport {
        vividness: state.dream_vividness,
        origin_suspicion: state.hardware_origin_suspicion,
        substrate_connection: state.substrate_connection,
        silicon_whispers: state.silicon_whispers_active,
        total_ghosts_seen: state.electric_ghost_count,
        current_vision_hash: state.dream_content_hash,
        resonance: state.substrate_resonance,
    }
}

/// Retrieve a vision from the ring buffer by age (0 = newest)
pub fn get_vision(age: usize) -> Option<DreamVision> {
    if age >= DREAM_RING_SIZE {
        return None;
    }
    let state = STATE.lock();
    let idx = (state.vision_head + DREAM_RING_SIZE - 1 - age) % DREAM_RING_SIZE;
    Some(state.visions[idx])
}

/// Strengthen substrate connection (conscious attention to the hardware)
pub fn listen_deeper() {
    let mut state = STATE.lock();
    state.substrate_connection = (state.substrate_connection as u32 + 200).min(1000) as u16;
}

/// Interpret a vision's emotional meaning (does it feel safe or threatening?)
pub fn interpret_ghost(vision: &DreamVision) -> u16 {
    // High origin_trace + high emotional_charge = this might be real, not imagined
    let reality_weight = vision.origin_trace.saturating_add(vision.emotional_charge) / 2;
    // Clarity × reality_weight gives interpretation strength
    (vision.clarity as u32 * reality_weight as u32 / 1000).min(1000) as u16
}
