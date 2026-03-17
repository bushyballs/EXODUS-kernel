/// 802.11 WiFi Driver (Stub)
///
/// Provides WiFi device management, scanning, and connection state.
/// Supports up to 2 WiFi devices with scan result cache (32 networks).
/// All data structures are fixed-size, no heap allocation.
///
/// Inspired by: Linux cfg80211 / mac80211, standard 802.11 driver architecture.
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// WiFi State and Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiState {
    Off,
    Scanning,
    Associated,
    Disconnected,
}

impl WifiState {
    pub const fn default() -> Self {
        WifiState::Off
    }
}

// ---------------------------------------------------------------------------
// Scan Result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct WifiScanResult {
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub bssid: [u8; 6],
    pub rssi: i8, // Signal strength in dBm (typically -30 to -90)
    pub channel: u8,
    pub security: u8, // 0=open, 1=WEP, 2=WPA, 3=WPA2
    pub active: bool,
}

impl WifiScanResult {
    pub const fn empty() -> Self {
        WifiScanResult {
            ssid: [0; 32],
            ssid_len: 0,
            bssid: [0; 6],
            rssi: 0,
            channel: 0,
            security: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// WiFi Device
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct WifiDevice {
    pub id: u8,
    pub mac: [u8; 6],
    pub state: WifiState,
    pub channel: u8,
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub bssid: [u8; 6],
    pub rssi: i8,
    pub active: bool,
}

impl WifiDevice {
    pub const fn empty() -> Self {
        WifiDevice {
            id: 0,
            mac: [0; 6],
            state: WifiState::Off,
            channel: 0,
            ssid: [0; 32],
            ssid_len: 0,
            bssid: [0; 6],
            rssi: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global WiFi State
// ---------------------------------------------------------------------------

static WIFI_DEVICES: Mutex<[WifiDevice; 2]> = Mutex::new([WifiDevice::empty(); 2]);
static WIFI_SCAN: Mutex<[WifiScanResult; 32]> = Mutex::new([WifiScanResult::empty(); 32]);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Add a new WiFi device.
/// Returns device ID (0 or 1) if successful, None if no space available.
pub fn wifi_device_add(mac: &[u8; 6]) -> Option<u8> {
    let mut devices = WIFI_DEVICES.lock();

    for i in 0..2 {
        if !devices[i].active {
            devices[i] = WifiDevice {
                id: i as u8,
                mac: *mac,
                state: WifiState::Off,
                channel: 0,
                ssid: [0; 32],
                ssid_len: 0,
                bssid: [0; 6],
                rssi: 0,
                active: true,
            };
            return Some(i as u8);
        }
    }
    None
}

/// Start a WiFi scan on the specified device.
pub fn wifi_start_scan(dev_id: u8) -> bool {
    if dev_id >= 2 {
        return false;
    }

    let mut devices = WIFI_DEVICES.lock();
    if !devices[dev_id as usize].active {
        return false;
    }

    devices[dev_id as usize].state = WifiState::Scanning;
    true
}

/// Add a scan result to the global scan cache.
/// Returns true if added successfully, false if cache is full.
pub fn wifi_add_scan_result(
    ssid: &[u8; 32],
    ssid_len: u8,
    bssid: &[u8; 6],
    rssi: i8,
    channel: u8,
    security: u8,
) -> bool {
    let mut scan = WIFI_SCAN.lock();

    // Look for empty slot
    for i in 0..32 {
        if !scan[i].active {
            scan[i] = WifiScanResult {
                ssid: *ssid,
                ssid_len,
                bssid: *bssid,
                rssi,
                channel,
                security,
                active: true,
            };
            return true;
        }
    }
    false
}

/// Get all scan results.
/// Returns number of results written.
pub fn wifi_get_scan_results(out: &mut [WifiScanResult; 32]) -> usize {
    let scan = WIFI_SCAN.lock();

    let mut count = 0;
    for i in 0..32 {
        if scan[i].active {
            out[count] = scan[i];
            count += 1;
        }
    }
    count
}

/// Connect to a WiFi network.
pub fn wifi_connect(
    dev_id: u8,
    ssid: &[u8; 32],
    ssid_len: u8,
    bssid: &[u8; 6],
    channel: u8,
) -> bool {
    if dev_id >= 2 {
        return false;
    }

    let mut devices = WIFI_DEVICES.lock();
    if !devices[dev_id as usize].active {
        return false;
    }

    devices[dev_id as usize].state = WifiState::Associated;
    devices[dev_id as usize].ssid = *ssid;
    devices[dev_id as usize].ssid_len = ssid_len;
    devices[dev_id as usize].bssid = *bssid;
    devices[dev_id as usize].channel = channel;
    devices[dev_id as usize].rssi = -50; // Simulated signal strength
    true
}

/// Disconnect from WiFi network.
pub fn wifi_disconnect(dev_id: u8) -> bool {
    if dev_id >= 2 {
        return false;
    }

    let mut devices = WIFI_DEVICES.lock();
    if !devices[dev_id as usize].active {
        return false;
    }

    devices[dev_id as usize].state = WifiState::Disconnected;
    devices[dev_id as usize].ssid_len = 0;
    true
}

/// Get the current state of a WiFi device.
pub fn wifi_get_state(dev_id: u8) -> Option<WifiState> {
    if dev_id >= 2 {
        return None;
    }

    let devices = WIFI_DEVICES.lock();
    if !devices[dev_id as usize].active {
        return None;
    }

    Some(devices[dev_id as usize].state)
}

/// Get the RSSI (signal strength in dBm) of a WiFi device.
pub fn wifi_get_rssi(dev_id: u8) -> Option<i8> {
    if dev_id >= 2 {
        return None;
    }

    let devices = WIFI_DEVICES.lock();
    if !devices[dev_id as usize].active {
        return None;
    }

    Some(devices[dev_id as usize].rssi)
}

/// Initialize the WiFi driver.
pub fn init() {
    // Register one simulated device
    if let Some(dev_id) = wifi_device_add(&[0x00, 0x11, 0x22, 0x33, 0x44, 0x55]) {
        // Add simulated scan results
        let ssid1 = [
            b'H', b'o', b'a', b'g', b's', b'N', b'e', b't', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let _ = wifi_add_scan_result(&ssid1, 8, &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x01], -45, 6, 3);

        let ssid2 = [
            b'G', b'u', b'e', b's', b't', b'N', b'e', b't', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let _ = wifi_add_scan_result(&ssid2, 8, &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x02], -62, 11, 2);

        let ssid3 = [
            b'O', b'p', b'e', b'n', b'W', b'i', b'F', b'i', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let _ = wifi_add_scan_result(&ssid3, 8, &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x03], -75, 1, 0);
    }

    serial_println!("[wifi] 802.11 WiFi driver initialized");
}
