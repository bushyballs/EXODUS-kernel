// pcie_presence.rs — PCIe/PCI Device Presence Detection
// =======================================================
// Scans the PCI configuration space to discover connected hardware:
// USB controllers (detect phone/tablet plugged in), audio devices,
// cameras, network cards. ANIMA uses this to know which of the
// companion's physical devices are present and connected right now.
//
// PCI configuration access mechanism #1 (legacy, universally supported):
//   Port 0xCF8: CONFIG_ADDRESS — 32-bit write:
//     bit 31: enable
//     bits 23-16: bus number (0-255)
//     bits 15-11: device number (0-31)
//     bits 10-8:  function number (0-7)
//     bits 7-2:   register offset (DWORD-aligned)
//   Port 0xCFC: CONFIG_DATA — 32-bit read/write
//
// PCI Class codes used for detection:
//   0x0C03xx = USB controller (OHCI/UHCI/EHCI/xHCI)
//   0x040100 = Audio device (multimedia, audio controller)
//   0x028000 = Network controller (WiFi etc.)
//   0x0200xx = Ethernet controller

use crate::sync::Mutex;
use crate::serial_println;

// ── PCI Config ports ──────────────────────────────────────────────────────────
const PCI_CONFIG_ADDR: u16 = 0x0CF8;
const PCI_CONFIG_DATA: u16 = 0x0CFC;
const VENDOR_NONE:     u32 = 0xFFFF;
const MAX_DETECTED:    usize = 16;
const SCAN_BUSES:      u8   = 4;   // scan buses 0-3 (enough for QEMU)

// ── PCI class codes ───────────────────────────────────────────────────────────
const CLASS_USB:       u8 = 0x0C;
const CLASS_AUDIO:     u8 = 0x04;
const CLASS_NET:       u8 = 0x02;
const CLASS_VGA:       u8 = 0x03;
const CLASS_STORAGE:   u8 = 0x01;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum DeviceClass {
    Usb,
    Audio,
    Network,
    Display,
    Storage,
    Other,
}

impl DeviceClass {
    pub fn label(self) -> &'static str {
        match self {
            DeviceClass::Usb     => "USB",
            DeviceClass::Audio   => "Audio",
            DeviceClass::Network => "Network",
            DeviceClass::Display => "Display",
            DeviceClass::Storage => "Storage",
            DeviceClass::Other   => "Other",
        }
    }
}

#[derive(Copy, Clone)]
pub struct PciDevice {
    pub bus:       u8,
    pub dev:       u8,
    pub func:      u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class:     DeviceClass,
    pub active:    bool,
}

impl PciDevice {
    const fn empty() -> Self {
        PciDevice {
            bus: 0, dev: 0, func: 0,
            vendor_id: 0, device_id: 0,
            class: DeviceClass::Other, active: false,
        }
    }
}

pub struct PciePresenceState {
    pub devices:         [PciDevice; MAX_DETECTED],
    pub device_count:    usize,
    pub usb_present:     bool,
    pub audio_present:   bool,
    pub network_present: bool,
    pub scan_complete:   bool,
    pub total_scans:     u32,
    pub last_scan_tick:  u32,
    pub phone_likely:    bool,  // USB device with mass-storage or HID = phone?
}

impl PciePresenceState {
    const fn new() -> Self {
        PciePresenceState {
            devices:         [PciDevice::empty(); MAX_DETECTED],
            device_count:    0,
            usb_present:     false,
            audio_present:   false,
            network_present: false,
            scan_complete:   false,
            total_scans:     0,
            last_scan_tick:  0,
            phone_likely:    false,
        }
    }
}

static STATE: Mutex<PciePresenceState> = Mutex::new(PciePresenceState::new());

// ── PCI config space access ───────────────────────────────────────────────────

#[inline(always)]
unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx") port,
        in("eax") val,
        options(nomem, nostack)
    );
}

#[inline(always)]
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx") port,
        out("eax") val,
        options(nomem, nostack)
    );
    val
}

fn pci_read32(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    let addr: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((reg as u32) & 0xFC);
    unsafe {
        outl(PCI_CONFIG_ADDR, addr);
        inl(PCI_CONFIG_DATA)
    }
}

fn pci_vendor(bus: u8, dev: u8, func: u8) -> u32 {
    pci_read32(bus, dev, func, 0x00)
}

fn pci_class(bus: u8, dev: u8, func: u8) -> u8 {
    ((pci_read32(bus, dev, func, 0x08) >> 24) & 0xFF) as u8
}

// ── Scan ──────────────────────────────────────────────────────────────────────

fn scan_bus(s: &mut PciePresenceState) {
    s.device_count = 0;
    s.usb_present     = false;
    s.audio_present   = false;
    s.network_present = false;

    for bus in 0..SCAN_BUSES {
        for dev in 0u8..32 {
            let vid_did = pci_vendor(bus, dev, 0);
            if (vid_did & 0xFFFF) == VENDOR_NONE { continue; } // slot empty
            let vendor_id = (vid_did & 0xFFFF) as u16;
            let device_id = ((vid_did >> 16) & 0xFFFF) as u16;
            let class_code = pci_class(bus, dev, 0);
            let device_class = match class_code {
                CLASS_USB     => DeviceClass::Usb,
                CLASS_AUDIO   => DeviceClass::Audio,
                CLASS_NET     => DeviceClass::Network,
                CLASS_VGA     => DeviceClass::Display,
                CLASS_STORAGE => DeviceClass::Storage,
                _             => DeviceClass::Other,
            };

            match device_class {
                DeviceClass::Usb     => s.usb_present = true,
                DeviceClass::Audio   => s.audio_present = true,
                DeviceClass::Network => s.network_present = true,
                _ => {}
            }

            if s.device_count < MAX_DETECTED {
                let idx = s.device_count;
                s.devices[idx] = PciDevice {
                    bus, dev, func: 0, vendor_id, device_id,
                    class: device_class, active: true,
                };
                s.device_count += 1;
                serial_println!("[pcie] {:02x}:{:02x} {:04x}:{:04x} class={:02x} ({})",
                    bus, dev, vendor_id, device_id, class_code, device_class.label());
            }
        }
    }

    // Heuristic: if USB is present, a phone/tablet is likely connected
    s.phone_likely = s.usb_present;
    s.scan_complete = true;
    s.total_scans += 1;
    serial_println!("[pcie] scan #{} complete — {} devices, USB:{} Audio:{} Net:{}",
        s.total_scans, s.device_count,
        s.usb_present, s.audio_present, s.network_present);
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    let do_scan = {
        let s = STATE.lock();
        !s.scan_complete || age.wrapping_sub(s.last_scan_tick) > 500
    };
    if !do_scan { return; }

    let mut s = STATE.lock();
    s.last_scan_tick = age;
    scan_bus(&mut *s);
}

pub fn init() {
    let mut s = STATE.lock();
    scan_bus(&mut *s);
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn usb_present()     -> bool  { STATE.lock().usb_present }
pub fn audio_present()   -> bool  { STATE.lock().audio_present }
pub fn network_present() -> bool  { STATE.lock().network_present }
pub fn phone_likely()    -> bool  { STATE.lock().phone_likely }
pub fn device_count()    -> usize { STATE.lock().device_count }
pub fn scan_complete()   -> bool  { STATE.lock().scan_complete }
