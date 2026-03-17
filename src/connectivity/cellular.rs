/// Cellular modem for Genesis
///
/// Radio state management, SIM card handling,
/// network registration, signal monitoring, and data connection.
///
/// Inspired by: Android Telephony, RIL. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Radio technology
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioTech {
    None,
    Gsm,
    Cdma,
    Lte,
    Nr5g,
    NrSa,
}

/// SIM state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimState {
    Absent,
    PinRequired,
    PukRequired,
    Ready,
    Error,
}

/// Network registration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegState {
    NotRegistered,
    RegisteredHome,
    Searching,
    Denied,
    RegisteredRoaming,
    Unknown,
}

/// Signal strength
pub struct SignalStrength {
    pub rssi_dbm: i16,
    pub rsrp_dbm: i16,
    pub rsrq_db: i16,
    pub sinr_db: i16,
    pub level: u8, // 0-4 bars
}

/// Data connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataState {
    Disconnected,
    Connecting,
    Connected,
    Suspended,
}

/// SIM info
pub struct SimInfo {
    pub slot: u8,
    pub state: SimState,
    pub iccid: String,
    pub imsi: String,
    pub carrier_name: String,
    pub mcc: u16,
    pub mnc: u16,
}

/// Cellular modem
pub struct CellularModem {
    pub radio_on: bool,
    pub airplane_mode: bool,
    pub technology: RadioTech,
    pub reg_state: RegState,
    pub data_state: DataState,
    pub signal: SignalStrength,
    pub sims: Vec<SimInfo>,
    pub active_sim: u8,
    pub data_enabled: bool,
    pub data_roaming_enabled: bool,
    pub preferred_network: RadioTech,
    pub operator_name: String,
    pub cell_id: u32,
    pub tac: u16, // tracking area code
}

impl CellularModem {
    const fn new() -> Self {
        CellularModem {
            radio_on: false,
            airplane_mode: false,
            technology: RadioTech::None,
            reg_state: RegState::NotRegistered,
            data_state: DataState::Disconnected,
            signal: SignalStrength {
                rssi_dbm: -120,
                rsrp_dbm: -140,
                rsrq_db: -20,
                sinr_db: -3,
                level: 0,
            },
            sims: Vec::new(),
            active_sim: 0,
            data_enabled: true,
            data_roaming_enabled: false,
            preferred_network: RadioTech::Nr5g,
            operator_name: String::new(),
            cell_id: 0,
            tac: 0,
        }
    }

    pub fn power_on(&mut self) {
        if self.airplane_mode {
            return;
        }
        self.radio_on = true;
        self.reg_state = RegState::Searching;
    }

    pub fn power_off(&mut self) {
        self.radio_on = false;
        self.reg_state = RegState::NotRegistered;
        self.data_state = DataState::Disconnected;
    }

    pub fn set_airplane_mode(&mut self, on: bool) {
        self.airplane_mode = on;
        if on {
            self.power_off();
        }
    }

    pub fn update_signal(&mut self, rssi: i16, level: u8) {
        self.signal.rssi_dbm = rssi;
        self.signal.level = level;
    }

    pub fn register(&mut self, operator: &str, tech: RadioTech, roaming: bool) {
        self.operator_name = String::from(operator);
        self.technology = tech;
        self.reg_state = if roaming {
            RegState::RegisteredRoaming
        } else {
            RegState::RegisteredHome
        };
    }

    pub fn connect_data(&mut self) -> bool {
        if !self.radio_on || !self.data_enabled {
            return false;
        }
        if self.reg_state == RegState::RegisteredRoaming && !self.data_roaming_enabled {
            return false;
        }
        self.data_state = DataState::Connected;
        true
    }

    pub fn insert_sim(&mut self, slot: u8, carrier: &str, mcc: u16, mnc: u16) {
        self.sims.push(SimInfo {
            slot,
            state: SimState::Ready,
            iccid: String::new(),
            imsi: String::new(),
            carrier_name: String::from(carrier),
            mcc,
            mnc,
        });
    }

    pub fn is_connected(&self) -> bool {
        self.data_state == DataState::Connected
    }

    pub fn signal_bars(&self) -> u8 {
        self.signal.level
    }
}

static MODEM: Mutex<CellularModem> = Mutex::new(CellularModem::new());

pub fn init() {
    crate::serial_println!("  [connectivity] Cellular modem initialized");
}
