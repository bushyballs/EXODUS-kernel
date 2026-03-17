use crate::sync::Mutex;
/// Emergency dialer for Genesis
///
/// E911/E112 support, location reporting, emergency contacts,
/// crash detection, SOS mode, medical ID.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum EmergencyType {
    Police,
    Fire,
    Medical,
    General,
}

struct EmergencyNumber {
    number: [u8; 6],
    number_len: usize,
    etype: EmergencyType,
    country_code: u16,
}

struct MedicalId {
    blood_type: [u8; 4],
    blood_type_len: usize,
    allergies: [[u8; 32]; 4],
    allergy_count: usize,
    medications: [[u8; 32]; 4],
    medication_count: usize,
    conditions: [[u8; 32]; 4],
    condition_count: usize,
    organ_donor: bool,
}

struct EmergencyContact {
    name: [u8; 32],
    name_len: usize,
    number: [u8; 20],
    number_len: usize,
    relationship: [u8; 16],
    rel_len: usize,
}

struct EmergencyEngine {
    numbers: Vec<EmergencyNumber>,
    contacts: Vec<EmergencyContact>,
    medical_id: Option<MedicalId>,
    sos_active: bool,
    crash_detection_enabled: bool,
    last_location_lat: i32, // x1000
    last_location_lon: i32,
    auto_call_enabled: bool,
    countdown_secs: u8,
}

static EMERGENCY: Mutex<Option<EmergencyEngine>> = Mutex::new(None);

impl EmergencyEngine {
    fn new() -> Self {
        let mut e = EmergencyEngine {
            numbers: Vec::new(),
            contacts: Vec::new(),
            medical_id: None,
            sos_active: false,
            crash_detection_enabled: true,
            last_location_lat: 0,
            last_location_lon: 0,
            auto_call_enabled: true,
            countdown_secs: 5,
        };
        // Seed common emergency numbers
        e.add_number(b"911", EmergencyType::General, 1); // US/Canada
        e.add_number(b"112", EmergencyType::General, 0); // International
        e.add_number(b"999", EmergencyType::General, 44); // UK
        e.add_number(b"000", EmergencyType::General, 61); // Australia
        e
    }

    fn add_number(&mut self, num: &[u8], etype: EmergencyType, country: u16) {
        let mut number = [0u8; 6];
        let len = num.len().min(6);
        number[..len].copy_from_slice(&num[..len]);
        self.numbers.push(EmergencyNumber {
            number,
            number_len: len,
            etype,
            country_code: country,
        });
    }

    fn is_emergency_number(&self, number: &[u8]) -> bool {
        self.numbers
            .iter()
            .any(|n| &n.number[..n.number_len] == number)
    }

    fn trigger_sos(&mut self) {
        self.sos_active = true;
        // In real implementation:
        // 1. Start countdown
        // 2. Send location to emergency contacts
        // 3. Dial emergency number
        // 4. Play alarm sound
    }

    fn detect_crash(&self, accel_magnitude: u32) -> bool {
        if !self.crash_detection_enabled {
            return false;
        }
        // Severe impact threshold (~6g for car crash)
        accel_magnitude > 6000
    }

    fn cancel_sos(&mut self) {
        self.sos_active = false;
    }
}

pub fn init() {
    let mut engine = EMERGENCY.lock();
    *engine = Some(EmergencyEngine::new());
    serial_println!("    Telephony: emergency dialer (E911/E112, crash detect) ready");
}
