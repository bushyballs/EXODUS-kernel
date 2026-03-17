use crate::net::NetworkDriver;
/// Network device abstraction layer for Genesis
///
/// Provides a unified interface over heterogeneous NIC drivers.  Each
/// physical or virtual interface is represented by a `NetDevice` record
/// stored in a global device table.  Drivers implement the `NetDriver`
/// trait; the table maps device name strings to driver indices.
///
/// Supported device classes
///   "eth0"  — Intel E1000 (PCI Ethernet)
///   "lo"    — Loopback pseudo-driver
///   "wlan0" — Wi-Fi (future)
///
/// All code is original and `#![no_std]`.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// NetDevice record
// ---------------------------------------------------------------------------

/// Maximum number of simultaneously registered network devices.
const MAX_DEVICES: usize = 16;

/// A registered network device.
#[derive(Clone)]
pub struct NetDevice {
    /// Human-readable name: "eth0", "lo", "wlan0", …  (NUL-padded, ASCII)
    pub name: [u8; 16],
    /// Ethernet MAC address (all zeros for loopback).
    pub mac: [u8; 6],
    /// Assigned IPv4 address (0.0.0.0 = unconfigured).
    pub ip: [u8; 4],
    /// IPv4 subnet mask (0.0.0.0 = unconfigured).
    pub netmask: [u8; 4],
    /// True when the interface is administratively up.
    pub up: bool,
    /// Index into `DRIVER_TABLE` that dispatches send/recv for this device.
    pub driver_idx: u8,
}

impl NetDevice {
    pub const fn zeroed() -> Self {
        NetDevice {
            name: [0u8; 16],
            mac: [0u8; 6],
            ip: [0u8; 4],
            netmask: [0u8; 4],
            up: false,
            driver_idx: 0xFF, // sentinel: no driver
        }
    }

    /// Return the device name as a `&str` (trimmed at first NUL byte).
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&self.name[..end]).unwrap_or("?")
    }

    /// Write up to 15 ASCII bytes of `name` into `self.name`.
    pub fn set_name(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let len = bytes.len().min(15);
        self.name[..len].copy_from_slice(&bytes[..len]);
        self.name[len] = 0;
    }
}

// ---------------------------------------------------------------------------
// NetDriver trait
// ---------------------------------------------------------------------------

/// Trait that every NIC driver must implement.
///
/// Implementations live in `crate::drivers::*` and are registered via
/// `register_driver()`.  The `send`/`recv` methods MUST be safe to call from
/// interrupt context (i.e. no blocking allocations, no panics).
pub trait NetDriver: Send + Sync {
    /// Transmit a raw Ethernet frame.  Returns `true` on success.
    fn send(&self, buf: &[u8]) -> bool;

    /// Receive the next available Ethernet frame into `buf`.
    /// Returns the number of bytes written (0 = no frame available).
    fn recv(&self, buf: &mut [u8]) -> usize;

    /// Return the hardware MAC address for this driver.
    fn get_mac(&self) -> [u8; 6];
}

// ---------------------------------------------------------------------------
// Global device table
// ---------------------------------------------------------------------------

/// Internal slot in the device table.
struct DeviceSlot {
    dev: NetDevice,
    used: bool,
}

impl DeviceSlot {
    const fn empty() -> Self {
        DeviceSlot {
            dev: NetDevice::zeroed(),
            used: false,
        }
    }
}

static DEVICE_TABLE: Mutex<[DeviceSlot; MAX_DEVICES]> =
    Mutex::new([const { DeviceSlot::empty() }; MAX_DEVICES]);

// ---------------------------------------------------------------------------
// Driver dispatch table
// ---------------------------------------------------------------------------

/// Loopback RX ring — packets written to the loopback TX are enqueued here.
static LO_RX_RING: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

// ---------------------------------------------------------------------------
// Driver index constants
// ---------------------------------------------------------------------------

/// Driver index for the Intel E1000 Ethernet driver.
pub const DRIVER_E1000: u8 = 0;
/// Driver index for the software loopback driver.
pub const DRIVER_LOOPBACK: u8 = 1;
/// Driver index for the VirtIO network driver.
pub const DRIVER_VIRTIO_NET: u8 = 2;
/// Driver index for the USB CDC-ECM Ethernet driver.
pub const DRIVER_CDC_ETH: u8 = 3;

// ---------------------------------------------------------------------------
// Driver dispatch — send
// ---------------------------------------------------------------------------

fn driver_send(driver_idx: u8, buf: &[u8]) -> bool {
    match driver_idx {
        DRIVER_E1000 => {
            let driver = crate::drivers::e1000::driver().lock();
            if let Some(ref nic) = *driver {
                nic.send(buf).is_ok()
            } else {
                false
            }
        }
        DRIVER_LOOPBACK => {
            // Copy the frame into the loopback RX ring.
            let mut ring = LO_RX_RING.lock();
            ring.push(Vec::from(buf));
            true
        }
        DRIVER_VIRTIO_NET => crate::drivers::virtio_net::netdev_send(buf),
        DRIVER_CDC_ETH => {
            // CDC-ECM send: use device id=1 (the first registered gadget)
            crate::drivers::usb_cdc_eth::cdc_eth_send(1, buf, buf.len())
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Driver dispatch — recv
// ---------------------------------------------------------------------------

fn driver_recv(driver_idx: u8, buf: &mut [u8]) -> usize {
    match driver_idx {
        DRIVER_E1000 => {
            let driver = crate::drivers::e1000::driver().lock();
            if let Some(ref nic) = *driver {
                nic.recv(buf).unwrap_or(0)
            } else {
                0
            }
        }
        DRIVER_LOOPBACK => {
            let mut ring = LO_RX_RING.lock();
            if let Some(frame) = ring.first().cloned() {
                let len = frame.len().min(buf.len());
                buf[..len].copy_from_slice(&frame[..len]);
                ring.remove(0);
                len
            } else {
                0
            }
        }
        DRIVER_VIRTIO_NET => crate::drivers::virtio_net::netdev_recv(buf),
        DRIVER_CDC_ETH => {
            // CDC-ECM recv: poll device id=1 and copy into buf if a frame is ready
            if let Some(len) = crate::drivers::usb_cdc_eth::cdc_eth_recv_poll(1) {
                if buf.len() >= len {
                    // Temporary fixed-size buffer to satisfy the read_frame signature
                    let mut tmp = [0u8; 1520];
                    let n = crate::drivers::usb_cdc_eth::cdc_eth_read_frame(1, &mut tmp);
                    let copy = n.min(buf.len());
                    buf[..copy].copy_from_slice(&tmp[..copy]);
                    copy
                } else {
                    0
                }
            } else {
                0
            }
        }
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Driver dispatch — get MAC
// ---------------------------------------------------------------------------

fn driver_get_mac(driver_idx: u8) -> [u8; 6] {
    match driver_idx {
        DRIVER_E1000 => {
            let driver = crate::drivers::e1000::driver().lock();
            driver
                .as_ref()
                .map(|nic| {
                    let mac = nic.mac_addr();
                    mac.0
                })
                .unwrap_or([0u8; 6])
        }
        DRIVER_LOOPBACK => [0u8; 6], // Loopback has no MAC
        DRIVER_VIRTIO_NET => crate::drivers::virtio_net::virtio_net_get_mac().unwrap_or([0u8; 6]),
        DRIVER_CDC_ETH => {
            // Return the MAC of the first CDC-ECM gadget (locally-administered)
            [0x02, 0x52, 0x54, 0xCC, 0xEC, 0x01]
        }
        _ => [0u8; 6],
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new network device.
///
/// `dev.driver_idx` must be one of the `DRIVER_*` constants.
///
/// Returns `Ok(slot_index)` or `Err` if the table is full or a device with
/// the same name is already registered.
pub fn register_device(dev: NetDevice) -> Result<usize, &'static str> {
    let mut table = DEVICE_TABLE.lock();

    // Check for duplicate name
    for slot in table.iter() {
        if slot.used && slot.dev.name_str() == dev.name_str() {
            return Err("Device already registered");
        }
    }

    // Find a free slot
    for (i, slot) in table.iter_mut().enumerate() {
        if !slot.used {
            slot.dev = dev;
            slot.used = true;
            crate::serial_println!(
                "  netdev: registered '{}' driver={}",
                slot.dev.name_str(),
                slot.dev.driver_idx,
            );
            return Ok(i);
        }
    }
    Err("Device table full")
}

/// Look up a device by name (e.g. "eth0", "lo").
///
/// Returns a clone of the `NetDevice` record, or `None` if not found.
pub fn get_device(name: &str) -> Option<NetDevice> {
    let table = DEVICE_TABLE.lock();
    table
        .iter()
        .find(|s| s.used && s.dev.name_str() == name)
        .map(|s| s.dev.clone())
}

/// Bring an interface up: set `up = true` and send a DHCP Discover.
///
/// Returns `true` if the device was found, `false` otherwise.
pub fn bring_up(name: &str) -> bool {
    // Mark the device as up.
    {
        let mut table = DEVICE_TABLE.lock();
        let slot = table
            .iter_mut()
            .find(|s| s.used && s.dev.name_str() == name);
        match slot {
            Some(s) => {
                s.dev.up = true;
                crate::serial_println!("  netdev: {} is up", name);
            }
            None => return false,
        }
    }

    // For real Ethernet interfaces, kick off DHCP discovery.
    // Skip loopback (it has no DHCP server).
    if name != "lo" {
        crate::net::dhcp_discover();
    }

    true
}

/// Bring an interface down: set `up = false`.
pub fn bring_down(name: &str) {
    let mut table = DEVICE_TABLE.lock();
    if let Some(slot) = table
        .iter_mut()
        .find(|s| s.used && s.dev.name_str() == name)
    {
        slot.dev.up = false;
        crate::serial_println!("  netdev: {} is down", name);
    }
}

/// Transmit a raw Ethernet frame through the device occupying slot `iface_idx`
/// in the device table.
///
/// `iface_idx` is a zero-based index into the device table (as returned when
/// the device was registered).  Sends only the first `len` bytes of `frame`.
///
/// Returns `true` on success; `false` if the slot is empty, the device is
/// administratively down, or the driver rejects the packet.
pub fn driver_send_by_idx(iface_idx: u32, frame: &[u8; 1514], len: usize) -> bool {
    let len = len.min(1514);
    let (up, driver_idx) = {
        let table = DEVICE_TABLE.lock();
        let idx = iface_idx as usize;
        if idx >= MAX_DEVICES {
            return false;
        }
        let slot = &table[idx];
        if !slot.used {
            return false;
        }
        (slot.dev.up, slot.dev.driver_idx)
    };
    if !up {
        return false;
    }
    driver_send(driver_idx, &frame[..len])
}

/// Transmit a raw packet through the named interface's driver.
///
/// Returns `true` on success, `false` if the device is not found, is not up,
/// or the driver rejects the packet.
pub fn send_packet(name: &str, buf: &[u8]) -> bool {
    let (up, driver_idx) = {
        let table = DEVICE_TABLE.lock();
        match table.iter().find(|s| s.used && s.dev.name_str() == name) {
            Some(s) => (s.dev.up, s.dev.driver_idx),
            None => return false,
        }
    };

    if !up {
        return false;
    }

    driver_send(driver_idx, buf)
}

/// Receive the next available frame from the named interface's driver.
///
/// Returns the number of bytes written into `buf` (0 = no frame, or device
/// not found / not up).
pub fn recv_packet(name: &str, buf: &mut [u8]) -> usize {
    let (up, driver_idx) = {
        let table = DEVICE_TABLE.lock();
        match table.iter().find(|s| s.used && s.dev.name_str() == name) {
            Some(s) => (s.dev.up, s.dev.driver_idx),
            None => return 0,
        }
    };

    if !up {
        return 0;
    }

    let bytes = driver_recv(driver_idx, buf);
    // Network contact → belonging pulse (the organism is connected to others)
    if bytes > 0 {
        let tick = crate::life::life_tick::age();
        crate::life::belonging::contact(tick);
    }
    bytes
}

/// Update the IPv4 address and netmask of a registered device.
///
/// This is called by the DHCP client after a lease is obtained.
pub fn set_ip(name: &str, ip: [u8; 4], netmask: [u8; 4]) {
    let mut table = DEVICE_TABLE.lock();
    if let Some(slot) = table
        .iter_mut()
        .find(|s| s.used && s.dev.name_str() == name)
    {
        slot.dev.ip = ip;
        slot.dev.netmask = netmask;
        crate::serial_println!(
            "  netdev: {}: IP {}.{}.{}.{}/{}.{}.{}.{}",
            name,
            ip[0],
            ip[1],
            ip[2],
            ip[3],
            netmask[0],
            netmask[1],
            netmask[2],
            netmask[3],
        );
    }
}

/// List all registered devices (clones for external inspection).
pub fn list_devices() -> Vec<NetDevice> {
    let table = DEVICE_TABLE.lock();
    table
        .iter()
        .filter(|s| s.used)
        .map(|s| s.dev.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Loopback driver helpers
// ---------------------------------------------------------------------------

/// Flush all frames currently queued in the loopback RX ring without
/// processing them (useful during shutdown or reset).
pub fn lo_flush() {
    LO_RX_RING.lock().clear();
}

/// Return the number of frames waiting in the loopback RX ring.
pub fn lo_pending() -> usize {
    LO_RX_RING.lock().len()
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the network device layer and register the default devices.
///
/// Registers:
///   - "lo"   (DRIVER_LOOPBACK) — always present, brought up immediately
///   - "eth0" (DRIVER_E1000)    — brought up if the E1000 driver is active
pub fn init() {
    // Loopback device
    let mut lo = NetDevice::zeroed();
    lo.set_name("lo");
    lo.ip = [127, 0, 0, 1];
    lo.netmask = [255, 0, 0, 0];
    lo.driver_idx = DRIVER_LOOPBACK;
    let _ = register_device(lo);

    // Mark loopback up without DHCP
    {
        let mut table = DEVICE_TABLE.lock();
        if let Some(slot) = table
            .iter_mut()
            .find(|s| s.used && s.dev.name_str() == "lo")
        {
            slot.dev.up = true;
        }
    }

    // Ethernet device — register even if driver is not yet initialised so the
    // slot exists for later bring_up() calls.
    let e1000_mac = driver_get_mac(DRIVER_E1000);
    let mut eth0 = NetDevice::zeroed();
    eth0.set_name("eth0");
    eth0.mac = e1000_mac;
    eth0.driver_idx = DRIVER_E1000;
    let _ = register_device(eth0);

    crate::serial_println!("  netdev: initialized (lo + eth0)");
}
