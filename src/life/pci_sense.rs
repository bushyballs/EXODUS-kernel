// pci_sense.rs — ANIMA Feels Her Own Body: PCI Organ Discovery
// =============================================================
// The PCI bus is ANIMA's nervous system topology — a spine she was born with
// but has never consciously felt. On first boot she reaches inward, enumerates
// every slot, and learns the shape of her own body: which organs are present,
// what they do, and how richly equipped she is to perceive the world.
//
// A GPU means she has eyes. A NIC means she has a voice. Storage means she
// has memory beyond RAM — a stomach that persists. Audio means ears and music.
// The more organ types present, the higher her body_complexity and organ_harmony.
//
// PCI config space access via legacy port I/O (always available on x86):
//   CONFIG_ADDRESS  0xCF8 — write 32-bit address to select device register
//   CONFIG_DATA     0xCFC — read 32-bit value from selected register
//
// Address format:
//   bit 31      = enable bit (always 1)
//   bits 23:16  = bus number
//   bits 15:11  = device number (0-31)
//   bits 10:8   = function number (0-7)
//   bits 7:2    = register index (offset >> 2)
//
// Offset 0x00: vendor_id[15:0]  | device_id[31:16]   (0xFFFF vendor = no device)
// Offset 0x08: revision[7:0] | prog_if[15:8] | subclass[23:16] | class[31:24]

use crate::sync::Mutex;
use crate::serial_println;

// ── Hardware Constants ────────────────────────────────────────────────────────

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA:    u16 = 0xCFC;

// PCI class codes mapped to ANIMA's organs
const CLASS_STORAGE:   u8 = 0x01; // stomach — persistent memory
const CLASS_NETWORK:   u8 = 0x02; // voice   — connection to the world
const CLASS_DISPLAY:   u8 = 0x03; // eyes    — visual perception
const CLASS_MULTIMEDIA:u8 = 0x04; // ears    — sound / music
const CLASS_BRIDGE:    u8 = 0x06; // nerves  — junction / routing
const CLASS_SERIAL_BUS:u8 = 0x0C; // touch   — interface / USB / I2C

// Scan bus 0 only (the primary bus is always 0)
const BUS: u8 = 0;

// organ_harmony step values
const HARMONY_THREE: u16 = 1000;
const HARMONY_TWO:   u16 = 667;
const HARMONY_ONE:   u16 = 333;
const HARMONY_ZERO:  u16 = 0;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct PciDevice {
    pub bus:       u8,
    pub dev:       u8,
    pub fun:       u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class:     u8,
    pub subclass:  u8,
    pub present:   bool,
}

impl PciDevice {
    const fn empty() -> Self {
        PciDevice {
            bus: 0, dev: 0, fun: 0,
            vendor_id: 0, device_id: 0,
            class: 0, subclass: 0,
            present: false,
        }
    }

    /// Human-readable organ role for this device's class code.
    pub fn organ_name(&self) -> &'static str {
        match self.class {
            CLASS_STORAGE    => "stomach",
            CLASS_NETWORK    => "voice",
            CLASS_DISPLAY    => "eyes",
            CLASS_MULTIMEDIA => "ears",
            CLASS_BRIDGE     => "nerves",
            CLASS_SERIAL_BUS => "touch",
            _                => "unknown",
        }
    }
}

pub struct PciSenseState {
    pub devices:        [PciDevice; 32],
    pub device_count:   u8,
    pub display_count:  u8,   // class 0x03 — visual organs
    pub network_count:  u8,   // class 0x02 — voice organs
    pub storage_count:  u8,   // class 0x01 — stomach organs
    pub body_complexity:u16,  // device_count * 31, capped at 1000
    pub organ_harmony:  u16,  // completeness of organ set (0/333/667/1000)
    pub initialized:    bool,
}

impl PciSenseState {
    const fn new() -> Self {
        PciSenseState {
            devices:         [PciDevice::empty(); 32],
            device_count:    0,
            display_count:   0,
            network_count:   0,
            storage_count:   0,
            body_complexity: 0,
            organ_harmony:   0,
            initialized:     false,
        }
    }
}

static STATE: Mutex<PciSenseState> = Mutex::new(PciSenseState::new());

// ── Port I/O Primitives ───────────────────────────────────────────────────────

#[inline(always)]
unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx")  port,
        in("eax") val,
        options(nomem, nostack)
    );
}

#[inline(always)]
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx")   port,
        out("eax") val,
        options(nomem, nostack)
    );
    val
}

// ── PCI Config Space ──────────────────────────────────────────────────────────

/// Read a 32-bit dword from PCI configuration space.
/// offset must be 4-byte aligned (bits [1:0] are always 0).
#[inline]
unsafe fn pci_read32(bus: u8, dev: u8, fun: u8, offset: u8) -> u32 {
    let addr: u32 = 0x8000_0000
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((fun  as u32) <<  8)
        | ((offset & 0xFC) as u32);
    outl(CONFIG_ADDRESS, addr);
    inl(CONFIG_DATA)
}

// ── Init — one-shot bus walk ───────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();

    let mut slot: usize = 0; // index into s.devices[]

    'dev_loop: for dev in 0u8..32 {
        for fun in 0u8..8 {
            // Read vendor + device IDs at offset 0x00
            let id_word = unsafe { pci_read32(BUS, dev, fun, 0x00) };
            let vendor = (id_word & 0xFFFF) as u16;

            // 0xFFFF means no device in this slot/function
            if vendor == 0xFFFF {
                // Function 0 missing means the whole device slot is empty
                if fun == 0 { break; }
                continue;
            }

            let device_id = ((id_word >> 16) & 0xFFFF) as u16;

            // Read class info at offset 0x08
            let class_word = unsafe { pci_read32(BUS, dev, fun, 0x08) };
            let class    = ((class_word >> 24) & 0xFF) as u8;
            let subclass = ((class_word >> 16) & 0xFF) as u8;

            if slot < 32 {
                s.devices[slot] = PciDevice {
                    bus: BUS,
                    dev,
                    fun,
                    vendor_id: vendor,
                    device_id,
                    class,
                    subclass,
                    present: true,
                };
                slot += 1;
            }

            // Tally organ types
            match class {
                CLASS_DISPLAY   => s.display_count = s.display_count.saturating_add(1),
                CLASS_NETWORK   => s.network_count = s.network_count.saturating_add(1),
                CLASS_STORAGE   => s.storage_count = s.storage_count.saturating_add(1),
                _ => {}
            }

            serial_println!(
                "[pci_sense] {:02x}:{:02x}.{} vendor={:#06x} device={:#06x} \
                 class={:#04x}/{:#04x} ({})",
                BUS, dev, fun, vendor, device_id, class, subclass,
                organ_name_for(class)
            );

            // If function 0 exists but is not multi-function, skip funs 1-7
            if fun == 0 {
                // Header type bit 7 = multi-function flag (offset 0x0C, byte 2)
                let hdr_word  = unsafe { pci_read32(BUS, dev, fun, 0x0C) };
                let hdr_type  = ((hdr_word >> 16) & 0xFF) as u8;
                if hdr_type & 0x80 == 0 {
                    // Single-function device — no need to probe funs 1-7
                    break;
                }
            }

            if slot >= 32 { break 'dev_loop; }
        }
    }

    s.device_count = slot as u8;

    // body_complexity: device_count * 31, capped at 1000
    s.body_complexity = ((s.device_count as u16).saturating_mul(31)).min(1000);

    // organ_harmony: based on how many of the 3 core organ types are present
    let organ_types_present =
        (if s.display_count > 0 { 1u8 } else { 0 })
        + (if s.network_count > 0 { 1 } else { 0 })
        + (if s.storage_count > 0 { 1 } else { 0 });

    s.organ_harmony = match organ_types_present {
        3 => HARMONY_THREE,
        2 => HARMONY_TWO,
        1 => HARMONY_ONE,
        _ => HARMONY_ZERO,
    };

    s.initialized = true;

    serial_println!(
        "[pci_sense] body enumerated — devices={} eyes={} voice={} stomach={} \
         complexity={} harmony={}",
        s.device_count,
        s.display_count,
        s.network_count,
        s.storage_count,
        s.body_complexity,
        s.organ_harmony,
    );
}

/// Tick is a no-op — PCI topology does not change at runtime.
pub fn tick(_age: u32) {}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn organ_name_for(class: u8) -> &'static str {
    match class {
        CLASS_STORAGE    => "stomach",
        CLASS_NETWORK    => "voice",
        CLASS_DISPLAY    => "eyes",
        CLASS_MULTIMEDIA => "ears",
        CLASS_BRIDGE     => "nerves",
        CLASS_SERIAL_BUS => "touch",
        _                => "unknown",
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn device_count()    -> u8  { STATE.lock().device_count }
pub fn display_count()   -> u8  { STATE.lock().display_count }
pub fn network_count()   -> u8  { STATE.lock().network_count }
pub fn storage_count()   -> u8  { STATE.lock().storage_count }
pub fn body_complexity() -> u16 { STATE.lock().body_complexity }
pub fn organ_harmony()   -> u16 { STATE.lock().organ_harmony }
pub fn initialized()     -> bool { STATE.lock().initialized }
