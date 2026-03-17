use crate::sync::Mutex;
/// I2C bus master framework for Genesis — no-heap, fixed-size static arrays
///
/// Implements a simulated I2C master with adapter and client registries.
/// All transfers use loopback simulation: read operations fill the buffer
/// with (addr & 0xFF) repeated; write operations are acknowledged silently.
/// SMBus helper functions (read_byte, write_byte, read_word) are built on
/// top of the core i2c_transfer path.
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

/// Maximum number of I2C adapters (bus controllers) supported
pub const MAX_I2C_ADAPTERS: usize = 4;

/// Maximum number of I2C client devices supported
pub const MAX_I2C_CLIENTS: usize = 32;

/// Standard mode speed: 100 kHz
pub const I2C_SPEED_STANDARD: u32 = 100_000;

/// Fast mode speed: 400 kHz
pub const I2C_SPEED_FAST: u32 = 400_000;

/// Fast-plus mode speed: 1 MHz
pub const I2C_SPEED_FAST_PLUS: u32 = 1_000_000;

/// Simulated I2C controller base I/O port
pub const I2C_IO_BASE: u16 = 0x3B0;

// ---------------------------------------------------------------------------
// I2cMsg flags
// ---------------------------------------------------------------------------

/// Read direction flag for I2cMsg
pub const I2C_M_RD: u16 = 1;

/// Ten-bit address flag for I2cMsg
pub const I2C_M_TEN: u16 = 0x10;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A registered I2C adapter (bus controller)
#[derive(Copy, Clone)]
pub struct I2cAdapter {
    /// Unique adapter identifier (assigned on registration)
    pub id: u32,
    /// Base I/O port for this adapter
    pub io_base: u16,
    /// Bus speed in Hz
    pub speed_hz: u32,
    /// Whether this adapter slot is occupied
    pub active: bool,
}

impl I2cAdapter {
    pub const fn empty() -> Self {
        I2cAdapter {
            id: 0,
            io_base: 0,
            speed_hz: 0,
            active: false,
        }
    }
}

/// A registered I2C client device
#[derive(Copy, Clone)]
pub struct I2cClient {
    /// ID of the adapter this client is attached to
    pub adapter_id: u32,
    /// 7-bit or 10-bit I2C address
    pub addr: u16,
    /// Human-readable device name (null-padded)
    pub name: [u8; 16],
    /// Whether this client slot is occupied
    pub active: bool,
}

impl I2cClient {
    pub const fn empty() -> Self {
        I2cClient {
            adapter_id: 0,
            addr: 0,
            name: [0u8; 16],
            active: false,
        }
    }
}

/// A single I2C message descriptor
#[derive(Copy, Clone)]
pub struct I2cMsg {
    /// Target device address
    pub addr: u16,
    /// Flags: I2C_M_RD for read, I2C_M_TEN for 10-bit addressing
    pub flags: u16,
    /// Number of bytes to transfer (max 64)
    pub len: u8,
    /// Data buffer
    pub buf: [u8; 64],
}

impl I2cMsg {
    pub const fn empty() -> Self {
        I2cMsg {
            addr: 0,
            flags: 0,
            len: 0,
            buf: [0u8; 64],
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static I2C_ADAPTERS: Mutex<[I2cAdapter; MAX_I2C_ADAPTERS]> =
    Mutex::new([I2cAdapter::empty(); MAX_I2C_ADAPTERS]);

static I2C_CLIENTS: Mutex<[I2cClient; MAX_I2C_CLIENTS]> =
    Mutex::new([I2cClient::empty(); MAX_I2C_CLIENTS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Verify that an adapter with the given id exists and is active.
fn adapter_exists(adapter_id: u32) -> bool {
    let adapters = I2C_ADAPTERS.lock();
    let mut i: usize = 0;
    while i < MAX_I2C_ADAPTERS {
        if adapters[i].active && adapters[i].id == adapter_id {
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new I2C adapter (bus controller).
///
/// Returns the assigned adapter id on success, or None if no slots remain.
pub fn i2c_register_adapter(io_base: u16, speed_hz: u32) -> Option<u32> {
    let mut adapters = I2C_ADAPTERS.lock();
    let mut i: usize = 0;
    while i < MAX_I2C_ADAPTERS {
        if !adapters[i].active {
            // Use slot index + 1 as the adapter id (0 reserved as "unset")
            let id = (i as u32).wrapping_add(1);
            adapters[i] = I2cAdapter {
                id,
                io_base,
                speed_hz,
                active: true,
            };
            return Some(id);
        }
        i = i.saturating_add(1);
    }
    None
}

/// Register an I2C client device on an existing adapter.
///
/// Returns true on success, false if the adapter is not found or no client
/// slots remain.
pub fn i2c_register_client(adapter_id: u32, addr: u16, name: &[u8]) -> bool {
    // Verify adapter exists
    if !adapter_exists(adapter_id) {
        return false;
    }

    let mut clients = I2C_CLIENTS.lock();
    let mut i: usize = 0;
    while i < MAX_I2C_CLIENTS {
        if !clients[i].active {
            let mut client = I2cClient {
                adapter_id,
                addr,
                name: [0u8; 16],
                active: true,
            };
            // Copy name bytes (up to 15 chars + null terminator)
            let copy_len = if name.len() < 15 { name.len() } else { 15 };
            let mut j: usize = 0;
            while j < copy_len {
                client.name[j] = name[j];
                j = j.saturating_add(1);
            }
            clients[i] = client;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Perform a sequence of I2C messages.
///
/// Simulated behaviour:
///   - If I2C_M_RD flag is set: fill msg.buf[0..len] with (addr & 0xFF).
///   - Otherwise (write): silently acknowledge (no-op loopback).
///
/// Returns false if the adapter is not found or nmsgs is zero.
pub fn i2c_transfer(adapter_id: u32, msgs: &mut [I2cMsg; 8], nmsgs: usize) -> bool {
    if nmsgs == 0 {
        return false;
    }
    if !adapter_exists(adapter_id) {
        return false;
    }

    let count = if nmsgs <= 8 { nmsgs } else { 8 };
    let mut m: usize = 0;
    while m < count {
        let msg = &mut msgs[m];
        if msg.flags & I2C_M_RD != 0 {
            // Read: fill buffer with low byte of address
            let fill_byte = (msg.addr & 0xFF) as u8;
            let fill_len = msg.len as usize;
            let safe_len = if fill_len <= 64 { fill_len } else { 64 };
            let mut i: usize = 0;
            while i < safe_len {
                msg.buf[i] = fill_byte;
                i = i.saturating_add(1);
            }
        }
        // Write: no-op loopback — device is simulated as ACKing everything
        m = m.saturating_add(1);
    }
    true
}

/// SMBus read byte: read a single byte from register `reg` at device `addr`.
///
/// Returns Some(byte) on success, None if the adapter is not found.
pub fn i2c_smbus_read_byte(adapter_id: u32, addr: u16, reg: u8) -> Option<u8> {
    if !adapter_exists(adapter_id) {
        return None;
    }

    // Build a write message for the register address followed by a read message
    let mut msgs = [I2cMsg::empty(); 8];

    // Message 0: write register address
    msgs[0].addr = addr;
    msgs[0].flags = 0;
    msgs[0].len = 1;
    msgs[0].buf[0] = reg;

    // Message 1: read one byte back
    msgs[1].addr = addr;
    msgs[1].flags = I2C_M_RD;
    msgs[1].len = 1;

    if !i2c_transfer(adapter_id, &mut msgs, 2) {
        return None;
    }

    Some(msgs[1].buf[0])
}

/// SMBus write byte: write a single byte `val` to register `reg` at device `addr`.
///
/// Returns true on success, false if the adapter is not found.
pub fn i2c_smbus_write_byte(adapter_id: u32, addr: u16, reg: u8, val: u8) -> bool {
    if !adapter_exists(adapter_id) {
        return false;
    }

    let mut msgs = [I2cMsg::empty(); 8];

    // Single write message: [register, value]
    msgs[0].addr = addr;
    msgs[0].flags = 0;
    msgs[0].len = 2;
    msgs[0].buf[0] = reg;
    msgs[0].buf[1] = val;

    i2c_transfer(adapter_id, &mut msgs, 1)
}

/// SMBus read word: read a 16-bit word from register `reg` at device `addr`.
///
/// The word is assembled as low byte (buf[0]) | (high byte (buf[1]) << 8).
/// Returns Some(word) on success, None if the adapter is not found.
pub fn i2c_smbus_read_word(adapter_id: u32, addr: u16, reg: u8) -> Option<u16> {
    if !adapter_exists(adapter_id) {
        return None;
    }

    let mut msgs = [I2cMsg::empty(); 8];

    // Message 0: write register address
    msgs[0].addr = addr;
    msgs[0].flags = 0;
    msgs[0].len = 1;
    msgs[0].buf[0] = reg;

    // Message 1: read two bytes back
    msgs[1].addr = addr;
    msgs[1].flags = I2C_M_RD;
    msgs[1].len = 2;

    if !i2c_transfer(adapter_id, &mut msgs, 2) {
        return None;
    }

    let low = msgs[1].buf[0] as u16;
    let high = msgs[1].buf[1] as u16;
    Some(low | (high << 8))
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the I2C bus framework.
///
/// Registers a default simulated adapter at I2C_IO_BASE running at standard
/// (100 kHz) speed.
pub fn init() {
    match i2c_register_adapter(I2C_IO_BASE, I2C_SPEED_STANDARD) {
        Some(id) => {
            serial_println!("[i2c] framework initialized (adapter id={})", id);
        }
        None => {
            serial_println!("[i2c] framework initialized (no adapter slots available)");
        }
    }
}
