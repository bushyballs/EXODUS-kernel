use crate::sync::Mutex;
use alloc::vec;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IndoorSource {
    WifiFingerprint,
    Ble,
    Uwb,
    MagneticField,
    Barometer,
}

#[derive(Clone, Copy, Debug)]
pub struct FloorLevel {
    pub building_id: u32,
    pub floor: i8,      // Negative for basement levels
    pub confidence: u8, // 0-100
}

impl FloorLevel {
    pub fn new(building_id: u32, floor: i8, confidence: u8) -> Self {
        Self {
            building_id,
            floor,
            confidence: confidence.min(100),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct IndoorPosition {
    pub x_cm: i32, // X coordinate in centimeters (building-relative)
    pub y_cm: i32, // Y coordinate in centimeters (building-relative)
    pub floor: FloorLevel,
    pub accuracy_cm: u16, // Horizontal accuracy in centimeters
    pub source: IndoorSource,
    pub timestamp: u64,
}

impl IndoorPosition {
    pub fn new(
        x_cm: i32,
        y_cm: i32,
        floor: FloorLevel,
        accuracy_cm: u16,
        source: IndoorSource,
        timestamp: u64,
    ) -> Self {
        Self {
            x_cm,
            y_cm,
            floor,
            accuracy_cm,
            source,
            timestamp,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WifiFingerprint {
    pub bssid: [u8; 6], // MAC address
    pub rssi: i8,       // Signal strength
    pub channel: u8,
    pub timestamp: u64,
}

impl WifiFingerprint {
    pub fn new(bssid: [u8; 6], rssi: i8, channel: u8, timestamp: u64) -> Self {
        Self {
            bssid,
            rssi,
            channel,
            timestamp,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BleBeacon {
    pub uuid: [u8; 16],
    pub major: u16,
    pub minor: u16,
    pub rssi: i8,
    pub tx_power: i8,
    pub timestamp: u64,
}

impl BleBeacon {
    pub fn new(
        uuid: [u8; 16],
        major: u16,
        minor: u16,
        rssi: i8,
        tx_power: i8,
        timestamp: u64,
    ) -> Self {
        Self {
            uuid,
            major,
            minor,
            rssi,
            tx_power,
            timestamp,
        }
    }

    /// Estimate distance in centimeters based on RSSI
    pub fn estimate_distance_cm(&self) -> u16 {
        // Path loss formula: RSSI = TxPower - 10 * n * log10(d)
        // Simplified for n=2 (free space): d = 10^((TxPower - RSSI) / 20)
        // Using integer approximation
        let ratio = (self.tx_power - self.rssi) as u16;

        // Lookup table for 10^(x/20) where x is 0-100
        // This is a rough approximation
        match ratio {
            0..=10 => 100,   // ~1m
            11..=20 => 200,  // ~2m
            21..=30 => 400,  // ~4m
            31..=40 => 800,  // ~8m
            41..=50 => 1600, // ~16m
            51..=60 => 3200, // ~32m
            _ => 6400,       // >64m
        }
    }
}

pub struct IndoorEngine {
    pub positions: Vec<IndoorPosition>,
    pub max_history: usize,
    pub wifi_fingerprints: Vec<WifiFingerprint>,
    pub wifi_fingerprints_count: u32,
    pub ble_beacons: Vec<BleBeacon>,
    pub ble_beacons_count: u16,
    pub current_floor: Option<FloorLevel>,
    pub last_barometer_pa: u32, // Barometric pressure in pascals
}

impl IndoorEngine {
    pub fn new() -> Self {
        Self {
            positions: vec![],
            max_history: 50,
            wifi_fingerprints: vec![],
            wifi_fingerprints_count: 0,
            ble_beacons: vec![],
            ble_beacons_count: 0,
            current_floor: None,
            last_barometer_pa: 0,
        }
    }

    /// Estimate indoor position based on available signals
    pub fn estimate_position(
        &mut self,
        wifi_scans: &[WifiFingerprint],
        ble_scans: &[BleBeacon],
        timestamp: u64,
    ) -> Option<IndoorPosition> {
        // Priority: UWB > BLE > WiFi

        // For now, implement simple BLE trilateration if we have 3+ beacons
        if ble_scans.len() >= 3 {
            return self.estimate_from_ble(ble_scans, timestamp);
        }

        // Fallback to WiFi fingerprinting
        if !wifi_scans.is_empty() {
            return self.estimate_from_wifi(wifi_scans, timestamp);
        }

        None
    }

    fn estimate_from_ble(
        &mut self,
        beacons: &[BleBeacon],
        timestamp: u64,
    ) -> Option<IndoorPosition> {
        // Simple weighted centroid based on signal strength
        // In a real system, this would use proper trilateration

        if beacons.is_empty() {
            return None;
        }

        let mut weighted_x: i64 = 0;
        let mut weighted_y: i64 = 0;
        let mut total_weight: u32 = 0;

        for (i, beacon) in beacons.iter().take(10).enumerate() {
            // Use RSSI as weight (higher = closer = more weight)
            let weight = (100 + beacon.rssi as i32).max(0) as u32;

            // Distribute beacons in a circle for demo (real system would have known positions)
            let angle = (i as i32 * 360) / beacons.len() as i32;
            let distance = beacon.estimate_distance_cm();

            // Simple trig approximation
            let x = (distance as i32 * Self::cos_approx(angle)) / 1000;
            let y = (distance as i32 * Self::sin_approx(angle)) / 1000;

            weighted_x += x as i64 * weight as i64;
            weighted_y += y as i64 * weight as i64;
            total_weight += weight;
        }

        if total_weight == 0 {
            return None;
        }

        let x_cm = (weighted_x / total_weight as i64) as i32;
        let y_cm = (weighted_y / total_weight as i64) as i32;

        // Estimate accuracy based on number of beacons
        let accuracy_cm = match beacons.len() {
            0..=2 => 500, // 5m
            3..=5 => 200, // 2m
            _ => 100,     // 1m
        };

        let floor = self.current_floor.unwrap_or(FloorLevel::new(0, 0, 50));

        Some(IndoorPosition::new(
            x_cm,
            y_cm,
            floor,
            accuracy_cm,
            IndoorSource::Ble,
            timestamp,
        ))
    }

    fn estimate_from_wifi(
        &mut self,
        scans: &[WifiFingerprint],
        timestamp: u64,
    ) -> Option<IndoorPosition> {
        // WiFi fingerprinting would compare against known database
        // For now, return a default position with low confidence

        if scans.is_empty() {
            return None;
        }

        let floor = self.current_floor.unwrap_or(FloorLevel::new(0, 0, 30));

        Some(IndoorPosition::new(
            0,
            0,
            floor,
            1000,
            IndoorSource::WifiFingerprint,
            timestamp,
        ))
    }

    pub fn add_wifi_fingerprint(&mut self, fingerprint: WifiFingerprint) {
        if self.wifi_fingerprints.len() >= 100 {
            self.wifi_fingerprints.remove(0);
        }
        self.wifi_fingerprints.push(fingerprint);
        self.wifi_fingerprints_count = self.wifi_fingerprints_count.saturating_add(1);
    }

    pub fn add_ble_beacon(&mut self, beacon: BleBeacon) {
        if self.ble_beacons.len() >= 50 {
            self.ble_beacons.remove(0);
        }
        self.ble_beacons.push(beacon);
        self.ble_beacons_count = self.ble_beacons_count.saturating_add(1);
    }

    /// Detect floor change using barometric pressure
    pub fn detect_floor_change(&mut self, pressure_pa: u32, _timestamp: u64) -> Option<i8> {
        if self.last_barometer_pa == 0 {
            self.last_barometer_pa = pressure_pa;
            return None;
        }

        // Pressure change of ~12 Pa per meter altitude
        // Typical floor height is 3-4 meters
        let pressure_diff = self.last_barometer_pa as i32 - pressure_pa as i32;
        let floor_change = pressure_diff / 40; // ~40 Pa per floor

        if floor_change.abs() >= 1 {
            self.last_barometer_pa = pressure_pa;

            if let Some(ref mut floor) = self.current_floor {
                floor.floor += floor_change as i8;
                return Some(floor_change as i8);
            }
        }

        None
    }

    pub fn get_accuracy(&self) -> u16 {
        if let Some(pos) = self.positions.last() {
            pos.accuracy_cm
        } else {
            u16::MAX
        }
    }

    fn cos_approx(deg: i32) -> i32 {
        // Simple cosine approximation (returns value * 1000)
        let deg = deg % 360;
        let deg = if deg < 0 { deg + 360 } else { deg };

        match deg {
            0..=45 => 1000 - (deg * 11) / 5,             // 1.0 -> 0.7
            46..=90 => 707 - ((deg - 45) * 15) / 5,      // 0.7 -> 0.0
            91..=135 => -((deg - 90) * 15) / 5,          // 0.0 -> -0.7
            136..=180 => -707 - ((deg - 135) * 11) / 5,  // -0.7 -> -1.0
            181..=225 => -1000 + ((deg - 180) * 11) / 5, // -1.0 -> -0.7
            226..=270 => -707 + ((deg - 225) * 15) / 5,  // -0.7 -> 0.0
            271..=315 => ((deg - 270) * 15) / 5,         // 0.0 -> 0.7
            _ => 707 + ((deg - 315) * 11) / 5,           // 0.7 -> 1.0
        }
    }

    fn sin_approx(deg: i32) -> i32 {
        // sin(x) = cos(90 - x)
        Self::cos_approx(90 - deg)
    }
}

static INDOOR: Mutex<Option<IndoorEngine>> = Mutex::new(None);

pub fn init() {
    let mut indoor = INDOOR.lock();
    *indoor = Some(IndoorEngine::new());
    serial_println!("[INDOOR] Indoor positioning engine initialized");
}

pub fn get_engine() -> &'static Mutex<Option<IndoorEngine>> {
    &INDOOR
}
