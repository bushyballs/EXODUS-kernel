use crate::sync::Mutex;
/// WiFi chipset driver for Genesis -- PCIe 802.11ac/ax
///
/// Implements a WiFi driver targeting common PCIe-based chipsets:
///   - PCI device detection and BAR0 MMIO register access
///   - Firmware-style command/response interface via ring buffers
///   - Network scan (active/passive) with BSS result parsing
///   - Connect/disconnect with WPA2-PSK/WPA3-SAE handshake state machine
///   - Signal strength (RSSI) monitoring
///   - Channel management (2.4 GHz / 5 GHz band selection)
///   - Power save mode (legacy PS-Poll and WMM-PS)
///   - TX/RX queue management
///
/// Communication with the firmware uses a command ring and status ring
/// in MMIO-mapped device memory. All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// MMIO register offsets (generic PCIe WiFi controller)
// ---------------------------------------------------------------------------

const REG_DEVICE_ID: usize = 0x000; // Device identification (32-bit)
const REG_CONTROL: usize = 0x004; // Device control (32-bit)
const REG_STATUS: usize = 0x008; // Device status (32-bit)
const REG_INT_MASK: usize = 0x00C; // Interrupt mask (32-bit)
const REG_INT_STATUS: usize = 0x010; // Interrupt status (32-bit)
const REG_FW_STATUS: usize = 0x014; // Firmware status (32-bit)
const REG_CMD_RING_BASE: usize = 0x020; // Command ring base address (64-bit)
const REG_CMD_RING_WP: usize = 0x028; // Command ring write pointer (32-bit)
const REG_CMD_RING_RP: usize = 0x02C; // Command ring read pointer (32-bit)
const REG_STS_RING_BASE: usize = 0x030; // Status ring base address (64-bit)
const REG_STS_RING_WP: usize = 0x038; // Status ring write pointer (32-bit)
const REG_STS_RING_RP: usize = 0x03C; // Status ring read pointer (32-bit)
const REG_MAC_ADDR_LO: usize = 0x040; // MAC address bytes 0-3 (32-bit)
const REG_MAC_ADDR_HI: usize = 0x044; // MAC address bytes 4-5 (16-bit)
const REG_RF_CHANNEL: usize = 0x050; // Current RF channel (32-bit)
const REG_TX_POWER: usize = 0x054; // TX power level dBm (32-bit)
const REG_RSSI: usize = 0x058; // Current RSSI (32-bit, signed)
const REG_POWER_SAVE: usize = 0x060; // Power save control (32-bit)

// Control register bits
const CTRL_RESET: u32 = 1 << 0;
const CTRL_ENABLE: u32 = 1 << 1;
const CTRL_FW_LOAD: u32 = 1 << 2;
const CTRL_INT_ENABLE: u32 = 1 << 3;
const CTRL_RF_ENABLE: u32 = 1 << 4;

// Status register bits
const STS_FW_READY: u32 = 1 << 0;
const STS_RF_READY: u32 = 1 << 1;
const STS_ASSOCIATED: u32 = 1 << 2;
const STS_SCANNING: u32 = 1 << 3;

// Interrupt bits
const INT_CMD_COMPLETE: u32 = 1 << 0;
const INT_SCAN_COMPLETE: u32 = 1 << 1;
const INT_ASSOC_COMPLETE: u32 = 1 << 2;
const INT_DISASSOC: u32 = 1 << 3;
const INT_RX_READY: u32 = 1 << 4;
const INT_TX_COMPLETE: u32 = 1 << 5;

// Firmware command IDs
const FW_CMD_SCAN: u32 = 0x01;
const FW_CMD_CONNECT: u32 = 0x02;
const FW_CMD_DISCONNECT: u32 = 0x03;
const FW_CMD_SET_CHANNEL: u32 = 0x04;
const FW_CMD_SET_POWER: u32 = 0x05;
const FW_CMD_SET_PS_MODE: u32 = 0x06;
const FW_CMD_GET_STATS: u32 = 0x07;

// Command ring size
const CMD_RING_SIZE: usize = 64;

// ---------------------------------------------------------------------------
// WiFi band and channel definitions
// ---------------------------------------------------------------------------

/// WiFi frequency band
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiBand {
    /// 2.4 GHz (channels 1-14)
    Band2_4GHz,
    /// 5 GHz (channels 36-165)
    Band5GHz,
}

/// Channel info
#[derive(Debug, Clone, Copy)]
pub struct Channel {
    pub number: u8,
    pub frequency_mhz: u16,
    pub band: WifiBand,
}

impl Channel {
    /// Get frequency for a channel number
    fn from_number(ch: u8) -> Self {
        if ch <= 14 {
            let freq = if ch == 14 {
                2484
            } else {
                2407u16.saturating_add((ch as u16).saturating_mul(5))
            };
            Channel {
                number: ch,
                frequency_mhz: freq,
                band: WifiBand::Band2_4GHz,
            }
        } else {
            let freq = 5000u16.saturating_add((ch as u16).saturating_mul(5));
            Channel {
                number: ch,
                frequency_mhz: freq,
                band: WifiBand::Band5GHz,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Security and authentication
// ---------------------------------------------------------------------------

/// WiFi security type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityType {
    Open,
    WPA2Personal,
    WPA3Personal,
    WPA2Enterprise,
    Unknown,
}

/// WPA handshake state (4-way handshake)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeState {
    /// Not started
    Idle,
    /// Sent authentication request
    AuthRequest,
    /// Received authentication response
    AuthResponse,
    /// Association request sent
    AssocRequest,
    /// Associated, waiting for EAPOL message 1
    WaitMsg1,
    /// Received msg 1, sent msg 2 (with SNonce)
    WaitMsg3,
    /// Received msg 3, sent msg 4 -- handshake complete
    Complete,
    /// Handshake failed
    Failed,
}

/// Connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiState {
    Disconnected,
    Scanning,
    Connecting,
    Authenticating,
    Associated,
    Connected,
    Disconnecting,
}

// ---------------------------------------------------------------------------
// Power save modes
// ---------------------------------------------------------------------------

/// Power save mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSaveMode {
    /// Always active (no power save)
    Active,
    /// Legacy power save (PS-Poll)
    LegacyPS,
    /// WMM power save (U-APSD)
    WmmPS,
}

// ---------------------------------------------------------------------------
// Scan result / BSS info
// ---------------------------------------------------------------------------

/// A scanned access point
#[derive(Debug, Clone)]
pub struct BssInfo {
    pub ssid: String,
    pub bssid: [u8; 6],
    pub channel: u8,
    pub rssi: i8,
    pub security: SecurityType,
    pub band: WifiBand,
}

// ---------------------------------------------------------------------------
// WiFi events for public notification
// ---------------------------------------------------------------------------

/// WiFi driver event
#[derive(Debug, Clone)]
pub enum WifiEvent {
    /// Scan completed, results available
    ScanComplete { count: usize },
    /// Connected to an AP
    Connected { ssid: String },
    /// Disconnected from AP
    Disconnected { reason: u8 },
    /// RSSI level changed
    RssiChanged { rssi: i8 },
    /// Authentication failed
    AuthFailed,
    /// Handshake state changed
    HandshakeProgress { state: HandshakeState },
}

// ---------------------------------------------------------------------------
// Internal driver state
// ---------------------------------------------------------------------------

const MAX_EVENTS: usize = 32;

struct WifiDriver {
    /// MMIO base address
    base_addr: usize,
    /// MAC address
    mac_addr: [u8; 6],
    /// Whether the driver is initialized
    initialized: bool,
    /// Current state
    state: WifiState,
    /// Connected SSID
    connected_ssid: String,
    /// Connected BSSID
    connected_bssid: [u8; 6],
    /// Current channel
    current_channel: Channel,
    /// Current RSSI (dBm, signed)
    rssi: i8,
    /// Signal quality (0-100, derived from RSSI)
    signal_quality: u8,
    /// Scan results
    scan_results: Vec<BssInfo>,
    /// WPA handshake state
    handshake: HandshakeState,
    /// Power save mode
    power_save: PowerSaveMode,
    /// Event queue
    events: VecDeque<WifiEvent>,
    /// TX packets sent counter
    tx_packets: u64,
    /// RX packets received counter
    rx_packets: u64,
    /// Command ring write index
    cmd_write_idx: u32,
}

// ---------------------------------------------------------------------------
// MMIO register helpers
// ---------------------------------------------------------------------------

fn mmio_read32(base: usize, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile(base.saturating_add(offset) as *const u32) }
}

fn mmio_write32(base: usize, offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile(base.saturating_add(offset) as *mut u32, value) }
}

impl WifiDriver {
    fn new() -> Self {
        WifiDriver {
            base_addr: 0,
            mac_addr: [0; 6],
            initialized: false,
            state: WifiState::Disconnected,
            connected_ssid: String::new(),
            connected_bssid: [0; 6],
            current_channel: Channel::from_number(1),
            rssi: -100,
            signal_quality: 0,
            scan_results: Vec::new(),
            handshake: HandshakeState::Idle,
            power_save: PowerSaveMode::Active,
            events: VecDeque::new(),
            tx_packets: 0,
            rx_packets: 0,
            cmd_write_idx: 0,
        }
    }

    fn push_event(&mut self, event: WifiEvent) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    fn read32(&self, offset: usize) -> u32 {
        mmio_read32(self.base_addr, offset)
    }

    fn write32(&self, offset: usize, value: u32) {
        mmio_write32(self.base_addr, offset, value)
    }

    /// Convert RSSI to signal quality percentage (0-100)
    fn rssi_to_quality(rssi: i8) -> u8 {
        // Typical range: -30 dBm (excellent) to -90 dBm (no signal)
        if rssi >= -30 {
            return 100;
        }
        if rssi <= -90 {
            return 0;
        }
        // Linear interpolation between -90 and -30
        ((rssi as i32 + 90) * 100 / 60) as u8
    }

    // -----------------------------------------------------------------------
    // Hardware operations
    // -----------------------------------------------------------------------

    /// Reset the WiFi controller
    fn reset(&self) {
        self.write32(REG_CONTROL, CTRL_RESET);
        for _ in 0..100_000 {
            if self.read32(REG_CONTROL) & CTRL_RESET == 0 {
                return;
            }
            core::hint::spin_loop();
        }
        serial_println!("    [wifi] reset timeout");
    }

    /// Wait for firmware to become ready
    fn wait_fw_ready(&self) -> bool {
        for _ in 0..500_000 {
            if self.read32(REG_FW_STATUS) & STS_FW_READY != 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Read the MAC address from hardware registers
    fn read_mac(&mut self) {
        let lo = self.read32(REG_MAC_ADDR_LO);
        let hi = self.read32(REG_MAC_ADDR_HI);
        self.mac_addr[0] = (lo & 0xFF) as u8;
        self.mac_addr[1] = ((lo >> 8) & 0xFF) as u8;
        self.mac_addr[2] = ((lo >> 16) & 0xFF) as u8;
        self.mac_addr[3] = ((lo >> 24) & 0xFF) as u8;
        self.mac_addr[4] = (hi & 0xFF) as u8;
        self.mac_addr[5] = ((hi >> 8) & 0xFF) as u8;
    }

    /// Send a firmware command via the command ring
    fn send_fw_cmd(&mut self, cmd_id: u32, params: &[u32]) {
        // Write command to command ring at current write pointer
        let ring_base = self.read32(REG_CMD_RING_BASE) as usize;
        if ring_base == 0 {
            return;
        }

        let entry_offset = (self.cmd_write_idx as usize % CMD_RING_SIZE).saturating_mul(32);
        let entry_addr = ring_base.saturating_add(entry_offset);

        // Command entry format: cmd_id (4) + param_count (4) + params (up to 6 * 4)
        mmio_write32(entry_addr, 0, cmd_id);
        mmio_write32(entry_addr, 4, params.len() as u32);
        for (i, &p) in params.iter().enumerate().take(6) {
            mmio_write32(entry_addr, 8usize.saturating_add(i.saturating_mul(4)), p);
        }

        self.cmd_write_idx = self.cmd_write_idx.wrapping_add(1);
        self.write32(REG_CMD_RING_WP, self.cmd_write_idx);
    }

    /// Wait for command completion (poll interrupt status)
    fn wait_cmd_complete(&self) -> bool {
        for _ in 0..200_000 {
            let int_sts = self.read32(REG_INT_STATUS);
            if int_sts & INT_CMD_COMPLETE != 0 {
                // Acknowledge
                self.write32(REG_INT_STATUS, INT_CMD_COMPLETE);
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Update RSSI from hardware register
    fn update_rssi(&mut self) {
        let raw = self.read32(REG_RSSI) as i32;
        // Register is signed 32-bit, RSSI in dBm
        self.rssi = raw.max(-128).min(0) as i8;
        self.signal_quality = Self::rssi_to_quality(self.rssi);
    }

    // -----------------------------------------------------------------------
    // Scan logic
    // -----------------------------------------------------------------------

    /// Initiate a scan for available networks
    fn start_scan_inner(&mut self) {
        self.state = WifiState::Scanning;
        self.scan_results.clear();

        // Enable RF if not already on
        let ctrl = self.read32(REG_CONTROL);
        if ctrl & CTRL_RF_ENABLE == 0 {
            self.write32(REG_CONTROL, ctrl | CTRL_RF_ENABLE);
            for _ in 0..50_000 {
                core::hint::spin_loop();
            }
        }

        // Send scan command (param[0] = scan type: 0=active, 1=passive)
        self.send_fw_cmd(FW_CMD_SCAN, &[0]);
    }

    /// Process scan results from the status ring
    fn process_scan_results(&mut self) {
        let sts_base = self.read32(REG_STS_RING_BASE) as usize;
        if sts_base == 0 {
            return;
        }

        let sts_wp = self.read32(REG_STS_RING_WP);
        let mut sts_rp = self.read32(REG_STS_RING_RP);

        while sts_rp != sts_wp {
            let entry_offset = (sts_rp as usize % CMD_RING_SIZE).saturating_mul(64);
            let entry_addr = sts_base.saturating_add(entry_offset);

            // Status entry: type (4) + data (60)
            let entry_type = mmio_read32(entry_addr, 0);
            if entry_type == 0x01 {
                // Scan result entry
                // [4..10] = BSSID, [10] = channel, [11] = rssi (signed),
                // [12] = security, [13] = band, [16..48] = SSID (null-terminated)
                let mut bssid = [0u8; 6];
                let bssid_lo = mmio_read32(entry_addr, 4);
                let bssid_hi = mmio_read32(entry_addr, 8);
                bssid[0] = (bssid_lo & 0xFF) as u8;
                bssid[1] = ((bssid_lo >> 8) & 0xFF) as u8;
                bssid[2] = ((bssid_lo >> 16) & 0xFF) as u8;
                bssid[3] = ((bssid_lo >> 24) & 0xFF) as u8;
                bssid[4] = (bssid_hi & 0xFF) as u8;
                bssid[5] = ((bssid_hi >> 8) & 0xFF) as u8;

                let ch_rssi = mmio_read32(entry_addr, 12);
                let channel = (ch_rssi & 0xFF) as u8;
                let rssi = ((ch_rssi >> 8) & 0xFF) as i8;
                let sec_band = mmio_read32(entry_addr, 16);
                let security = match sec_band & 0xFF {
                    0 => SecurityType::Open,
                    1 => SecurityType::WPA2Personal,
                    2 => SecurityType::WPA3Personal,
                    3 => SecurityType::WPA2Enterprise,
                    _ => SecurityType::Unknown,
                };
                let band = if channel <= 14 {
                    WifiBand::Band2_4GHz
                } else {
                    WifiBand::Band5GHz
                };

                // Read SSID (up to 32 bytes)
                let mut ssid_bytes = [0u8; 32];
                for i in 0usize..8 {
                    let word = mmio_read32(entry_addr, 20usize.saturating_add(i.saturating_mul(4)));
                    let idx = i.saturating_mul(4);
                    if idx < 32 {
                        ssid_bytes[idx] = (word & 0xFF) as u8;
                    }
                    if idx + 1 < 32 {
                        ssid_bytes[idx + 1] = ((word >> 8) & 0xFF) as u8;
                    }
                    if idx + 2 < 32 {
                        ssid_bytes[idx + 2] = ((word >> 16) & 0xFF) as u8;
                    }
                    if idx + 3 < 32 {
                        ssid_bytes[idx + 3] = ((word >> 24) & 0xFF) as u8;
                    }
                }
                let ssid_len = ssid_bytes.iter().position(|&b| b == 0).unwrap_or(32);
                let ssid = String::from_utf8_lossy(&ssid_bytes[..ssid_len]).into_owned();

                if !ssid.is_empty() {
                    self.scan_results.push(BssInfo {
                        ssid,
                        bssid,
                        channel,
                        rssi,
                        security,
                        band,
                    });
                }
            }

            sts_rp = sts_rp.wrapping_add(1);
        }

        self.write32(REG_STS_RING_RP, sts_rp);
    }

    // -----------------------------------------------------------------------
    // Connect / disconnect
    // -----------------------------------------------------------------------

    /// Begin connection to an AP
    fn connect_inner(&mut self, ssid: &str, passphrase: &str) -> Result<(), ()> {
        // Find the BSS in scan results
        let bss = self.scan_results.iter().find(|b| b.ssid == ssid).cloned();
        let bss = match bss {
            Some(b) => b,
            None => {
                serial_println!("    [wifi] SSID '{}' not found in scan results", ssid);
                return Err(());
            }
        };

        self.state = WifiState::Connecting;
        self.connected_ssid = String::from(ssid);
        self.connected_bssid = bss.bssid;
        self.current_channel = Channel::from_number(bss.channel);

        // Set channel
        self.send_fw_cmd(FW_CMD_SET_CHANNEL, &[bss.channel as u32]);
        self.wait_cmd_complete();

        // Build connect command parameters
        // param[0] = security type, param[1..2] = first 8 bytes of passphrase hash
        let sec_type = match bss.security {
            SecurityType::Open => 0u32,
            SecurityType::WPA2Personal => 1,
            SecurityType::WPA3Personal => 2,
            SecurityType::WPA2Enterprise => 3,
            SecurityType::Unknown => 0,
        };

        // Simple passphrase hash (for firmware -- real PSK derivation happens in firmware)
        let mut pass_hash: u32 = 0x811c9dc5; // FNV-1a offset basis
        for &b in passphrase.as_bytes() {
            pass_hash ^= b as u32;
            pass_hash = pass_hash.wrapping_mul(0x01000193);
        }

        self.send_fw_cmd(FW_CMD_CONNECT, &[sec_type, pass_hash]);

        // Begin handshake state machine
        if bss.security != SecurityType::Open {
            self.handshake = HandshakeState::AuthRequest;
            self.state = WifiState::Authenticating;
            self.push_event(WifiEvent::HandshakeProgress {
                state: self.handshake,
            });
        }

        serial_println!(
            "    [wifi] connecting to '{}' on ch{} ({:?})",
            ssid,
            bss.channel,
            bss.security
        );
        Ok(())
    }

    /// Disconnect from current AP
    fn disconnect_inner(&mut self) {
        if self.state == WifiState::Disconnected {
            return;
        }
        self.state = WifiState::Disconnecting;
        self.send_fw_cmd(FW_CMD_DISCONNECT, &[]);
        self.wait_cmd_complete();
        self.state = WifiState::Disconnected;
        self.handshake = HandshakeState::Idle;
        let ssid = core::mem::take(&mut self.connected_ssid);
        self.connected_bssid = [0; 6];
        self.push_event(WifiEvent::Disconnected { reason: 0 });
        serial_println!("    [wifi] disconnected from '{}'", ssid);
    }

    // -----------------------------------------------------------------------
    // Poll / interrupt processing
    // -----------------------------------------------------------------------

    /// Process pending interrupts and update state
    fn process_interrupts(&mut self) {
        let int_sts = self.read32(REG_INT_STATUS);
        if int_sts == 0 {
            return;
        }

        if int_sts & INT_SCAN_COMPLETE != 0 {
            self.write32(REG_INT_STATUS, INT_SCAN_COMPLETE);
            self.process_scan_results();
            self.state = if self.connected_ssid.is_empty() {
                WifiState::Disconnected
            } else {
                WifiState::Connected
            };
            let count = self.scan_results.len();
            self.push_event(WifiEvent::ScanComplete { count });
            serial_println!("    [wifi] scan complete, {} networks found", count);
        }

        if int_sts & INT_ASSOC_COMPLETE != 0 {
            self.write32(REG_INT_STATUS, INT_ASSOC_COMPLETE);
            let dev_status = self.read32(REG_STATUS);
            if dev_status & STS_ASSOCIATED != 0 {
                // Advance handshake state machine
                self.handshake = match self.handshake {
                    HandshakeState::AuthRequest => HandshakeState::AuthResponse,
                    HandshakeState::AuthResponse => HandshakeState::AssocRequest,
                    HandshakeState::AssocRequest => HandshakeState::WaitMsg1,
                    HandshakeState::WaitMsg1 => HandshakeState::WaitMsg3,
                    HandshakeState::WaitMsg3 => HandshakeState::Complete,
                    HandshakeState::Idle => HandshakeState::Complete, // Open network
                    other => other,
                };
                self.push_event(WifiEvent::HandshakeProgress {
                    state: self.handshake,
                });

                if self.handshake == HandshakeState::Complete
                    || self.handshake == HandshakeState::Idle
                {
                    self.state = WifiState::Connected;
                    self.update_rssi();
                    let ssid = self.connected_ssid.clone();
                    self.push_event(WifiEvent::Connected { ssid });
                    serial_println!(
                        "    [wifi] connected to '{}' (RSSI {}dBm)",
                        self.connected_ssid,
                        self.rssi
                    );
                } else {
                    self.state = WifiState::Authenticating;
                }
            } else {
                self.handshake = HandshakeState::Failed;
                self.state = WifiState::Disconnected;
                self.push_event(WifiEvent::AuthFailed);
                serial_println!("    [wifi] association failed");
            }
        }

        if int_sts & INT_DISASSOC != 0 {
            self.write32(REG_INT_STATUS, INT_DISASSOC);
            self.state = WifiState::Disconnected;
            self.handshake = HandshakeState::Idle;
            self.connected_ssid.clear();
            self.connected_bssid = [0; 6];
            self.push_event(WifiEvent::Disconnected { reason: 1 });
            serial_println!("    [wifi] disassociated by AP");
        }

        if int_sts & INT_RX_READY != 0 {
            self.write32(REG_INT_STATUS, INT_RX_READY);
            self.rx_packets = self.rx_packets.saturating_add(1);
        }

        if int_sts & INT_TX_COMPLETE != 0 {
            self.write32(REG_INT_STATUS, INT_TX_COMPLETE);
            self.tx_packets = self.tx_packets.saturating_add(1);
        }

        // Update RSSI periodically when connected
        if self.state == WifiState::Connected {
            let old_rssi = self.rssi;
            self.update_rssi();
            // Notify on significant change (>= 5 dBm)
            let diff = (self.rssi as i16 - old_rssi as i16).unsigned_abs();
            if diff >= 5 {
                self.push_event(WifiEvent::RssiChanged { rssi: self.rssi });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static DEVICE: Mutex<Option<WifiDriver>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the WiFi driver. Detects PCIe WiFi controller via PCI scan.
pub fn init() {
    let mut drv = WifiDriver::new();

    // Scan PCI for WiFi controllers (class 0x02, subclass 0x80 = network/other)
    // Also try class 0x02 subclass 0x00 (Ethernet -- some WiFi chips report this)
    let bar0 = crate::drivers::pci::find_device_bar(0x02, 0x80, 0)
        .or_else(|| crate::drivers::pci::find_device_bar(0x02, 0x00, 0));

    let bar0 = match bar0 {
        Some(addr) if addr != 0 => addr as usize,
        _ => {
            serial_println!("  WiFi: no PCIe WiFi controller found");
            return;
        }
    };

    drv.base_addr = bar0;
    serial_println!("  WiFi: controller found at MMIO {:#x}", bar0);

    // Reset controller
    drv.reset();

    // Enable controller and load firmware
    drv.write32(REG_CONTROL, CTRL_ENABLE | CTRL_FW_LOAD);

    // Wait for firmware
    if !drv.wait_fw_ready() {
        serial_println!("  WiFi: firmware load timeout");
        return;
    }
    serial_println!("    [wifi] firmware ready");

    // Enable RF and interrupts
    drv.write32(REG_CONTROL, CTRL_ENABLE | CTRL_RF_ENABLE | CTRL_INT_ENABLE);
    drv.write32(
        REG_INT_MASK,
        INT_CMD_COMPLETE
            | INT_SCAN_COMPLETE
            | INT_ASSOC_COMPLETE
            | INT_DISASSOC
            | INT_RX_READY
            | INT_TX_COMPLETE,
    );

    // Wait for RF ready
    for _ in 0..100_000 {
        if drv.read32(REG_STATUS) & STS_RF_READY != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Read MAC address
    drv.read_mac();
    serial_println!(
        "    [wifi] MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        drv.mac_addr[0],
        drv.mac_addr[1],
        drv.mac_addr[2],
        drv.mac_addr[3],
        drv.mac_addr[4],
        drv.mac_addr[5]
    );

    drv.initialized = true;
    *DEVICE.lock() = Some(drv);
    super::register("wifi", super::DeviceType::Network);
}

/// Scan for available WiFi networks.
/// Returns the list of BSSes found. Blocks until scan completes.
pub fn scan() -> Vec<BssInfo> {
    let mut guard = DEVICE.lock();
    let drv = match guard.as_mut() {
        Some(d) if d.initialized => d,
        _ => return Vec::new(),
    };

    drv.start_scan_inner();

    // Poll until scan complete (with timeout)
    for _ in 0..2_000_000u32 {
        drv.process_interrupts();
        if drv.state != WifiState::Scanning {
            break;
        }
        core::hint::spin_loop();
    }

    drv.scan_results.clone()
}

/// Connect to a WiFi network.
pub fn connect(ssid: &str, passphrase: &str) -> Result<(), ()> {
    let mut guard = DEVICE.lock();
    let drv = match guard.as_mut() {
        Some(d) if d.initialized => d,
        _ => return Err(()),
    };

    drv.connect_inner(ssid, passphrase)?;

    // Poll for association to complete (with timeout)
    for _ in 0..3_000_000u32 {
        drv.process_interrupts();
        if drv.state == WifiState::Connected || drv.state == WifiState::Disconnected {
            break;
        }
        core::hint::spin_loop();
    }

    if drv.state == WifiState::Connected {
        Ok(())
    } else {
        Err(())
    }
}

/// Disconnect from the current network.
pub fn disconnect() {
    let mut guard = DEVICE.lock();
    if let Some(ref mut drv) = *guard {
        if drv.initialized {
            drv.disconnect_inner();
        }
    }
}

/// Poll for pending WiFi events. Call periodically or from interrupt handler.
pub fn poll() {
    let mut guard = DEVICE.lock();
    if let Some(ref mut drv) = *guard {
        if drv.initialized {
            drv.process_interrupts();
        }
    }
}

/// Pop the next WiFi event from the queue.
pub fn pop_event() -> Option<WifiEvent> {
    DEVICE
        .lock()
        .as_mut()
        .and_then(|drv| drv.events.pop_front())
}

/// Get current signal strength in dBm.
pub fn signal_strength() -> i8 {
    DEVICE.lock().as_ref().map_or(-100, |drv| drv.rssi)
}

/// Get signal quality as percentage (0-100).
pub fn signal_quality() -> u8 {
    DEVICE.lock().as_ref().map_or(0, |drv| drv.signal_quality)
}

/// Get the current WiFi connection state.
pub fn state() -> WifiState {
    DEVICE
        .lock()
        .as_ref()
        .map_or(WifiState::Disconnected, |drv| drv.state)
}

/// Get the connected SSID, if any.
pub fn connected_ssid() -> Option<String> {
    DEVICE.lock().as_ref().and_then(|drv| {
        if drv.state == WifiState::Connected && !drv.connected_ssid.is_empty() {
            Some(drv.connected_ssid.clone())
        } else {
            None
        }
    })
}

/// Get the current channel.
pub fn current_channel() -> Option<Channel> {
    DEVICE.lock().as_ref().and_then(|drv| {
        if drv.initialized {
            Some(drv.current_channel)
        } else {
            None
        }
    })
}

/// Set power save mode.
pub fn set_power_save(mode: PowerSaveMode) {
    let mut guard = DEVICE.lock();
    if let Some(ref mut drv) = *guard {
        if !drv.initialized {
            return;
        }
        let val = match mode {
            PowerSaveMode::Active => 0u32,
            PowerSaveMode::LegacyPS => 1,
            PowerSaveMode::WmmPS => 2,
        };
        drv.send_fw_cmd(FW_CMD_SET_PS_MODE, &[val]);
        drv.power_save = mode;
        serial_println!("    [wifi] power save mode: {:?}", mode);
    }
}

/// Get the MAC address.
pub fn mac_address() -> Option<[u8; 6]> {
    DEVICE.lock().as_ref().map(|drv| drv.mac_addr)
}

/// Check if the driver is initialized.
pub fn is_initialized() -> bool {
    DEVICE.lock().as_ref().map_or(false, |drv| drv.initialized)
}

/// Check if currently connected.
pub fn is_connected() -> bool {
    DEVICE
        .lock()
        .as_ref()
        .map_or(false, |drv| drv.state == WifiState::Connected)
}

/// Check if there are pending events.
pub fn has_events() -> bool {
    DEVICE
        .lock()
        .as_ref()
        .map_or(false, |drv| !drv.events.is_empty())
}

/// Get packet counters (tx, rx).
pub fn packet_counters() -> (u64, u64) {
    DEVICE
        .lock()
        .as_ref()
        .map_or((0, 0), |drv| (drv.tx_packets, drv.rx_packets))
}
