/// USB CDC-ECM (Ethernet Control Model) Driver — no-heap implementation
///
/// CDC-ECM provides a virtual Ethernet interface over USB.  The host OS
/// sees a standard NIC; the USB device presents itself as an ECM-class
/// function.
///
/// This driver manages up to MAX_CDC_ETH_DEVICES concurrent CDC-ECM
/// devices using fixed-size static buffers only.  No Vec, Box, String,
/// or alloc calls anywhere in this module.
///
/// Simulated probe: only VID=0x0525 / PID=0xA4A2 (Linux USB Ethernet
/// Gadget) is recognised in simulation mode.
///
/// Rules enforced:
///   - No heap (no Vec, Box, String, alloc::*)
///   - No floats (no as f32 / as f64)
///   - No panics (no unwrap, expect, panic!)
///   - Counters: saturating_add / saturating_sub
///   - Sequence numbers: wrapping_add
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

/// Maximum Ethernet frame payload (bytes)
pub const CDC_ETH_MTU: usize = 1500;

/// Ethernet header length (6 dst + 6 src + 2 ethertype)
pub const CDC_ETH_HEADER_LEN: usize = 14;

/// Maximum simultaneous CDC-ECM devices supported
pub const MAX_CDC_ETH_DEVICES: usize = 4;

/// CDC-ECM data interface number (standard ECM uses interface 1 for data)
pub const CDC_ECM_DATA_IFACE: u8 = 1;

/// USB Communications Device Class code
pub const CDC_CLASS: u8 = 0x02;

/// CDC Ethernet Control Model subclass
pub const CDC_SUBCLASS_ECM: u8 = 0x06;

/// Simulated probe VID (Linux USB Ethernet Gadget)
const PROBE_VID: u16 = 0x0525;

/// Simulated probe PID (Linux USB Ethernet Gadget)
const PROBE_PID: u16 = 0xA4A2;

/// Internal RX/TX buffer size — MTU + header + 6 bytes headroom
const BUF_SIZE: usize = 1520;

// ============================================================================
// State enum
// ============================================================================

/// Link state of a CDC-ECM device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdcEthState {
    /// No USB link; device not ready
    Disconnected,
    /// USB link is up; device is sending/receiving
    Connected,
}

// ============================================================================
// Device record
// ============================================================================

/// Per-device state for a CDC-ECM Ethernet gadget.
///
/// All fields are plain data — no heap allocations.  The struct is `Copy`
/// so it can be stored in a `static Mutex<[CdcEthDevice; N]>`.
#[derive(Clone, Copy)]
pub struct CdcEthDevice {
    /// Unique device identifier (monotonically assigned, wrapping_add)
    pub id: u32,
    /// Hardware MAC address
    pub mac: [u8; 6],
    /// Current link state
    pub state: CdcEthState,
    /// Maximum transmission unit in bytes
    pub mtu: u16,
    /// Receive frame buffer
    pub rx_buf: [u8; BUF_SIZE],
    /// Transmit frame buffer
    pub tx_buf: [u8; BUF_SIZE],
    /// Number of valid bytes in rx_buf (0 = empty)
    pub rx_len: u16,
    /// True when tx_buf holds a frame pending submission to the USB host
    pub tx_pending: bool,
    /// Total bytes transmitted (saturating)
    pub tx_bytes: u64,
    /// Total bytes received (saturating)
    pub rx_bytes: u64,
    /// Slot is occupied (false = free for reuse)
    pub active: bool,
}

impl CdcEthDevice {
    /// Construct a zero-initialised, inactive device slot.
    pub const fn empty() -> Self {
        CdcEthDevice {
            id: 0,
            mac: [0u8; 6],
            state: CdcEthState::Disconnected,
            mtu: CDC_ETH_MTU as u16,
            rx_buf: [0u8; BUF_SIZE],
            tx_buf: [0u8; BUF_SIZE],
            rx_len: 0,
            tx_pending: false,
            tx_bytes: 0,
            rx_bytes: 0,
            active: false,
        }
    }
}

// CdcEthDevice contains only plain integer types and byte arrays — safe to
// share across thread boundaries under the protection of the Mutex.
unsafe impl Send for CdcEthDevice {}

// ============================================================================
// Global device table
// ============================================================================

/// Global array of CDC-ECM device slots, protected by a spinlock.
static CDC_ETH_DEVS: Mutex<[CdcEthDevice; MAX_CDC_ETH_DEVICES]> =
    Mutex::new([CdcEthDevice::empty(); MAX_CDC_ETH_DEVICES]);

/// Next device ID to assign (wrapping).
static NEXT_ID: Mutex<u32> = Mutex::new(1);

// ============================================================================
// Probe
// ============================================================================

/// Attempt to detect a CDC-ECM device by USB Vendor/Product ID.
///
/// In simulation mode only VID=0x0525 / PID=0xA4A2 is recognised.
/// Returns `true` when the device is detected and can be registered.
pub fn cdc_eth_probe(vendor_id: u16, product_id: u16) -> bool {
    vendor_id == PROBE_VID && product_id == PROBE_PID
}

// ============================================================================
// Register
// ============================================================================

/// Allocate a device slot and record its MAC address.
///
/// Returns `Some(id)` on success or `None` when the table is full.
pub fn cdc_eth_register(mac: [u8; 6]) -> Option<u32> {
    let mut devs = CDC_ETH_DEVS.lock();

    // Find a free slot
    let mut slot_idx: Option<usize> = None;
    for (i, slot) in devs.iter().enumerate() {
        if !slot.active {
            slot_idx = Some(i);
            break;
        }
    }

    let idx = match slot_idx {
        Some(i) => i,
        None => {
            serial_println!("[usb_cdc_eth] device table full");
            return None;
        }
    };

    // Assign a unique id
    let id = {
        let mut id_lock = NEXT_ID.lock();
        let current = *id_lock;
        *id_lock = current.wrapping_add(1);
        current
    };

    let dev = &mut devs[idx];
    *dev = CdcEthDevice::empty();
    dev.id = id;
    dev.mac = mac;
    dev.active = true;

    serial_println!(
        "[usb_cdc_eth] registered device id={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        id,
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );

    Some(id)
}

// ============================================================================
// Connect / Disconnect
// ============================================================================

/// Mark the device as connected and register it with the netdev layer.
///
/// The netdev name is formatted as "eth_cdc{id}".  Returns `true` on success.
pub fn cdc_eth_connect(id: u32) -> bool {
    let mut devs = CDC_ETH_DEVS.lock();

    for dev in devs.iter_mut() {
        if dev.active && dev.id == id {
            dev.state = CdcEthState::Connected;

            // Register with netdev layer.  Build name as "eth_cdc{id}".
            // Maximum id digits that fit in a 16-byte name: "eth_cdc" = 7 chars,
            // leaving 8 digits for id + NUL.
            let mut net_dev = crate::net::netdev::NetDevice::zeroed();
            // Write name bytes manually — no alloc::format
            let name_prefix = b"eth_cdc";
            let mut name_buf = [0u8; 16];
            let prefix_len = name_prefix.len(); // 7
            name_buf[..prefix_len].copy_from_slice(name_prefix);
            // Append decimal digits of id
            let digit_start = prefix_len;
            let mut tmp = id;
            let mut digits = [0u8; 10];
            let mut ndigits = 0usize;
            if tmp == 0 {
                digits[0] = b'0';
                ndigits = 1;
            } else {
                while tmp > 0 && ndigits < digits.len() {
                    digits[ndigits] = b'0' + (tmp % 10) as u8;
                    tmp /= 10;
                    ndigits = ndigits.saturating_add(1);
                }
                // Digits are in reverse order — reverse in place
                let mut lo = 0usize;
                let mut hi = ndigits.saturating_sub(1);
                while lo < hi {
                    digits.swap(lo, hi);
                    lo = lo.saturating_add(1);
                    hi = hi.saturating_sub(1);
                }
            }
            let copy_len = ndigits.min(16usize.saturating_sub(digit_start).saturating_sub(1));
            name_buf[digit_start..digit_start + copy_len].copy_from_slice(&digits[..copy_len]);
            // NUL-terminate (array is already zero, but be explicit)
            let term_pos = (digit_start + copy_len).min(15);
            name_buf[term_pos] = 0;

            net_dev.name = name_buf;
            net_dev.mac = dev.mac;
            net_dev.up = true;
            // Use a dedicated driver index constant for CDC-ETH (index 3)
            net_dev.driver_idx = crate::net::netdev::DRIVER_CDC_ETH;
            let _ = crate::net::netdev::register_device(net_dev);

            serial_println!("[usb_cdc_eth] id={} connected", id);
            return true;
        }
    }

    serial_println!("[usb_cdc_eth] cdc_eth_connect: id={} not found", id);
    false
}

/// Mark the device as disconnected.
pub fn cdc_eth_disconnect(id: u32) {
    let mut devs = CDC_ETH_DEVS.lock();
    for dev in devs.iter_mut() {
        if dev.active && dev.id == id {
            dev.state = CdcEthState::Disconnected;
            serial_println!("[usb_cdc_eth] id={} disconnected", id);
            return;
        }
    }
    serial_println!("[usb_cdc_eth] cdc_eth_disconnect: id={} not found", id);
}

// ============================================================================
// Transmit
// ============================================================================

/// Copy `frame[..len]` into the device TX buffer and mark it pending.
///
/// Returns `false` if `id` is not found, the device is disconnected,
/// `len` is 0, or `len` exceeds `BUF_SIZE`.
pub fn cdc_eth_send(id: u32, frame: &[u8], len: usize) -> bool {
    if len == 0 || len > BUF_SIZE {
        return false;
    }

    let mut devs = CDC_ETH_DEVS.lock();
    for dev in devs.iter_mut() {
        if dev.active && dev.id == id {
            if dev.state != CdcEthState::Connected {
                return false;
            }
            // Safe: len <= BUF_SIZE checked above; frame slice must cover len
            let copy_len = len.min(frame.len()).min(BUF_SIZE);
            dev.tx_buf[..copy_len].copy_from_slice(&frame[..copy_len]);
            dev.tx_pending = true;
            dev.tx_bytes = dev.tx_bytes.saturating_add(copy_len as u64);
            return true;
        }
    }
    false
}

// ============================================================================
// Receive poll / read
// ============================================================================

/// Check whether a received frame is waiting in the RX buffer.
///
/// Returns `Some(len)` with the frame length, or `None` if no frame.
pub fn cdc_eth_recv_poll(id: u32) -> Option<usize> {
    let devs = CDC_ETH_DEVS.lock();
    for dev in devs.iter() {
        if dev.active && dev.id == id {
            if dev.rx_len > 0 {
                return Some(dev.rx_len as usize);
            } else {
                return None;
            }
        }
    }
    None
}

/// Copy the RX buffer contents into `out` and clear the pending frame.
///
/// Returns the number of bytes copied.  If no frame is ready, returns 0.
pub fn cdc_eth_read_frame(id: u32, out: &mut [u8; BUF_SIZE]) -> usize {
    let mut devs = CDC_ETH_DEVS.lock();
    for dev in devs.iter_mut() {
        if dev.active && dev.id == id {
            let len = dev.rx_len as usize;
            if len == 0 {
                return 0;
            }
            out[..len].copy_from_slice(&dev.rx_buf[..len]);
            dev.rx_len = 0;
            return len;
        }
    }
    0
}

// ============================================================================
// Test helper — simulate inbound frame
// ============================================================================

/// Inject a test frame into the device RX buffer.
///
/// Used in unit tests and simulation mode to exercise the receive path
/// without actual USB hardware.  Silently truncates to BUF_SIZE.
pub fn cdc_eth_simulate_rx(id: u32, data: &[u8], len: usize) {
    let copy_len = len.min(data.len()).min(BUF_SIZE);
    let mut devs = CDC_ETH_DEVS.lock();
    for dev in devs.iter_mut() {
        if dev.active && dev.id == id {
            dev.rx_buf[..copy_len].copy_from_slice(&data[..copy_len]);
            dev.rx_len = copy_len as u16;
            dev.rx_bytes = dev.rx_bytes.saturating_add(copy_len as u64);
            return;
        }
    }
    serial_println!("[usb_cdc_eth] simulate_rx: id={} not found", id);
}

// ============================================================================
// Module init
// ============================================================================

/// Initialise the CDC-ECM driver.
///
/// Probes for the simulated Linux USB Ethernet Gadget (VID=0x0525, PID=0xA4A2),
/// registers it, and brings the link up.
pub fn init() {
    if cdc_eth_probe(PROBE_VID, PROBE_PID) {
        // Use a locally-administered MAC for the simulated gadget
        let mac: [u8; 6] = [0x02, 0x52, 0x54, 0xCC, 0xEC, 0x01];
        if let Some(id) = cdc_eth_register(mac) {
            cdc_eth_connect(id);
        }
    }
    serial_println!("[usb_cdc_eth] CDC-ECM driver initialized");
    super::register("usb-cdc-eth", super::DeviceType::Network);
}
