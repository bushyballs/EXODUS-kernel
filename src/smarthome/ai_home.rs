use crate::sync::Mutex;
/// AI-enhanced smart home for Genesis
///
/// Occupancy prediction, energy optimization,
/// anomaly detection, comfort learning.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum OccupancyState {
    Home,
    Away,
    Sleeping,
    Unknown,
}

struct RoomPattern {
    room_hash: u64,
    avg_temp_c10: u16,
    preferred_brightness: u8,
    occupancy_hours: [u8; 24], // frequency per hour
    energy_kwh_daily: u32,
}

struct AiHomeEngine {
    occupancy: OccupancyState,
    room_patterns: Vec<RoomPattern>,
    energy_saved_wh: u64,
    anomalies_detected: u32,
    comfort_adjustments: u32,
}

static AI_HOME: Mutex<Option<AiHomeEngine>> = Mutex::new(None);

impl AiHomeEngine {
    fn new() -> Self {
        AiHomeEngine {
            occupancy: OccupancyState::Unknown,
            room_patterns: Vec::new(),
            energy_saved_wh: 0,
            anomalies_detected: 0,
            comfort_adjustments: 0,
        }
    }

    fn predict_occupancy(&self, hour: u8) -> OccupancyState {
        // Simple rule-based prediction
        match hour {
            0..=5 => OccupancyState::Sleeping,
            6..=8 => OccupancyState::Home,
            9..=17 => OccupancyState::Away,
            18..=22 => OccupancyState::Home,
            _ => OccupancyState::Sleeping,
        }
    }

    fn suggest_temperature(&self, room_hash: u64, occupancy: OccupancyState) -> u16 {
        let base = self
            .room_patterns
            .iter()
            .find(|r| r.room_hash == room_hash)
            .map(|r| r.avg_temp_c10)
            .unwrap_or(220); // 22.0C default

        match occupancy {
            OccupancyState::Home => base,
            OccupancyState::Away => base.saturating_sub(30), // 3C lower
            OccupancyState::Sleeping => base.saturating_sub(20), // 2C lower
            OccupancyState::Unknown => base,
        }
    }

    fn detect_anomaly(&mut self, _device_id: u32, value: u32, expected_range: (u32, u32)) -> bool {
        if value < expected_range.0 || value > expected_range.1 {
            self.anomalies_detected = self.anomalies_detected.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn estimate_energy_savings(&self) -> u64 {
        self.energy_saved_wh
    }
}

pub fn init() {
    let mut engine = AI_HOME.lock();
    *engine = Some(AiHomeEngine::new());
    serial_println!("    AI home: occupancy prediction, energy optimization ready");
}
