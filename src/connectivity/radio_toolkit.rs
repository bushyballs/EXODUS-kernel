use crate::sync::Mutex;
/// Radio toolkit for Genesis — Flipper Zero-style capabilities
///
/// Multi-band radio scanning, WiFi discovery & bridging,
/// Bluetooth mesh relay, sub-GHz transceiver (433/868/915 MHz),
/// NFC/RFID read/write/emulate, IR transmit/receive/learn,
/// signal analysis, spectrum analyzer, packet capture.
///
/// Designed for Raspberry Pi, PIC, and embedded boards
/// with attached radio modules (CC1101, nRF24, PN532, etc.)
///
/// IMPORTANT: This module is for authorized testing,
/// research, and personal device management only.
///
/// Original implementation for Hoags OS.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Radio Bands ──

#[derive(Clone, Copy, PartialEq)]
pub enum RadioBand {
    SubGhz315, // 315 MHz (garage doors, car keys)
    SubGhz433, // 433 MHz (weather stations, remotes)
    SubGhz868, // 868 MHz (EU IoT, LoRa)
    SubGhz915, // 915 MHz (US LoRa, ISM)
    Wifi2_4,   // 2.4 GHz WiFi
    Wifi5,     // 5 GHz WiFi
    Bluetooth, // 2.4 GHz BLE
    Nfc13_56,  // 13.56 MHz NFC
    Rfid125,   // 125 kHz RFID
    Infrared,  // IR
}

// ── WiFi Scanner & Bridge ──

#[derive(Clone, Copy, PartialEq)]
pub enum WifiSecurityType {
    Open,
    Wep,
    WpaPersonal,
    Wpa2Personal,
    Wpa3Personal,
    Wpa2Enterprise,
    Unknown,
}

#[derive(Clone, Copy)]
pub struct WifiNetwork {
    pub ssid: [u8; 32],
    pub ssid_len: usize,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi_dbm: i16,
    pub security: WifiSecurityType,
    pub is_open: bool,
    pub has_captive_portal: bool,
}

// ── Sub-GHz Radio ──

#[derive(Clone, Copy)]
struct SubGhzCapture {
    frequency_hz: u32,
    modulation: Modulation,
    data: [u8; 128],
    data_len: usize,
    rssi_dbm: i16,
    timestamp: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Modulation {
    Ask,  // amplitude shift keying (OOK)
    Fsk,  // frequency shift keying
    Gfsk, // gaussian FSK
    Msk,  // minimum shift keying
    LoRa, // LoRa chirp spread spectrum
    Raw,
}

// ── NFC/RFID ──

#[derive(Clone, Copy, PartialEq)]
pub enum NfcType {
    MifareClassic,
    MifareUltralight,
    MifareDESFire,
    NtagNtag215,
    NfcTypeA,
    NfcTypeB,
    NfcTypeF,
    NfcTypeV,
    Iso14443,
    Iso15693,
}

#[derive(Clone, Copy)]
struct NfcCard {
    uid: [u8; 10],
    uid_len: usize,
    nfc_type: NfcType,
    atqa: [u8; 2],
    sak: u8,
    data_blocks: u16,
    saved: bool,
    label: [u8; 16],
    label_len: usize,
}

// ── IR ──

#[derive(Clone, Copy, PartialEq)]
pub enum IrProtocol {
    Nec,
    Rc5,
    Rc6,
    Samsung,
    Sony,
    Sharp,
    Raw,
}

#[derive(Clone, Copy)]
struct IrSignal {
    protocol: IrProtocol,
    address: u16,
    command: u16,
    repeat: u8,
    raw_timings: [u16; 128],
    raw_len: usize,
    label: [u8; 16],
    label_len: usize,
}

// ── Opportunistic Connectivity ──

#[derive(Clone, Copy, PartialEq)]
pub enum BridgeType {
    WifiToWifi,     // relay between WiFi networks
    CellToWifi,     // phone cellular -> WiFi hotspot -> us
    BtTether,       // Bluetooth PAN tethering
    LoRaMesh,       // LoRa mesh relay
    SatelliteRelay, // via satellite modem
}

struct ConnectivityBridge {
    bridge_type: BridgeType,
    source_device_hash: u64,
    active: bool,
    uplink_kbps: u32,
    downlink_kbps: u32,
    latency_ms: u32,
    data_transferred_bytes: u64,
    auto_discovered: bool,
}

// ── Main Engine ──

struct RadioToolkit {
    // WiFi scanning
    wifi_networks: Vec<WifiNetwork>,
    last_wifi_scan: u64,

    // Sub-GHz
    subghz_captures: Vec<SubGhzCapture>,
    subghz_frequency: u32,
    subghz_listening: bool,

    // NFC/RFID
    saved_nfc_cards: Vec<NfcCard>,
    last_nfc_read: Option<NfcCard>,

    // IR
    saved_ir_signals: Vec<IrSignal>,
    ir_learning: bool,

    // Connectivity bridges
    bridges: Vec<ConnectivityBridge>,
    active_bridge: Option<usize>,

    // Stats
    total_scans: u32,
    total_captures: u32,
    total_bridges: u32,
}

static RADIO: Mutex<Option<RadioToolkit>> = Mutex::new(None);

impl RadioToolkit {
    fn new() -> Self {
        RadioToolkit {
            wifi_networks: Vec::new(),
            last_wifi_scan: 0,
            subghz_captures: Vec::new(),
            subghz_frequency: 433920000, // 433.92 MHz default
            subghz_listening: false,
            saved_nfc_cards: Vec::new(),
            last_nfc_read: None,
            saved_ir_signals: Vec::new(),
            ir_learning: false,
            bridges: Vec::new(),
            active_bridge: None,
            total_scans: 0,
            total_captures: 0,
            total_bridges: 0,
        }
    }

    // ── WiFi Scanning ──

    fn scan_wifi(&mut self, timestamp: u64) {
        self.last_wifi_scan = timestamp;
        self.total_scans = self.total_scans.saturating_add(1);
        // In real implementation: send probe requests, listen for beacons
        // Hardware: ESP32, RTL8812AU, etc.
    }

    fn get_open_networks(&self) -> Vec<usize> {
        self.wifi_networks
            .iter()
            .enumerate()
            .filter(|(_, n)| n.is_open)
            .map(|(i, _)| i)
            .collect()
    }

    fn get_strongest_network(&self) -> Option<usize> {
        self.wifi_networks
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| n.rssi_dbm)
            .map(|(i, _)| i)
    }

    // ── Sub-GHz Radio ──

    fn start_subghz_listen(&mut self, frequency_hz: u32) {
        self.subghz_frequency = frequency_hz;
        self.subghz_listening = true;
        // In real implementation: configure CC1101/SX1276 radio
    }

    fn stop_subghz_listen(&mut self) {
        self.subghz_listening = false;
    }

    fn record_subghz(&mut self, data: &[u8], modulation: Modulation, rssi: i16, timestamp: u64) {
        let mut d = [0u8; 128];
        let dlen = data.len().min(128);
        d[..dlen].copy_from_slice(&data[..dlen]);
        if self.subghz_captures.len() < 200 {
            self.subghz_captures.push(SubGhzCapture {
                frequency_hz: self.subghz_frequency,
                modulation,
                data: d,
                data_len: dlen,
                rssi_dbm: rssi,
                timestamp,
            });
            self.total_captures = self.total_captures.saturating_add(1);
        }
    }

    fn replay_subghz(&self, capture_idx: usize) -> Option<&SubGhzCapture> {
        self.subghz_captures.get(capture_idx)
        // In real implementation: modulate and transmit via radio
    }

    // ── NFC/RFID ──

    fn read_nfc(&mut self) -> Option<&NfcCard> {
        // In real implementation: poll PN532/RC522 for card presence
        self.last_nfc_read.as_ref()
    }

    fn save_nfc_card(&mut self, card: NfcCard) {
        if self.saved_nfc_cards.len() < 100 {
            self.saved_nfc_cards.push(card);
        }
    }

    fn emulate_nfc(&self, card_idx: usize) -> Option<&NfcCard> {
        self.saved_nfc_cards.get(card_idx)
        // In real implementation: start card emulation mode
    }

    // ── IR ──

    fn start_ir_learn(&mut self) {
        self.ir_learning = true;
        // In real implementation: start IR receiver, decode protocol
    }

    fn save_ir_signal(&mut self, signal: IrSignal) {
        if self.saved_ir_signals.len() < 200 {
            self.saved_ir_signals.push(signal);
        }
    }

    fn transmit_ir(&self, signal_idx: usize) -> bool {
        self.saved_ir_signals.get(signal_idx).is_some()
        // In real implementation: modulate IR LED
    }

    // ── Opportunistic Connectivity Bridges ──

    fn discover_bridges(&mut self) {
        // Scan for any nearby device willing to share connectivity:
        // 1. Open WiFi networks
        // 2. Known phone's BT tether
        // 3. LoRa mesh nodes
        // 4. WiFi Direct peers
        self.total_scans = self.total_scans.saturating_add(1);
    }

    fn create_bridge(&mut self, btype: BridgeType, source_hash: u64) -> usize {
        let idx = self.bridges.len();
        self.bridges.push(ConnectivityBridge {
            bridge_type: btype,
            source_device_hash: source_hash,
            active: true,
            uplink_kbps: 0,
            downlink_kbps: 0,
            latency_ms: 0,
            data_transferred_bytes: 0,
            auto_discovered: true,
        });
        self.active_bridge = Some(idx);
        self.total_bridges = self.total_bridges.saturating_add(1);
        idx
    }

    fn auto_connect_best(&mut self) -> Option<usize> {
        // Priority: WiFi > BT tether > LoRa > Satellite
        // First try open WiFi
        if !self.wifi_networks.is_empty() {
            if let Some(idx) = self.get_strongest_network() {
                let net = &self.wifi_networks[idx];
                let hash = {
                    let mut h = 0u64;
                    for &b in &net.bssid {
                        h = h.wrapping_mul(31).wrapping_add(b as u64);
                    }
                    h
                };
                return Some(self.create_bridge(BridgeType::WifiToWifi, hash));
            }
        }
        // Then try BT tether (if paired phone available)
        // Then try LoRa mesh
        None
    }

    fn bridge_stats(&self) -> Option<(u32, u32, u32)> {
        self.active_bridge.and_then(|idx| {
            self.bridges
                .get(idx)
                .map(|b| (b.uplink_kbps, b.downlink_kbps, b.latency_ms))
        })
    }
}

pub fn init() {
    let mut radio = RADIO.lock();
    *radio = Some(RadioToolkit::new());
    serial_println!("    Radio toolkit: WiFi scan, sub-GHz, NFC/RFID, IR, mesh bridge ready");
}
