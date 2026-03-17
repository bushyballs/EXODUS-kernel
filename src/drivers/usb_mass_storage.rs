/// USB Mass Storage Class Driver — Bulk-Only Transport (BOT)
///
/// Implements the USB Mass Storage Class (MSC) using the Bulk-Only Transport
/// protocol (BBB / BOT).  A Command Block Wrapper (CBW) is sent from the host
/// to the device to issue a SCSI command; the device responds with data (if
/// any) and a Command Status Wrapper (CSW) indicating success or failure.
///
/// This is a no-heap simulation driver.  No actual USB I/O is performed;
/// block reads return a deterministic fill pattern and block writes validate
/// parameters and succeed silently.
///
/// Rules enforced:
///   - No heap (no Vec, Box, String, alloc::*)
///   - No floats (no as f32 / as f64)
///   - No panics (no unwrap, expect, panic!)
///   - Counters: saturating_add / saturating_sub
///   - Sequence numbers: wrapping_add
///   - No division without guarding divisor != 0
///   - Structs in static Mutex must be Copy + have `const fn empty()`
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

/// USB class code for Mass Storage
pub const USB_CLASS_MASS_STORAGE: u8 = 0x08;

/// SCSI transparent command set subclass
pub const USB_SUBCLASS_SCSI: u8 = 0x06;

/// Bulk-Only Transport protocol
pub const USB_PROTO_BOT: u8 = 0x50;

/// Maximum simultaneous MSC devices
pub const MAX_MSC_DEVICES: usize = 4;

/// Standard block size in bytes
pub const MSC_BLOCK_SIZE: usize = 512;

/// CBW signature: "USBC" little-endian
pub const MSC_CBW_SIGNATURE: u32 = 0x43425355;

/// CSW signature: "USBS" little-endian
pub const MSC_CSW_SIGNATURE: u32 = 0x53425355;

/// I/O buffer size: 4 blocks (2 KiB)
const IO_BUF_LEN: usize = MSC_BLOCK_SIZE * 4;

/// Simulated drive: 2 097 152 blocks × 512 = 1 GiB
const SIM_BLOCK_COUNT: u64 = 2 * 1024 * 1024;

/// Simulated block size
const SIM_BLOCK_SIZE: u32 = 512;

/// SCSI READ(10) opcode
const SCSI_READ10: u8 = 0x28;

/// SCSI WRITE(10) opcode
const SCSI_WRITE10: u8 = 0x2A;

// ============================================================================
// Packed on-wire structures
//
// These are only used as documentation of the wire format; fields are written
// manually into a [u8; 31] / [u8; 13] buffer to avoid undefined behaviour
// from reading packed fields through references.
// ============================================================================

/// Command Block Wrapper (31 bytes, little-endian)
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct MscCbw {
    pub signature: u32, // 0x43425355
    pub tag: u32,
    pub data_transfer_len: u32,
    pub flags: u8, // 0x80 = data-in (device→host)
    pub lun: u8,
    pub cb_len: u8,   // 6..16
    pub cb: [u8; 16], // SCSI command block
}

/// Command Status Wrapper (13 bytes, little-endian)
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct MscCsw {
    pub signature: u32, // 0x53425355
    pub tag: u32,
    pub residue: u32,
    pub status: u8, // 0=pass, 1=fail, 2=phase error
}

// ============================================================================
// Device record
// ============================================================================

/// Per-device state for a USB MSC device.
///
/// All fields are plain data — no heap allocations.  The struct is `Copy` so
/// it can be placed in a `static Mutex<[MscDevice; N]>`.
#[derive(Copy, Clone)]
pub struct MscDevice {
    /// Unique device identifier
    pub id: u32,
    /// Number of logical units (LUNs)
    pub lun_count: u8,
    /// Total number of logical blocks on the medium
    pub block_count: u64,
    /// Bytes per logical block
    pub block_size: u32,
    /// CBW tag counter — incremented with wrapping_add for every command
    pub tag_counter: u32,
    /// Scratch I/O buffer (4 blocks)
    pub io_buf: [u8; IO_BUF_LEN],
    /// Slot is occupied (false = free for reuse)
    pub present: bool,
    /// Device is operational
    pub active: bool,
}

impl MscDevice {
    /// Construct a zero-initialised, inactive device slot.
    pub const fn empty() -> Self {
        MscDevice {
            id: 0,
            lun_count: 0,
            block_count: 0,
            block_size: 0,
            tag_counter: 0,
            io_buf: [0u8; IO_BUF_LEN],
            present: false,
            active: false,
        }
    }
}

// MscDevice contains only plain integer types and byte arrays.
unsafe impl Send for MscDevice {}

// ============================================================================
// Global device table
// ============================================================================

/// Protected table of MSC device slots.
static MSC_DEVICES: Mutex<[MscDevice; MAX_MSC_DEVICES]> =
    Mutex::new([MscDevice::empty(); MAX_MSC_DEVICES]);

/// Monotonically-assigned device ID (wrapping).
static NEXT_DEV_ID: Mutex<u32> = Mutex::new(1);

// ============================================================================
// Helpers — manual CBW/CSW serialisation
// ============================================================================

/// Write a u32 in little-endian byte order into `buf` at `offset`.
///
/// Returns `false` if the write would exceed the buffer bounds.
#[inline]
fn write_le32(buf: &mut [u8], offset: usize, val: u32) -> bool {
    if offset.saturating_add(4) > buf.len() {
        return false;
    }
    buf[offset] = (val & 0xFF) as u8;
    buf[offset + 1] = ((val >> 8) & 0xFF) as u8;
    buf[offset + 2] = ((val >> 16) & 0xFF) as u8;
    buf[offset + 3] = ((val >> 24) & 0xFF) as u8;
    true
}

/// Write a u64 as a big-endian 4-byte LBA into `buf` at `offset` (SCSI style).
///
/// Only the low 32 bits of `lba` are used; returns false on bounds failure or
/// if lba exceeds u32::MAX.
#[inline]
fn write_lba_be32(buf: &mut [u8], offset: usize, lba: u64) -> bool {
    if lba > 0xFFFF_FFFF {
        return false;
    }
    if offset.saturating_add(4) > buf.len() {
        return false;
    }
    let lba32 = lba as u32;
    buf[offset] = ((lba32 >> 24) & 0xFF) as u8;
    buf[offset + 1] = ((lba32 >> 16) & 0xFF) as u8;
    buf[offset + 2] = ((lba32 >> 8) & 0xFF) as u8;
    buf[offset + 3] = (lba32 & 0xFF) as u8;
    true
}

/// Build a 31-byte CBW into a local byte buffer.
///
/// Returns `false` on any encoding failure.
#[inline]
fn build_cbw(
    buf: &mut [u8; 31],
    tag: u32,
    data_transfer_len: u32,
    flags: u8,
    lun: u8,
    cb_len: u8,
    cb: &[u8; 16],
) -> bool {
    if !write_le32(buf, 0, MSC_CBW_SIGNATURE) {
        return false;
    }
    if !write_le32(buf, 4, tag) {
        return false;
    }
    if !write_le32(buf, 8, data_transfer_len) {
        return false;
    }
    buf[12] = flags;
    buf[13] = lun;
    buf[14] = cb_len;
    buf[15..31].copy_from_slice(cb);
    true
}

// ============================================================================
// Public API
// ============================================================================

/// Register a new MSC device with the given LUN count and total block count.
///
/// Returns `Some(device_id)` on success, `None` when the table is full.
pub fn msc_register(lun_count: u8, block_count: u64) -> Option<u32> {
    let mut devs = MSC_DEVICES.lock();

    // Find a free slot
    let mut free_idx: Option<usize> = None;
    for (i, slot) in devs.iter().enumerate() {
        if !slot.active {
            free_idx = Some(i);
            break;
        }
    }

    let idx = match free_idx {
        Some(i) => i,
        None => {
            serial_println!("[usb_msc] device table full");
            return None;
        }
    };

    // Assign unique ID
    let id = {
        let mut id_lock = NEXT_DEV_ID.lock();
        let current = *id_lock;
        *id_lock = current.wrapping_add(1);
        current
    };

    let dev = &mut devs[idx];
    *dev = MscDevice::empty();
    dev.id = id;
    dev.lun_count = lun_count;
    dev.block_count = block_count;
    dev.block_size = SIM_BLOCK_SIZE;
    dev.present = true;
    dev.active = true;

    serial_println!(
        "[usb_msc] registered device id={} luns={} blocks={}",
        id,
        lun_count,
        block_count
    );

    Some(id)
}

/// Read one 512-byte block at `lba` from device `dev_id` into `out`.
///
/// In simulation the block is filled with `(lba & 0xFF) as u8` repeated.
/// Returns `false` if the device is not found, not active, or `lba` is
/// out of range.
pub fn msc_read_block(dev_id: u32, lba: u64, out: &mut [u8; MSC_BLOCK_SIZE]) -> bool {
    // Validate device and bounds first (under lock, then drop lock before writing
    // out to avoid holding the mutex over a potentially large copy).
    let block_count = {
        let devs = MSC_DEVICES.lock();
        let mut found: Option<u64> = None;
        for dev in devs.iter() {
            if dev.active && dev.id == dev_id {
                found = Some(dev.block_count);
                break;
            }
        }
        match found {
            Some(bc) => bc,
            None => {
                serial_println!("[usb_msc] read_block: device id={} not found", dev_id);
                return false;
            }
        }
    };

    if lba >= block_count {
        serial_println!(
            "[usb_msc] read_block: lba={} out of range (max={})",
            lba,
            block_count
        );
        return false;
    }

    // Build CBW (simulation — no actual USB transfer)
    let mut cbw_buf = [0u8; 31];
    {
        let mut devs = MSC_DEVICES.lock();
        for dev in devs.iter_mut() {
            if dev.active && dev.id == dev_id {
                let tag = dev.tag_counter.wrapping_add(1);
                dev.tag_counter = tag;

                let mut cb = [0u8; 16];
                cb[0] = SCSI_READ10;
                // LBA at bytes 2-5 big-endian; transfer length (1 block) at 7-8
                let _ = write_lba_be32(&mut cb, 2, lba);
                cb[7] = 0x00;
                cb[8] = 0x01; // 1 block

                build_cbw(&mut cbw_buf, tag, MSC_BLOCK_SIZE as u32, 0x80, 0, 10, &cb);
                break;
            }
        }
    }

    // Simulate read: fill output with deterministic pattern
    let fill = (lba & 0xFF) as u8;
    let mut i = 0usize;
    while i < MSC_BLOCK_SIZE {
        out[i] = fill;
        i = i.saturating_add(1);
    }

    // Suppress unused variable warning for cbw_buf (it would be sent over USB)
    let _ = cbw_buf;

    true
}

/// Write one 512-byte block at `lba` to device `dev_id`.
///
/// In simulation this validates parameters and returns `true` without
/// performing any actual I/O.
pub fn msc_write_block(dev_id: u32, lba: u64, data: &[u8; MSC_BLOCK_SIZE]) -> bool {
    let block_count = {
        let devs = MSC_DEVICES.lock();
        let mut found: Option<u64> = None;
        for dev in devs.iter() {
            if dev.active && dev.id == dev_id {
                found = Some(dev.block_count);
                break;
            }
        }
        match found {
            Some(bc) => bc,
            None => {
                serial_println!("[usb_msc] write_block: device id={} not found", dev_id);
                return false;
            }
        }
    };

    if lba >= block_count {
        serial_println!(
            "[usb_msc] write_block: lba={} out of range (max={})",
            lba,
            block_count
        );
        return false;
    }

    // Build CBW (simulation — no actual USB transfer)
    let mut cbw_buf = [0u8; 31];
    {
        let mut devs = MSC_DEVICES.lock();
        for dev in devs.iter_mut() {
            if dev.active && dev.id == dev_id {
                let tag = dev.tag_counter.wrapping_add(1);
                dev.tag_counter = tag;

                let mut cb = [0u8; 16];
                cb[0] = SCSI_WRITE10;
                let _ = write_lba_be32(&mut cb, 2, lba);
                cb[7] = 0x00;
                cb[8] = 0x01; // 1 block

                build_cbw(&mut cbw_buf, tag, MSC_BLOCK_SIZE as u32, 0x00, 0, 10, &cb);
                break;
            }
        }
    }

    // Suppress unused variable warnings
    let _ = cbw_buf;
    let _ = data;

    true
}

/// Return `(block_count, block_size)` for the given device.
///
/// Returns `None` if the device is not found or not active.
pub fn msc_get_capacity(dev_id: u32) -> Option<(u64, u32)> {
    let devs = MSC_DEVICES.lock();
    for dev in devs.iter() {
        if dev.active && dev.id == dev_id {
            return Some((dev.block_count, dev.block_size));
        }
    }
    None
}

// ============================================================================
// Module init
// ============================================================================

/// Initialise the USB Mass Storage driver.
///
/// Registers a simulated 1 GiB drive (2 097 152 blocks × 512 bytes).
pub fn init() {
    let _ = msc_register(1, SIM_BLOCK_COUNT);
    serial_println!("[usb_msc] mass storage driver initialized");
    super::register("usb-mass-storage", super::DeviceType::Storage);
}
