use crate::sync::Mutex;
/// Satellite communication for Genesis
///
/// Emergency SOS via satellite, messaging without cell coverage,
/// GPS/GLONASS/Galileo/BeiDou positioning, Iridium/Globalstar
/// modem support, LEO constellation connectivity (Starlink, AST),
/// satellite IoT (LoRa/satellite hybrid).
///
/// This module enables Genesis devices to communicate via public
/// satellite networks when cellular/WiFi is unavailable — ideal
/// for Raspberry Pi, PIC, and embedded deployments in remote areas.
///
/// Original implementation for Hoags OS.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SatelliteNetwork {
    Iridium,
    Globalstar,
    Inmarsat,
    Thuraya,
    StarlinkDirect, // Starlink direct-to-cell
    AstSpaceMobile, // AST SpaceMobile
    Cospas,         // COSPAS-SARSAT (emergency only)
    Orbcomm,        // IoT/M2M
}

#[derive(Clone, Copy, PartialEq)]
pub enum GnssConstellation {
    Gps,
    Glonass,
    Galileo,
    BeiDou,
    Qzss,
    NavIC,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SatelliteState {
    Searching,
    Acquired,
    Connected,
    Transmitting,
    Receiving,
    NoSignal,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SatMessageType {
    EmergencySos,
    ShortMessage,
    LocationShare,
    IoTTelemetry,
    VoiceCall,
}

#[derive(Clone, Copy)]
pub struct GnssPosition {
    pub latitude: i32, // degrees * 1_000_000
    pub longitude: i32,
    pub altitude_m: i32,
    pub accuracy_m: u16,
    pub speed_kmh: u16,
    pub heading_deg: u16,
    pub satellites_used: u8,
    pub fix_type: GnssFix,
    pub timestamp: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GnssFix {
    NoFix,
    Fix2D,
    Fix3D,
    DgpsFix,
    RtkFloat,
    RtkFixed,
}

struct SatelliteMessage {
    id: u32,
    msg_type: SatMessageType,
    network: SatelliteNetwork,
    payload_hash: u64,
    payload_len: u16,
    sent: bool,
    acknowledged: bool,
    timestamp: u64,
    retry_count: u8,
}

struct SatelliteModem {
    network: SatelliteNetwork,
    state: SatelliteState,
    signal_strength: i16, // dBm
    frequency_mhz: u32,
    registered: bool,
    imei: [u8; 16],
    imei_len: usize,
}

struct VisibleSatellite {
    constellation: GnssConstellation,
    prn: u8,
    elevation_deg: u8,
    azimuth_deg: u16,
    snr_db: u8,
    used_in_fix: bool,
}

struct SatelliteEngine {
    modems: Vec<SatelliteModem>,
    messages: Vec<SatelliteMessage>,
    position: GnssPosition,
    visible_sats: Vec<VisibleSatellite>,
    gnss_enabled: [bool; 6], // one per constellation
    emergency_mode: bool,
    next_msg_id: u32,
    messages_sent: u32,
    messages_received: u32,
    sos_active: bool,
    // IoT telemetry for Pi/PIC boards
    iot_report_interval_secs: u32,
    last_iot_report: u64,
}

static SATELLITE: Mutex<Option<SatelliteEngine>> = Mutex::new(None);

impl SatelliteEngine {
    fn new() -> Self {
        SatelliteEngine {
            modems: Vec::new(),
            messages: Vec::new(),
            position: GnssPosition {
                latitude: 0,
                longitude: 0,
                altitude_m: 0,
                accuracy_m: 0,
                speed_kmh: 0,
                heading_deg: 0,
                satellites_used: 0,
                fix_type: GnssFix::NoFix,
                timestamp: 0,
            },
            visible_sats: Vec::new(),
            gnss_enabled: [true, true, true, true, false, false], // GPS, GLONASS, Galileo, BeiDou
            emergency_mode: false,
            next_msg_id: 1,
            messages_sent: 0,
            messages_received: 0,
            sos_active: false,
            iot_report_interval_secs: 3600, // hourly for IoT
            last_iot_report: 0,
        }
    }

    fn register_modem(&mut self, network: SatelliteNetwork, freq_mhz: u32) -> usize {
        let idx = self.modems.len();
        self.modems.push(SatelliteModem {
            network,
            state: SatelliteState::Searching,
            signal_strength: -120,
            frequency_mhz: freq_mhz,
            registered: false,
            imei: [0; 16],
            imei_len: 0,
        });
        idx
    }

    fn update_gnss(
        &mut self,
        lat: i32,
        lon: i32,
        alt: i32,
        accuracy: u16,
        sats: u8,
        fix: GnssFix,
        timestamp: u64,
    ) {
        self.position = GnssPosition {
            latitude: lat,
            longitude: lon,
            altitude_m: alt,
            accuracy_m: accuracy,
            speed_kmh: 0,
            heading_deg: 0,
            satellites_used: sats,
            fix_type: fix,
            timestamp,
        };
    }

    fn send_message(
        &mut self,
        msg_type: SatMessageType,
        network: SatelliteNetwork,
        payload_hash: u64,
        payload_len: u16,
        timestamp: u64,
    ) -> Option<u32> {
        // Find a connected modem on this network
        let modem = self
            .modems
            .iter()
            .find(|m| m.network == network && m.registered)?;
        let _ = modem; // used for validation

        let id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.saturating_add(1);
        self.messages.push(SatelliteMessage {
            id,
            msg_type,
            network,
            payload_hash,
            payload_len,
            sent: false,
            acknowledged: false,
            timestamp,
            retry_count: 0,
        });
        self.messages_sent = self.messages_sent.saturating_add(1);
        Some(id)
    }

    fn trigger_emergency_sos(&mut self, timestamp: u64) -> u32 {
        self.sos_active = true;
        self.emergency_mode = true;
        // Send on COSPAS-SARSAT first, then Iridium
        let id = self.next_msg_id;
        self.next_msg_id = self.next_msg_id.saturating_add(1);
        let pos_hash = (self.position.latitude as u64)
            .wrapping_mul(31)
            .wrapping_add(self.position.longitude as u64);
        self.messages.push(SatelliteMessage {
            id,
            msg_type: SatMessageType::EmergencySos,
            network: SatelliteNetwork::Cospas,
            payload_hash: pos_hash,
            payload_len: 32,
            sent: false,
            acknowledged: false,
            timestamp,
            retry_count: 0,
        });
        id
    }

    fn cancel_sos(&mut self) {
        self.sos_active = false;
        self.emergency_mode = false;
    }

    /// Send IoT telemetry via satellite (for Pi/PIC deployments)
    fn send_iot_telemetry(
        &mut self,
        sensor_data_hash: u64,
        data_len: u16,
        timestamp: u64,
    ) -> Option<u32> {
        if timestamp - self.last_iot_report < self.iot_report_interval_secs as u64 {
            return None; // Rate limited
        }
        self.last_iot_report = timestamp;
        // Prefer Orbcomm or Iridium for IoT
        let network = if self
            .modems
            .iter()
            .any(|m| m.network == SatelliteNetwork::Orbcomm && m.registered)
        {
            SatelliteNetwork::Orbcomm
        } else {
            SatelliteNetwork::Iridium
        };
        self.send_message(
            SatMessageType::IoTTelemetry,
            network,
            sensor_data_hash,
            data_len,
            timestamp,
        )
    }

    fn best_modem(&self) -> Option<usize> {
        self.modems
            .iter()
            .enumerate()
            .filter(|(_, m)| m.registered)
            .max_by_key(|(_, m)| m.signal_strength)
            .map(|(i, _)| i)
    }

    fn total_visible_satellites(&self) -> usize {
        self.visible_sats.len()
    }

    fn satellites_in_fix(&self) -> usize {
        self.visible_sats.iter().filter(|s| s.used_in_fix).count()
    }
}

pub fn init() {
    let mut engine = SATELLITE.lock();
    let mut sat = SatelliteEngine::new();
    // Register default modems
    sat.register_modem(SatelliteNetwork::Iridium, 1626);
    sat.register_modem(SatelliteNetwork::Cospas, 406);
    sat.register_modem(SatelliteNetwork::Orbcomm, 137);
    *engine = Some(sat);
    serial_println!("    Automotive/Satellite: GNSS (GPS/GLONASS/Galileo/BeiDou), Iridium, COSPAS-SARSAT, IoT relay ready");
}
