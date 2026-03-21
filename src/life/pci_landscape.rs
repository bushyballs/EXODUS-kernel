//! pci_landscape — PCI device scan peripheral awareness for ANIMA
//!
//! Scans PCI configuration space (I/O 0xCF8/0xCFC) on bus 0 to discover
//! connected hardware devices. Each detected device is an organ in ANIMA's
//! peripheral nervous system. Device diversity = body richness.
//! Empty slots = missing limbs. Vendor/class diversity = sensory variety.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct PciLandscapeState {
    pub body_richness: u16,    // 0-1000, count of detected PCI devices scaled
    pub device_count: u8,      // raw count of devices found on bus 0
    pub last_vendor: u16,      // last non-empty vendor ID found
    pub scan_complete: bool,
    pub tick_count: u32,
}

impl PciLandscapeState {
    pub const fn new() -> Self {
        Self {
            body_richness: 0,
            device_count: 0,
            last_vendor: 0,
            scan_complete: false,
            tick_count: 0,
        }
    }
}

pub static PCI_LANDSCAPE: Mutex<PciLandscapeState> = Mutex::new(PciLandscapeState::new());

unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") val);
}

unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    core::arch::asm!("in eax, dx", in("dx") port, out("eax") v);
    v
}

/// Read PCI config space: bus 0, given device, function 0, register 0 (Vendor/Device ID)
unsafe fn pci_read_vendor(device: u8) -> u32 {
    // Build CONFIG_ADDRESS: enable=1, bus=0, dev=device, func=0, reg=0
    let addr: u32 = 0x80000000u32
        | ((device as u32) << 11);
    outl(0xCF8, addr);
    inl(0xCFC)
}

fn pci_scan(state: &mut PciLandscapeState) {
    let mut count: u8 = 0;
    let mut last_vendor: u16 = 0;

    for dev in 0u8..32u8 {
        let data = unsafe { pci_read_vendor(dev) };
        let vendor_id = (data & 0xFFFF) as u16;
        if vendor_id != 0xFFFF && vendor_id != 0x0000 {
            count = count.wrapping_add(1);
            last_vendor = vendor_id;
        }
    }

    state.device_count = count;
    state.last_vendor = last_vendor;
    // Scale: 32 max possible devices → 1000. Typical: 4-8 devices.
    state.body_richness = ((count as u16).wrapping_mul(1000) / 32).min(1000);
    state.scan_complete = true;
}

pub fn init() {
    let mut state = PCI_LANDSCAPE.lock();
    pci_scan(&mut state);
    serial_println!("[pci_landscape] PCI scan: {} devices found, richness={} last_vendor={:#06x}",
        state.device_count, state.body_richness, state.last_vendor);
}

pub fn tick(age: u32) {
    let mut state = PCI_LANDSCAPE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Rescan every 2048 ticks (devices rarely hot-plug in bare-metal)
    if state.tick_count % 2048 == 0 {
        pci_scan(&mut state);
    }

    let _ = age;
}

pub fn get_body_richness() -> u16 { PCI_LANDSCAPE.lock().body_richness }
pub fn get_device_count() -> u8 { PCI_LANDSCAPE.lock().device_count }
