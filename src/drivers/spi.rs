use crate::sync::Mutex;
/// SPI bus master framework for Genesis — no-heap, fixed-size static arrays
///
/// Implements a simulated SPI master with controller and device registries.
/// All transfers use loopback simulation (rx_buf[i] = tx_buf[i].wrapping_add(1))
/// since no real SPI hardware is present in the QEMU target.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - Structs in static Mutex are Copy with const fn empty()
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of SPI controllers supported
pub const MAX_SPI_CONTROLLERS: usize = 4;

/// Maximum number of SPI devices supported
pub const MAX_SPI_DEVICES: usize = 16;

/// SPI mode bit: clock phase (CPHA)
pub const SPI_CPHA: u8 = 1;

/// SPI mode bit: clock polarity (CPOL)
pub const SPI_CPOL: u8 = 2;

/// SPI mode bit: chip select active high
pub const SPI_CS_HIGH: u8 = 4;

/// SPI mode bit: LSB first bit order
pub const SPI_LSB_FIRST: u8 = 8;

/// Simulated SPI controller base I/O port
pub const SPI_IO_BASE: u16 = 0x3A0;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A registered SPI controller (bus master)
#[derive(Copy, Clone)]
pub struct SpiController {
    /// Unique controller identifier (assigned on registration)
    pub id: u32,
    /// Base I/O port for this controller
    pub io_base: u16,
    /// Maximum clock speed in Hz
    pub max_speed_hz: u32,
    /// Whether this controller slot is occupied
    pub active: bool,
}

impl SpiController {
    pub const fn empty() -> Self {
        SpiController {
            id: 0,
            io_base: 0,
            max_speed_hz: 0,
            active: false,
        }
    }
}

/// A registered SPI device (peripheral on a bus)
#[derive(Copy, Clone)]
pub struct SpiDevice {
    /// ID of the controller this device is attached to
    pub controller_id: u32,
    /// Chip select line number
    pub chip_select: u8,
    /// SPI mode flags (CPHA, CPOL, CS_HIGH, LSB_FIRST)
    pub mode: u8,
    /// Maximum clock speed in Hz for this device
    pub max_speed_hz: u32,
    /// Bits per word (typically 8)
    pub bits_per_word: u8,
    /// Human-readable device name (null-padded)
    pub name: [u8; 16],
    /// Whether this device slot is occupied
    pub active: bool,
}

impl SpiDevice {
    pub const fn empty() -> Self {
        SpiDevice {
            controller_id: 0,
            chip_select: 0,
            mode: 0,
            max_speed_hz: 0,
            bits_per_word: 8,
            name: [0u8; 16],
            active: false,
        }
    }
}

/// A full-duplex SPI transfer descriptor
#[derive(Copy, Clone)]
pub struct SpiTransfer {
    /// Transmit buffer
    pub tx_buf: [u8; 256],
    /// Receive buffer (filled by spi_transfer)
    pub rx_buf: [u8; 256],
    /// Number of bytes to transfer
    pub len: u16,
    /// If true, deassert CS after this transfer and reassert before next
    pub cs_change: bool,
}

impl SpiTransfer {
    pub const fn empty() -> Self {
        SpiTransfer {
            tx_buf: [0u8; 256],
            rx_buf: [0u8; 256],
            len: 0,
            cs_change: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SPI_CONTROLLERS: Mutex<[SpiController; MAX_SPI_CONTROLLERS]> =
    Mutex::new([SpiController::empty(); MAX_SPI_CONTROLLERS]);

static SPI_DEVICES: Mutex<[SpiDevice; MAX_SPI_DEVICES]> =
    Mutex::new([SpiDevice::empty(); MAX_SPI_DEVICES]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Find controller index by id. Returns None if not found or not active.
fn find_controller_idx(controller_id: u32) -> Option<usize> {
    let controllers = SPI_CONTROLLERS.lock();
    let mut result: Option<usize> = None;
    let mut i: usize = 0;
    while i < MAX_SPI_CONTROLLERS {
        if controllers[i].active && controllers[i].id == controller_id {
            result = Some(i);
            break;
        }
        i = i.saturating_add(1);
    }
    result
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new SPI controller.
///
/// Returns the assigned controller id on success, or None if no slots remain.
pub fn spi_register_controller(io_base: u16, max_speed_hz: u32) -> Option<u32> {
    let mut controllers = SPI_CONTROLLERS.lock();
    let mut i: usize = 0;
    while i < MAX_SPI_CONTROLLERS {
        if !controllers[i].active {
            // Use slot index + 1 as the controller id (0 reserved as "unset")
            let id = (i as u32).wrapping_add(1);
            controllers[i] = SpiController {
                id,
                io_base,
                max_speed_hz,
                active: true,
            };
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Register a SPI device on an existing controller.
///
/// Returns true on success, false if the controller is not found or no device
/// slots remain.
pub fn spi_register_device(
    controller_id: u32,
    cs: u8,
    mode: u8,
    speed_hz: u32,
    name: &[u8],
) -> bool {
    // Verify controller exists
    {
        let controllers = SPI_CONTROLLERS.lock();
        let mut found = false;
        let mut i: usize = 0;
        while i < MAX_SPI_CONTROLLERS {
            if controllers[i].active && controllers[i].id == controller_id {
                found = true;
                break;
            }
            i = i.saturating_add(1);
        }
        if !found {
            return false;
        }
    }

    let mut devices = SPI_DEVICES.lock();
    let mut i: usize = 0;
    while i < MAX_SPI_DEVICES {
        if !devices[i].active {
            let mut dev = SpiDevice {
                controller_id,
                chip_select: cs,
                mode,
                max_speed_hz: speed_hz,
                bits_per_word: 8,
                name: [0u8; 16],
                active: true,
            };
            // Copy name bytes (up to 15 chars + null terminator)
            let copy_len = if name.len() < 15 { name.len() } else { 15 };
            let mut j: usize = 0;
            while j < copy_len {
                dev.name[j] = name[j];
                j = j.saturating_add(1);
            }
            // name[copy_len] already 0 from initialization
            devices[i] = dev;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Perform a full-duplex SPI transfer.
///
/// Simulated loopback: rx_buf[i] = tx_buf[i].wrapping_add(1).
/// Returns false if the controller is not found or len is zero.
pub fn spi_transfer(controller_id: u32, cs: u8, xfer: &mut SpiTransfer) -> bool {
    if xfer.len == 0 {
        return false;
    }
    // Verify controller exists
    let found = find_controller_idx(controller_id).is_some();
    if !found {
        return false;
    }

    let len = xfer.len as usize;
    let safe_len = if len <= 256 { len } else { 256 };

    // Simulated loopback transfer: increment each byte for test verification
    let mut i: usize = 0;
    while i < safe_len {
        xfer.rx_buf[i] = xfer.tx_buf[i].wrapping_add(1);
        i = i.saturating_add(1);
    }
    true
}

/// Write bytes to a SPI device (transmit only; receive is discarded).
///
/// Returns false if data is empty, exceeds 256 bytes, or controller not found.
pub fn spi_write(controller_id: u32, cs: u8, data: &[u8]) -> bool {
    if data.is_empty() || data.len() > 256 {
        return false;
    }
    if find_controller_idx(controller_id).is_none() {
        return false;
    }

    let mut xfer = SpiTransfer::empty();
    let len = data.len();
    xfer.len = len as u16;

    let mut i: usize = 0;
    while i < len {
        xfer.tx_buf[i] = data[i];
        i = i.saturating_add(1);
    }

    // Loopback — rx discarded
    spi_transfer(controller_id, cs, &mut xfer)
}

/// Read bytes from a SPI device (transmit zeros; receive into buf).
///
/// Returns false if len is zero, exceeds 256, or controller not found.
pub fn spi_read(controller_id: u32, cs: u8, buf: &mut [u8; 256], len: usize) -> bool {
    if len == 0 || len > 256 {
        return false;
    }
    if find_controller_idx(controller_id).is_none() {
        return false;
    }

    let mut xfer = SpiTransfer::empty();
    xfer.len = len as u16;
    // tx_buf stays all zeros — loopback will return 0u8.wrapping_add(1) = 1

    if !spi_transfer(controller_id, cs, &mut xfer) {
        return false;
    }

    let mut i: usize = 0;
    while i < len {
        buf[i] = xfer.rx_buf[i];
        i = i.saturating_add(1);
    }
    true
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the SPI bus framework.
///
/// Registers a default simulated controller at SPI_IO_BASE.
pub fn init() {
    match spi_register_controller(SPI_IO_BASE, 10_000_000) {
        Some(id) => {
            serial_println!("[spi] framework initialized (controller id={})", id);
        }
        None => {
            serial_println!("[spi] framework initialized (no controller slots available)");
        }
    }
}
