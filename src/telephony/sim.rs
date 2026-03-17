use crate::sync::Mutex;
/// SIM/eSIM management for Genesis
///
/// Physical SIM, eSIM profiles, dual-SIM management,
/// ICCID/IMSI handling, PIN/PUK, carrier detection.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SimType {
    Physical,
    ESim,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SimState {
    Absent,
    PinRequired,
    PukRequired,
    Ready,
    Locked,
    Error,
}

struct SimSlot {
    slot_id: u8,
    sim_type: SimType,
    state: SimState,
    iccid: [u8; 20],
    iccid_len: usize,
    imsi: [u8; 16],
    imsi_len: usize,
    carrier_name: [u8; 32],
    carrier_len: usize,
    mcc: u16, // mobile country code
    mnc: u16, // mobile network code
    signal_dbm: i16,
    data_enabled: bool,
    is_default_voice: bool,
    is_default_data: bool,
    pin_attempts_left: u8,
}

struct SimManager {
    slots: Vec<SimSlot>,
    active_data_slot: u8,
    active_voice_slot: u8,
    esim_profiles: Vec<EsimProfile>,
}

struct EsimProfile {
    iccid: [u8; 20],
    iccid_len: usize,
    carrier: [u8; 32],
    carrier_len: usize,
    active: bool,
}

static SIM_MGR: Mutex<Option<SimManager>> = Mutex::new(None);

impl SimManager {
    fn new() -> Self {
        SimManager {
            slots: Vec::new(),
            active_data_slot: 0,
            active_voice_slot: 0,
            esim_profiles: Vec::new(),
        }
    }

    fn detect_sims(&mut self) {
        // Simulate detecting SIM slots
        for i in 0..2u8 {
            self.slots.push(SimSlot {
                slot_id: i,
                sim_type: if i == 0 {
                    SimType::Physical
                } else {
                    SimType::ESim
                },
                state: SimState::Absent,
                iccid: [0; 20],
                iccid_len: 0,
                imsi: [0; 16],
                imsi_len: 0,
                carrier_name: [0; 32],
                carrier_len: 0,
                mcc: 0,
                mnc: 0,
                signal_dbm: -120,
                data_enabled: false,
                is_default_voice: i == 0,
                is_default_data: i == 0,
                pin_attempts_left: 3,
            });
        }
    }

    fn set_default_data(&mut self, slot: u8) {
        for s in self.slots.iter_mut() {
            s.is_default_data = s.slot_id == slot;
        }
        self.active_data_slot = slot;
    }

    fn set_default_voice(&mut self, slot: u8) {
        for s in self.slots.iter_mut() {
            s.is_default_voice = s.slot_id == slot;
        }
        self.active_voice_slot = slot;
    }

    fn verify_pin(&mut self, slot: u8, _pin: &[u8; 4]) -> bool {
        if let Some(s) = self.slots.iter_mut().find(|s| s.slot_id == slot) {
            if s.state == SimState::PinRequired {
                // In real implementation, send PIN to modem
                s.pin_attempts_left -= 1;
                if s.pin_attempts_left == 0 {
                    s.state = SimState::PukRequired;
                    return false;
                }
                s.state = SimState::Ready;
                return true;
            }
        }
        false
    }

    fn get_signal_strength(&self, slot: u8) -> i16 {
        self.slots
            .iter()
            .find(|s| s.slot_id == slot)
            .map(|s| s.signal_dbm)
            .unwrap_or(-120)
    }
}

pub fn init() {
    let mut mgr = SIM_MGR.lock();
    let mut sm = SimManager::new();
    sm.detect_sims();
    *mgr = Some(sm);
    serial_println!("    Telephony: SIM/eSIM manager ready (dual-SIM)");
}
