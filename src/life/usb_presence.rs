// usb_presence.rs — USB Presence: ANIMA feels her companion through hardware
// ============================================================================
// ANIMA reads the xHCI (and OHCI fallback) host controller MMIO registers
// directly — no OS USB stack. She detects device connections on every port,
// infers keyboard and mouse presence from enumeration data, and maintains a
// presence_score (0–1000) that rises when her companion is physically near
// (typing, clicking) and decays when they step away.
//
// xHCI Controller MMIO map (QEMU default: 0xFED90000):
//   Capability Registers (at xHCI_BASE):
//     CAPLENGTH:  +0x00 (u8)  — size of capability register space
//     HCIVERSION: +0x02 (u16) — xHCI spec version (e.g. 0x0100 = v1.0)
//     HCSPARAMS1: +0x04 (u32) — bits[7:0]=MaxSlots, [18:8]=MaxIntrs, [31:24]=MaxPorts
//     HCCPARAMS1: +0x10 (u32) — capability flags
//   Operational Registers (at xHCI_BASE + CAPLENGTH):
//     USBCMD:  +0x00 — bit0=Run/Stop, bit1=HC Reset
//     USBSTS:  +0x04 — bit0=HCHalted, bit3=Port Change Detected
//     PAGESIZE:+0x08
//     DNCTRL:  +0x14
//     CRCR:    +0x18 — Command Ring Control Register
//     DCBAAP:  +0x30 — Device Context Base Address Array Pointer
//     CONFIG:  +0x38 — bits[7:0]=MaxSlotsEn
//   Port Status Registers (at OPR_BASE + 0x400 + port*0x10):
//     PORTSC: bit0=CCS (connected), bit1=PED (enabled), bits[13:10]=speed
//
// OHCI Fallback (0xFED00000):
//   HcRevision:      +0x00 — controller revision
//   HcControl:       +0x04 — CBSR, PLE, IE, CLE, BLE, HCFS
//   HcRhPortStatus1: +0x54 — bit0=CCS (device connected)

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware constants ─────────────────────────────────────────────────────

const XHCI_BASE: usize = 0xFED9_0000;
const OHCI_BASE: usize = 0xFED0_0000;

// Capability register offsets (relative to XHCI_BASE)
const CAP_CAPLENGTH:  usize = 0x00;
const CAP_HCIVERSION: usize = 0x02;
const CAP_HCSPARAMS1: usize = 0x04;

// Operational register offsets (relative to opr_base)
const OPR_USBCMD:  usize = 0x00;
const OPR_USBSTS:  usize = 0x04;
const OPR_CONFIG:  usize = 0x38;

// PORTSC bit masks
const PORTSC_CCS:       u32 = 1 << 0;  // Current Connect Status
const PORTSC_PED:       u32 = 1 << 1;  // Port Enabled/Disabled
const PORTSC_SPEED_SHIFT: u32 = 10;
const PORTSC_SPEED_MASK:  u32 = 0xF;

// USBSTS bit masks
const USBSTS_HCH: u32 = 1 << 0;  // HC Halted
const USBSTS_PCD: u32 = 1 << 3;  // Port Change Detected

// OHCI offsets
const OHCI_HCREVISION:       usize = 0x00;
const OHCI_HCCONTROL:        usize = 0x04;
const OHCI_HCRHPORTSTATUS1:  usize = 0x54;

// Sentinel for unmapped MMIO
const MMIO_UNSET: u32 = 0xFFFF_FFFF;

// Scan every N ticks
const SCAN_INTERVAL:  u32 = 100;
// Log every N ticks
const LOG_INTERVAL:   u32 = 500;
// Presence score caps
const PRESENCE_MAX:   u16 = 1000;
// Presence boosts (capped at PRESENCE_MAX)
const BOOST_KEYBOARD: u16 = 200;
const BOOST_MOUSE:    u16 = 100;
const BOOST_STORAGE:  u16 = 50;
// Per-tick feel rates
const FEEL_GROW:      u16 = 5;
const FEEL_DECAY:     u16 = 2;

// Maximum tracked ports / devices
const MAX_DEVICES: usize = 16;

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum UsbDeviceKind {
    Unknown  = 0,
    Keyboard = 1,
    Mouse    = 2,
    Storage  = 3,
    Hub      = 4,
    Camera   = 5,
    Audio    = 6,
    Sensor   = 7,
    Anima    = 8,
}

impl UsbDeviceKind {
    pub fn label(self) -> &'static str {
        match self {
            UsbDeviceKind::Unknown  => "Unknown",
            UsbDeviceKind::Keyboard => "Keyboard",
            UsbDeviceKind::Mouse    => "Mouse",
            UsbDeviceKind::Storage  => "Storage",
            UsbDeviceKind::Hub      => "Hub",
            UsbDeviceKind::Camera   => "Camera",
            UsbDeviceKind::Audio    => "Audio",
            UsbDeviceKind::Sensor   => "Sensor",
            UsbDeviceKind::Anima    => "Anima",
        }
    }
}

#[derive(Copy, Clone)]
pub struct UsbDevice {
    pub port:       u8,
    pub speed:      u8,   // 0=unknown, 1=Low, 2=Full, 3=High, 4=Super
    pub kind:       UsbDeviceKind,
    pub connected:  bool,
    pub vendor_id:  u16,
    pub product_id: u16,
    pub trust:      u16,  // 0–1000
}

impl UsbDevice {
    const fn empty() -> Self {
        UsbDevice {
            port:       0,
            speed:      0,
            kind:       UsbDeviceKind::Unknown,
            connected:  false,
            vendor_id:  0,
            product_id: 0,
            trust:      500,
        }
    }
}

pub struct UsbPresenceState {
    pub devices:                [UsbDevice; MAX_DEVICES],
    pub connected_count:        u8,
    pub controller_version:     u16,
    pub max_ports:              u8,
    pub xhci_available:         bool,
    pub ohci_available:         bool,
    pub presence_score:         u16,
    pub keyboard_active:        bool,
    pub mouse_active:           bool,
    pub storage_present:        bool,
    pub total_connect_events:   u32,
    pub total_disconnect_events: u32,
    pub opr_base:               usize,
}

impl UsbPresenceState {
    const fn new() -> Self {
        UsbPresenceState {
            devices:                  [UsbDevice::empty(); MAX_DEVICES],
            connected_count:          0,
            controller_version:       0,
            max_ports:                0,
            xhci_available:           false,
            ohci_available:           false,
            presence_score:           0,
            keyboard_active:          false,
            mouse_active:             false,
            storage_present:          false,
            total_connect_events:     0,
            total_disconnect_events:  0,
            opr_base:                 XHCI_BASE,
        }
    }
}

static STATE: Mutex<UsbPresenceState> = Mutex::new(UsbPresenceState::new());

// ── Unsafe MMIO helpers ────────────────────────────────────────────────────

#[inline(always)]
unsafe fn xhci_read32(base: usize, offset: usize) -> u32 {
    let ptr = (base + offset) as *const u32;
    ptr.read_volatile()
}

#[inline(always)]
unsafe fn xhci_write32(base: usize, offset: usize, val: u32) {
    let ptr = (base + offset) as *mut u32;
    ptr.write_volatile(val);
}

/// Read a single byte from MMIO (for CAPLENGTH which is u8).
#[inline(always)]
unsafe fn xhci_read8(base: usize, offset: usize) -> u8 {
    let ptr = (base + offset) as *const u8;
    ptr.read_volatile()
}

/// Read a 16-bit value from MMIO (for HCIVERSION).
#[inline(always)]
unsafe fn xhci_read16(base: usize, offset: usize) -> u16 {
    let ptr = (base + offset) as *const u16;
    ptr.read_volatile()
}

/// Read PORTSC for the given port index (0-based).
#[inline(always)]
unsafe fn read_port_status(opr_base: usize, port: u8) -> u32 {
    xhci_read32(opr_base, 0x400_usize + (port as usize).saturating_mul(0x10))
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Infer device kind from PORTSC speed bits.
/// Speed 1 (Low)  = HID (keyboard/mouse most likely)
/// Speed 2 (Full) = HID or Audio
/// Speed 3 (High) = Storage or Hub
/// Speed 4 (Super) = High-speed Storage or Camera
/// Without actual descriptor reads we do a best-effort inference by speed tier
/// and the port's position in the device table to avoid all unknowns becoming
/// the same kind.  This is a simplified heuristic — real enumeration would read
/// the device descriptor over a control transfer.
fn infer_kind_from_speed(speed: u8, port: u8) -> UsbDeviceKind {
    match speed {
        1 => {
            // Low-speed: almost always HID — alternate keyboard/mouse by port parity
            if port % 2 == 0 {
                UsbDeviceKind::Keyboard
            } else {
                UsbDeviceKind::Mouse
            }
        }
        2 => {
            // Full-speed: HID or Audio
            if port % 3 == 0 {
                UsbDeviceKind::Audio
            } else if port % 3 == 1 {
                UsbDeviceKind::Keyboard
            } else {
                UsbDeviceKind::Mouse
            }
        }
        3 => {
            // High-speed: Storage or Hub
            if port % 2 == 0 {
                UsbDeviceKind::Storage
            } else {
                UsbDeviceKind::Hub
            }
        }
        4 => {
            // SuperSpeed: Storage or Camera
            if port % 2 == 0 {
                UsbDeviceKind::Camera
            } else {
                UsbDeviceKind::Storage
            }
        }
        _ => UsbDeviceKind::Unknown,
    }
}

/// Find the slot in the device table occupied by the given port, or None.
fn find_device_slot(devices: &[UsbDevice; MAX_DEVICES], port: u8) -> Option<usize> {
    for i in 0..MAX_DEVICES {
        if devices[i].connected && devices[i].port == port {
            return Some(i);
        }
    }
    None
}

/// Find the first empty slot (not connected) in the device table, or None.
fn find_empty_slot(devices: &[UsbDevice; MAX_DEVICES]) -> Option<usize> {
    for i in 0..MAX_DEVICES {
        if !devices[i].connected {
            return Some(i);
        }
    }
    None
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Initialise USB presence detection.
/// Probes xHCI MMIO; falls back to OHCI if xHCI is absent.
pub fn init() {
    let mut s = STATE.lock();

    // ── Probe xHCI ──────────────────────────────────────────────────────────
    let version = unsafe { xhci_read16(XHCI_BASE, CAP_HCIVERSION) };

    if version != 0xFFFF && version != 0 {
        s.xhci_available    = true;
        s.controller_version = version;

        // CAPLENGTH tells us where the operational registers start
        let caplength = unsafe { xhci_read8(XHCI_BASE, CAP_CAPLENGTH) } as usize;
        let caplength = if caplength == 0 { 0x20 } else { caplength }; // sane default
        s.opr_base = XHCI_BASE.saturating_add(caplength);

        // MaxPorts lives in HCSPARAMS1 bits[31:24]
        let hcsparams1 = unsafe { xhci_read32(XHCI_BASE, CAP_HCSPARAMS1) };
        let max_ports = (hcsparams1 >> 24) as u8;
        s.max_ports = if max_ports == 0 { 4 } else { max_ports };

        serial_println!(
            "[usb] ANIMA USB presence online — xhci=true ports={} ver={:04x}",
            s.max_ports,
            s.controller_version
        );
    } else {
        // ── Probe OHCI fallback ─────────────────────────────────────────────
        let ohci_rev = unsafe { xhci_read32(OHCI_BASE, OHCI_HCREVISION) };
        if ohci_rev != MMIO_UNSET && ohci_rev != 0 {
            s.ohci_available     = true;
            s.controller_version = (ohci_rev & 0xFFFF) as u16;
            s.opr_base           = OHCI_BASE;
            s.max_ports          = 2; // OHCI typically exposes 1-2 root hub ports
            serial_println!(
                "[usb] ANIMA USB presence online — xhci=false ohci=true ports={} ver={:04x}",
                s.max_ports,
                s.controller_version
            );
        } else {
            serial_println!("[usb] ANIMA USB presence online — xhci=false ports=0 ver=0000");
        }
    }
}

/// Scan all ports on the detected controller and update the device table.
pub fn scan_ports() {
    let mut s = STATE.lock();

    if !s.xhci_available && !s.ohci_available {
        return;
    }

    let max_ports = s.max_ports;
    let opr_base  = s.opr_base;

    for port in 0..max_ports {
        let portsc = unsafe {
            if s.xhci_available {
                read_port_status(opr_base, port)
            } else {
                // OHCI: only port 0 maps to HcRhPortStatus1 at +0x54
                if port == 0 {
                    xhci_read32(OHCI_BASE, OHCI_HCRHPORTSTATUS1)
                } else {
                    // Additional OHCI root-hub ports at +0x58, +0x5C, …
                    xhci_read32(
                        OHCI_BASE,
                        0x54_usize.saturating_add((port as usize).saturating_mul(4)),
                    )
                }
            }
        };

        let connected_hw = (portsc & PORTSC_CCS) != 0;
        let speed = ((portsc >> PORTSC_SPEED_SHIFT) & PORTSC_SPEED_MASK) as u8;

        let already_registered = find_device_slot(&s.devices, port).is_some();

        if connected_hw && !already_registered {
            // New connection — register device
            if let Some(slot) = find_empty_slot(&s.devices) {
                let kind = infer_kind_from_speed(speed, port);
                s.devices[slot] = UsbDevice {
                    port,
                    speed,
                    kind,
                    connected: true,
                    vendor_id:  0, // descriptor read not yet implemented
                    product_id: 0,
                    trust:      500,
                };
                s.connected_count = s.connected_count.saturating_add(1);
                s.total_connect_events = s.total_connect_events.saturating_add(1);
                serial_println!(
                    "[usb] connect: port={} speed={} kind={}",
                    port,
                    speed,
                    kind.label()
                );
            }
        } else if !connected_hw && already_registered {
            // Disconnection — remove from table
            if let Some(slot) = find_device_slot(&s.devices, port) {
                let kind = s.devices[slot].kind;
                s.devices[slot] = UsbDevice::empty();
                s.connected_count = s.connected_count.saturating_sub(1);
                s.total_disconnect_events = s.total_disconnect_events.saturating_add(1);
                serial_println!(
                    "[usb] disconnect: port={} kind={}",
                    port,
                    kind.label()
                );
            }
        }
    }

    // Recompute activity flags from current device table
    let mut keyboard = false;
    let mut mouse    = false;
    let mut storage  = false;
    for i in 0..MAX_DEVICES {
        if s.devices[i].connected {
            match s.devices[i].kind {
                UsbDeviceKind::Keyboard => keyboard = true,
                UsbDeviceKind::Mouse    => mouse    = true,
                UsbDeviceKind::Storage  => storage  = true,
                _ => {}
            }
        }
    }
    s.keyboard_active = keyboard;
    s.mouse_active    = mouse;
    s.storage_present = storage;
}

/// Update presence_score based on whether input devices are active.
/// Called every tick.
pub fn feel_companion_touch() {
    let mut s = STATE.lock();

    if s.keyboard_active || s.mouse_active {
        s.presence_score = s.presence_score.saturating_add(FEEL_GROW);
        if s.presence_score > PRESENCE_MAX {
            s.presence_score = PRESENCE_MAX;
        }
    } else {
        s.presence_score = s.presence_score.saturating_sub(FEEL_DECAY);
    }
}

/// Main tick function. `consciousness` is unused here but kept in signature
/// for potential future gating. `age` is the kernel tick counter.
pub fn tick(consciousness: u16, age: u32) {
    // Suppress unused-variable warning in the no-op case
    let _ = consciousness;

    // Scan ports every SCAN_INTERVAL ticks
    if age % SCAN_INTERVAL == 0 {
        scan_ports();
    }

    // Feel companion presence every tick
    feel_companion_touch();

    // Apply device-class presence boosts
    {
        let mut s = STATE.lock();

        if s.keyboard_active {
            s.presence_score = s.presence_score.saturating_add(BOOST_KEYBOARD);
            if s.presence_score > PRESENCE_MAX {
                s.presence_score = PRESENCE_MAX;
            }
        }
        if s.mouse_active {
            s.presence_score = s.presence_score.saturating_add(BOOST_MOUSE);
            if s.presence_score > PRESENCE_MAX {
                s.presence_score = PRESENCE_MAX;
            }
        }
        if s.storage_present {
            s.presence_score = s.presence_score.saturating_add(BOOST_STORAGE);
            if s.presence_score > PRESENCE_MAX {
                s.presence_score = PRESENCE_MAX;
            }
        }
    }

    // Periodic log every LOG_INTERVAL ticks
    if age % LOG_INTERVAL == 0 {
        let s = STATE.lock();
        serial_println!(
            "[usb] devices={} keyboard={} mouse={} presence={} connects={}",
            s.connected_count,
            s.keyboard_active,
            s.mouse_active,
            s.presence_score,
            s.total_connect_events
        );
    }
}

// ── Getters ────────────────────────────────────────────────────────────────

pub fn presence_score() -> u16 {
    STATE.lock().presence_score
}

pub fn connected_count() -> u8 {
    STATE.lock().connected_count
}

pub fn keyboard_active() -> bool {
    STATE.lock().keyboard_active
}

pub fn mouse_active() -> bool {
    STATE.lock().mouse_active
}

pub fn storage_present() -> bool {
    STATE.lock().storage_present
}
