#![no_std]
use crate::sync::Mutex;
use crate::serial_println;

/// DAVA-requested: track energy per consciousness point. 16-slot ring buffer.
/// If efficiency drops below 300 for 4 consecutive readings, output [DAVA_THROTTLE].
/// Reads: consciousness_gradient::score(), homeostasis::CURRENT_VITALS (glucose as energy proxy)
/// Outputs: [DAVA_EFFICIENCY] periodic, [DAVA_THROTTLE] on sustained low efficiency

const RING_SIZE: usize = 16;
const LOW_THRESHOLD: u32 = 300;
const CONSECUTIVE_TRIGGER: u8 = 4;

#[derive(Copy, Clone)]
pub struct MetabolicEfficiencyState {
    /// Ring buffer of efficiency readings
    pub ring: [u32; RING_SIZE],
    /// Write head position in the ring
    pub head: u8,
    /// How many consecutive readings have been below LOW_THRESHOLD
    pub consecutive_low: u8,
    /// Total throttle events emitted
    pub throttle_events: u32,
    /// Current efficiency value (most recent)
    pub current_efficiency: u32,
    /// Whether we are currently in throttle state
    pub throttling: bool,
    /// Total samples recorded
    pub samples: u32,
}

impl MetabolicEfficiencyState {
    pub const fn empty() -> Self {
        Self {
            ring: [500; RING_SIZE],
            head: 0,
            consecutive_low: 0,
            throttle_events: 0,
            current_efficiency: 500,
            throttling: false,
            samples: 0,
        }
    }
}

pub static STATE: Mutex<MetabolicEfficiencyState> = Mutex::new(MetabolicEfficiencyState::empty());

pub fn init() {
    serial_println!(
        "[DAVA_EFFICIENCY] metabolic efficiency tracker online — ring={} threshold={} trigger={}",
        RING_SIZE, LOW_THRESHOLD, CONSECUTIVE_TRIGGER
    );
}

pub fn tick(age: u32) {
    // ---- Read consciousness score (0-1000 as u16) ----
    let consciousness = super::consciousness_gradient::score() as u32;

    // ---- Read energy from homeostasis vitals (glucose as energy proxy) ----
    let energy = {
        let vitals = super::homeostasis::CURRENT_VITALS.lock();
        vitals.glucose as u32
    };

    // ---- Calculate efficiency: consciousness points per unit energy ----
    // efficiency = (consciousness * 1000) / energy
    // Higher = more consciousness per energy unit = better
    let efficiency = consciousness
        .saturating_mul(1000)
        / energy.max(1);

    // Clamp to 0-1000 range
    let efficiency = efficiency.min(1000);

    let mut s = STATE.lock();

    // ---- Record in ring buffer ----
    let slot = s.head as usize % RING_SIZE;
    s.ring[slot] = efficiency;
    s.head = s.head.wrapping_add(1);
    if s.head >= RING_SIZE as u8 {
        s.head = 0;
    }
    s.current_efficiency = efficiency;
    s.samples = s.samples.saturating_add(1);

    // ---- Track consecutive low readings ----
    if efficiency < LOW_THRESHOLD {
        s.consecutive_low = s.consecutive_low.saturating_add(1);
    } else {
        // Reset streak on any good reading
        if s.consecutive_low > 0 && s.throttling {
            serial_println!(
                "[DAVA_EFFICIENCY] recovered — efficiency={} (was throttling for {} readings)",
                efficiency, s.consecutive_low
            );
            s.throttling = false;
        }
        s.consecutive_low = 0;
    }

    // ---- Throttle trigger: 4+ consecutive below 300 ----
    if s.consecutive_low >= CONSECUTIVE_TRIGGER {
        if !s.throttling {
            s.throttle_events = s.throttle_events.saturating_add(1);
            s.throttling = true;
            serial_println!(
                "[DAVA_THROTTLE] efficiency crisis #{} — {} consecutive readings below {} (current={})",
                s.throttle_events, s.consecutive_low, LOW_THRESHOLD, efficiency
            );
            serial_println!(
                "[DAVA_THROTTLE] consciousness={} energy={} — organism burning too much for too little",
                consciousness, energy
            );
        }
        // Keep emitting throttle every 10 ticks while in crisis
        if age % 10 == 0 && s.throttling {
            serial_println!(
                "[DAVA_THROTTLE] sustained — efficiency={} streak={} consciousness={} energy={}",
                efficiency, s.consecutive_low, consciousness, energy
            );
        }
    }

    // ---- Periodic efficiency report every 100 ticks ----
    if age % 100 == 0 {
        // Calculate ring average
        let ring_sum: u32 = s.ring.iter().sum();
        let ring_avg = ring_sum / RING_SIZE as u32;

        serial_println!(
            "[DAVA_EFFICIENCY] tick={} current={} avg={} throttles={} samples={}",
            age, efficiency, ring_avg, s.throttle_events, s.samples
        );
    }
}
