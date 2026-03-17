use crate::sync::Mutex;
/// OBD-II diagnostics for Genesis
///
/// DTC reading, real-time PIDs, freeze frame,
/// emission readiness, trip computer.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

struct DiagnosticCode {
    code: [u8; 5], // e.g., P0420
    description_hash: u64,
    severity: DtcSeverity,
    timestamp: u64,
    cleared: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum DtcSeverity {
    Info,
    Warning,
    Serious,
    Critical,
}

struct TripComputer {
    distance_m: u64,
    fuel_used_ml: u64,
    avg_speed_kph: u32,
    max_speed_kph: u32,
    duration_secs: u64,
    start_time: u64,
}

struct ObdEngine {
    codes: Vec<DiagnosticCode>,
    trip: TripComputer,
    connected: bool,
    protocol: ObdProtocol,
}

#[derive(Clone, Copy, PartialEq)]
enum ObdProtocol {
    Iso9141,
    Kwp2000,
    Can11bit,
    Can29bit,
    Unknown,
}

static OBD: Mutex<Option<ObdEngine>> = Mutex::new(None);

impl ObdEngine {
    fn new() -> Self {
        ObdEngine {
            codes: Vec::new(),
            trip: TripComputer {
                distance_m: 0,
                fuel_used_ml: 0,
                avg_speed_kph: 0,
                max_speed_kph: 0,
                duration_secs: 0,
                start_time: 0,
            },
            connected: false,
            protocol: ObdProtocol::Unknown,
        }
    }

    fn add_dtc(&mut self, code: &[u8; 5], severity: DtcSeverity, timestamp: u64) {
        self.codes.push(DiagnosticCode {
            code: *code,
            description_hash: 0,
            severity,
            timestamp,
            cleared: false,
        });
    }

    fn clear_codes(&mut self) {
        for code in self.codes.iter_mut() {
            code.cleared = true;
        }
    }

    fn active_code_count(&self) -> usize {
        self.codes.iter().filter(|c| !c.cleared).count()
    }

    fn fuel_economy_l100km(&self) -> u32 {
        if self.trip.distance_m == 0 {
            return 0;
        }
        // L/100km = (fuel_ml / 1000) / (distance_m / 100000) = fuel_ml * 100 / distance_m
        (self.trip.fuel_used_ml * 100 / self.trip.distance_m) as u32
    }
}

pub fn init() {
    let mut o = OBD.lock();
    *o = Some(ObdEngine::new());
    serial_println!("    Automotive: OBD-II diagnostics ready");
}
