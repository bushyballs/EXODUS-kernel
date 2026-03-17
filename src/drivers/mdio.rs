use crate::sync::Mutex;
/// MDIO/MII PHY management bus driver for Genesis — no-heap, fixed-size static arrays
///
/// Implements IEEE 802.3 MDIO (Management Data Input/Output) bus registration,
/// PHY discovery, register read/write, and link-status helpers.
///
/// The register file is fully simulated in software; a real driver would
/// replace `mdio_read` / `mdio_write` with hardware MDIO bit-bang or
/// controller MMIO register accesses.
///
/// All critical rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - Structs in static Mutex are Copy with const fn empty()
///   - No division without divisor != 0 guard
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Capacities
// ---------------------------------------------------------------------------

pub const MDIO_MAX_BUSES: usize = 4;
pub const MDIO_MAX_PHYS: usize = 32; // max PHY addresses per bus (0-31)

// ---------------------------------------------------------------------------
// Standard MII register addresses
// ---------------------------------------------------------------------------

pub const MII_BMCR: u16 = 0x00; // Basic Mode Control
pub const MII_BMSR: u16 = 0x01; // Basic Mode Status
pub const MII_PHYSID1: u16 = 0x02;
pub const MII_PHYSID2: u16 = 0x03;
pub const MII_ADVERTISE: u16 = 0x04;
pub const MII_LPA: u16 = 0x05; // Link Partner Ability

// PHY register file size (MII allows up to 32 registers; we mirror that)
const PHY_REGS: usize = 32;

// ---------------------------------------------------------------------------
// BMCR bits
// ---------------------------------------------------------------------------

pub const BMCR_SPEED100: u16 = 0x2000;
pub const BMCR_ANENABLE: u16 = 0x1000;
pub const BMCR_RESET: u16 = 0x8000;
pub const BMCR_FULLDPLX: u16 = 0x0100;

// ---------------------------------------------------------------------------
// BMSR bits
// ---------------------------------------------------------------------------

pub const BMSR_LSTATUS: u16 = 0x0004; // Link up
pub const BMSR_ANEGCOMPLETE: u16 = 0x0020;
pub const BMSR_100FULL: u16 = 0x4000;

// ---------------------------------------------------------------------------
// Bus descriptor
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct MdioBus {
    pub id: u32,
    pub name: [u8; 16],
    pub name_len: u8,
    pub active: bool,
}

impl MdioBus {
    pub const fn empty() -> Self {
        MdioBus {
            id: 0,
            name: [0u8; 16],
            name_len: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// PHY device descriptor
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct MdioPhyDevice {
    pub bus_id: u32,
    pub phy_addr: u8,          // 0-31
    pub phy_id: u32,           // PHYSID1 << 16 | PHYSID2
    pub regs: [u16; PHY_REGS], // simulated register file
    pub link_up: bool,
    pub speed: u32,   // 10, 100, or 1000 Mbps
    pub duplex: bool, // true = full duplex
    pub active: bool,
}

impl MdioPhyDevice {
    pub const fn empty() -> Self {
        MdioPhyDevice {
            bus_id: 0,
            phy_addr: 0,
            phy_id: 0,
            regs: [0u16; PHY_REGS],
            link_up: false,
            speed: 0,
            duplex: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MDIO_BUSES: Mutex<[MdioBus; MDIO_MAX_BUSES]> = Mutex::new([
    MdioBus::empty(),
    MdioBus::empty(),
    MdioBus::empty(),
    MdioBus::empty(),
]);

// MDIO_MAX_PHYS = 32 — matches the IEEE 802.3 5-bit PHY address space
static MDIO_PHYS: Mutex<[MdioPhyDevice; MDIO_MAX_PHYS]> = Mutex::new([
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
    MdioPhyDevice::empty(),
]);

// ---------------------------------------------------------------------------
// Bus helpers
// ---------------------------------------------------------------------------

/// Find the index of the bus slot with `bus_id`, or `None` if not found.
fn find_bus_idx(bus_id: u32) -> Option<usize> {
    let buses = MDIO_BUSES.lock();
    for i in 0..MDIO_MAX_BUSES {
        if buses[i].active && buses[i].id == bus_id {
            return Some(i);
        }
    }
    None
}

/// Find the index of the first free (inactive) bus slot, or `None` if full.
fn alloc_bus_idx() -> Option<usize> {
    let buses = MDIO_BUSES.lock();
    for i in 0..MDIO_MAX_BUSES {
        if !buses[i].active {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// PHY helpers
// ---------------------------------------------------------------------------

/// Find the PHY slot for (`bus_id`, `phy_addr`), or `None`.
fn find_phy_idx(bus_id: u32, phy_addr: u8) -> Option<usize> {
    let phys = MDIO_PHYS.lock();
    for i in 0..MDIO_MAX_PHYS {
        if phys[i].active && phys[i].bus_id == bus_id && phys[i].phy_addr == phy_addr {
            return Some(i);
        }
    }
    None
}

/// Allocate a free PHY slot.
fn alloc_phy_idx() -> Option<usize> {
    let phys = MDIO_PHYS.lock();
    for i in 0..MDIO_MAX_PHYS {
        if !phys[i].active {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API — bus management
// ---------------------------------------------------------------------------

/// Register a new MDIO bus with the given name (up to 15 bytes, null-padded).
/// Returns the assigned bus id on success, or `None` if the bus table is full.
pub fn mdio_bus_register(name: &[u8]) -> Option<u32> {
    let slot = alloc_bus_idx()?;

    // Assign a unique id = slot index + 1  (0 reserved as "invalid")
    let id = (slot as u32).saturating_add(1);

    let mut buses = MDIO_BUSES.lock();
    buses[slot].id = id;
    buses[slot].name_len = 0;
    buses[slot].name = [0u8; 16];
    buses[slot].active = true;

    // Copy up to 15 bytes of the name
    let copy_len = if name.len() < 15 { name.len() } else { 15 };
    for i in 0..copy_len {
        buses[slot].name[i] = name[i];
    }
    buses[slot].name_len = copy_len as u8;

    Some(id)
}

/// Unregister an MDIO bus and deactivate all PHYs attached to it.
/// Returns `true` if the bus was found and removed.
pub fn mdio_bus_unregister(bus_id: u32) -> bool {
    // Deactivate all PHYs on this bus first
    {
        let mut phys = MDIO_PHYS.lock();
        for i in 0..MDIO_MAX_PHYS {
            if phys[i].active && phys[i].bus_id == bus_id {
                phys[i] = MdioPhyDevice::empty();
            }
        }
    }

    let mut buses = MDIO_BUSES.lock();
    for i in 0..MDIO_MAX_BUSES {
        if buses[i].active && buses[i].id == bus_id {
            buses[i] = MdioBus::empty();
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Public API — register access
// ---------------------------------------------------------------------------

/// Read a PHY register.
/// In a real system this would issue an MDIO read cycle over the wire.
/// Here it reads from the simulated per-PHY register file.
/// Returns `None` if the bus/phy combination is not registered or the
/// register index is out of range.
pub fn mdio_read(bus_id: u32, phy_addr: u8, reg: u16) -> Option<u16> {
    // Validate bus exists
    find_bus_idx(bus_id)?;
    if reg as usize >= PHY_REGS {
        return None;
    }
    let idx = find_phy_idx(bus_id, phy_addr)?;
    let phys = MDIO_PHYS.lock();
    Some(phys[idx].regs[reg as usize])
}

/// Write a PHY register.
/// Returns `false` if the bus/phy is not registered or the register index
/// is out of range.
pub fn mdio_write(bus_id: u32, phy_addr: u8, reg: u16, val: u16) -> bool {
    if find_bus_idx(bus_id).is_none() {
        return false;
    }
    if reg as usize >= PHY_REGS {
        return false;
    }
    let idx = match find_phy_idx(bus_id, phy_addr) {
        Some(i) => i,
        None => return false,
    };
    let mut phys = MDIO_PHYS.lock();
    phys[idx].regs[reg as usize] = val;

    // Mirror BMSR link bit to the link_up field for convenience
    if reg == MII_BMSR {
        phys[idx].link_up = val & BMSR_LSTATUS != 0;
    }
    true
}

// ---------------------------------------------------------------------------
// Public API — PHY discovery and management
// ---------------------------------------------------------------------------

/// Scan all 32 PHY addresses on `bus_id`.  For each address, read PHYSID1
/// and PHYSID2.  If the combined value is non-zero and not 0xFFFF_FFFF
/// (bus floating), register a new PHY device.
/// Returns the count of PHYs discovered.
pub fn mdio_phy_discover(bus_id: u32) -> u32 {
    if find_bus_idx(bus_id).is_none() {
        return 0;
    }
    let mut count: u32 = 0;

    for addr in 0u8..32 {
        // Read PHYSID registers from the simulated register file if the PHY
        // slot already exists, otherwise treat as 0 (not yet registered).
        let id1 = {
            match find_phy_idx(bus_id, addr) {
                Some(i) => {
                    let phys = MDIO_PHYS.lock();
                    phys[i].regs[MII_PHYSID1 as usize]
                }
                None => 0u16,
            }
        };
        let id2 = {
            match find_phy_idx(bus_id, addr) {
                Some(i) => {
                    let phys = MDIO_PHYS.lock();
                    phys[i].regs[MII_PHYSID2 as usize]
                }
                None => 0u16,
            }
        };

        let phy_id = ((id1 as u32) << 16) | (id2 as u32);

        // Skip absent (0x0000_0000) and floating-bus (0xFFFF_FFFF) values
        if phy_id == 0 || phy_id == 0xFFFF_FFFF {
            continue;
        }
        // Skip if already registered
        if find_phy_idx(bus_id, addr).is_some() {
            count = count.saturating_add(1);
            continue;
        }
        // Allocate a new slot
        let slot = match alloc_phy_idx() {
            Some(s) => s,
            None => break, // PHY table full
        };
        {
            let mut phys = MDIO_PHYS.lock();
            phys[slot].bus_id = bus_id;
            phys[slot].phy_addr = addr;
            phys[slot].phy_id = phy_id;
            phys[slot].active = true;
        }
        count = count.saturating_add(1);
    }
    count
}

/// Reset the PHY at `phy_addr` on `bus_id` by writing the BMCR_RESET bit.
/// In hardware the reset bit self-clears; here we clear it immediately.
/// Returns `false` if the PHY is not found.
pub fn mdio_phy_reset(bus_id: u32, phy_addr: u8) -> bool {
    // Write RESET bit
    if !mdio_write(bus_id, phy_addr, MII_BMCR, BMCR_RESET) {
        return false;
    }
    // In a real driver we would poll until the bit clears.
    // Stub: clear it immediately.
    mdio_write(bus_id, phy_addr, MII_BMCR, 0x0000)
}

/// Read BMSR and check the LSTATUS (link-up) bit.
/// Returns `false` if the PHY is not found.
pub fn mdio_phy_get_link(bus_id: u32, phy_addr: u8) -> bool {
    match mdio_read(bus_id, phy_addr, MII_BMSR) {
        Some(bmsr) => bmsr & BMSR_LSTATUS != 0,
        None => false,
    }
}

/// Enable auto-negotiation and advertise 100 Mbps full-duplex capability
/// by writing BMCR with ANENABLE | SPEED100 | FULLDPLX.
/// Returns `false` if the PHY is not found.
pub fn mdio_phy_setup_aneg(bus_id: u32, phy_addr: u8) -> bool {
    let bmcr = BMCR_ANENABLE | BMCR_SPEED100 | BMCR_FULLDPLX;
    mdio_write(bus_id, phy_addr, MII_BMCR, bmcr)
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the MDIO bus driver.
///
/// - Registers one simulated bus named "mdio0".
/// - Simulates a single RTL8211 PHY at address 1 (PHY_ID = 0x0022_1620).
/// - Pre-fills BMSR with LSTATUS | ANEGCOMPLETE | 100FULL.
/// - Sets link_up=true, speed=1000, duplex=true.
pub fn init() {
    // Register the primary simulated MDIO bus
    let bus_id = match mdio_bus_register(b"mdio0") {
        Some(id) => id,
        None => {
            serial_println!("[mdio] ERROR: failed to register mdio0 bus");
            return;
        }
    };

    // Allocate a PHY slot for the simulated RTL8211 at address 1
    let phy_slot = match alloc_phy_idx() {
        Some(s) => s,
        None => {
            serial_println!("[mdio] ERROR: PHY table full during init");
            return;
        }
    };

    {
        let mut phys = MDIO_PHYS.lock();
        phys[phy_slot].bus_id = bus_id;
        phys[phy_slot].phy_addr = 1;
        phys[phy_slot].phy_id = 0x0022_1620; // RTL8211
        phys[phy_slot].link_up = true;
        phys[phy_slot].speed = 1000;
        phys[phy_slot].duplex = true;
        phys[phy_slot].active = true;

        // Populate PHYSID registers
        phys[phy_slot].regs[MII_PHYSID1 as usize] = 0x0022;
        phys[phy_slot].regs[MII_PHYSID2 as usize] = 0x1620;

        // BMSR: link up, auto-neg complete, 100BASE-TX full-duplex capable
        phys[phy_slot].regs[MII_BMSR as usize] = BMSR_LSTATUS | BMSR_ANEGCOMPLETE | BMSR_100FULL;

        // BMCR: AN enabled, 100 Mbps, full duplex (reflects negotiated state)
        phys[phy_slot].regs[MII_BMCR as usize] = BMCR_ANENABLE | BMCR_SPEED100 | BMCR_FULLDPLX;
    }

    serial_println!("[mdio] MDIO bus driver initialized");
}
