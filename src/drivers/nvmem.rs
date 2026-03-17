/// nvmem — Non-Volatile Memory (NVMEM) framework
///
/// Provides a unified interface for reading/writing small NVM cells:
///   - EEPROM, EFUSE, OTP, NVRAM, SRAM-backed storage
///   - Byte-granular cell addressing
///   - Provider registration (multiple NVM devices)
///   - Named cell lookup (MAC address, calibration data, serial, etc.)
///
/// Inspired by: Linux drivers/nvmem/core.c. All code is original.
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MAX_PROVIDERS: usize = 8;
const MAX_CELLS: usize = 64;
const MAX_CELL_BYTES: usize = 32; // max bytes per named cell
const NVM_SIZE: usize = 4096; // per-provider simulated NVM storage
const CELL_NAME_LEN: usize = 24;

// ---------------------------------------------------------------------------
// NVMEM cell descriptor
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct NvmemCell {
    pub name: [u8; CELL_NAME_LEN],
    pub name_len: u8,
    pub provider_id: u32,
    pub offset: u32, // byte offset within provider NVM
    pub nbits: u8,   // number of bits (1..=256)
    pub active: bool,
}

impl NvmemCell {
    pub const fn empty() -> Self {
        NvmemCell {
            name: [0u8; CELL_NAME_LEN],
            name_len: 0,
            provider_id: 0,
            offset: 0,
            nbits: 8,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// NVMEM provider (device)
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum NvmemType {
    Eeprom,
    Efuse,
    Otp,
    Nvram,
    Unknown,
}

#[derive(Copy, Clone)]
pub struct NvmemProvider {
    pub id: u32,
    pub name: [u8; 24],
    pub name_len: u8,
    pub nvm_type: NvmemType,
    pub size: u32,
    pub data: [u8; NVM_SIZE],
    pub active: bool,
    pub read_only: bool,
}

impl NvmemProvider {
    pub const fn empty() -> Self {
        NvmemProvider {
            id: 0,
            name: [0u8; 24],
            name_len: 0,
            nvm_type: NvmemType::Unknown,
            size: 0,
            data: [0u8; NVM_SIZE],
            active: false,
            read_only: false,
        }
    }
}

fn copy_name_n<const N: usize>(dst: &mut [u8; N], src: &[u8]) -> u8 {
    let len = src.len().min(N - 1);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

const EMPTY_PROV: NvmemProvider = NvmemProvider::empty();
const EMPTY_CELL: NvmemCell = NvmemCell::empty();
static PROVIDERS: Mutex<[NvmemProvider; MAX_PROVIDERS]> = Mutex::new([EMPTY_PROV; MAX_PROVIDERS]);
static CELLS: Mutex<[NvmemCell; MAX_CELLS]> = Mutex::new([EMPTY_CELL; MAX_CELLS]);
static PROV_NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Public API: providers
// ---------------------------------------------------------------------------

/// Register an NVMEM provider. Returns provider id, or 0 on failure.
pub fn nvmem_register(name: &[u8], nvm_type: NvmemType, size: u32, read_only: bool) -> u32 {
    let id = PROV_NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut ps = PROVIDERS.lock();
    let mut i = 0usize;
    while i < MAX_PROVIDERS {
        if !ps[i].active {
            ps[i] = NvmemProvider::empty();
            ps[i].id = id;
            ps[i].name_len = copy_name_n(&mut ps[i].name, name);
            ps[i].nvm_type = nvm_type;
            ps[i].size = size.min(NVM_SIZE as u32);
            ps[i].read_only = read_only;
            ps[i].active = true;
            return id;
        }
        i = i.saturating_add(1);
    }
    0
}

pub fn nvmem_unregister(id: u32) -> bool {
    let mut ps = PROVIDERS.lock();
    let mut i = 0usize;
    while i < MAX_PROVIDERS {
        if ps[i].active && ps[i].id == id {
            ps[i].active = false;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Public API: raw read/write
// ---------------------------------------------------------------------------

/// Read raw bytes from a provider starting at byte offset.
pub fn nvmem_read(id: u32, offset: u32, buf: &mut [u8]) -> bool {
    let mut ps = PROVIDERS.lock();
    let mut i = 0usize;
    while i < MAX_PROVIDERS {
        if ps[i].active && ps[i].id == id {
            let off = offset as usize;
            let len = buf.len().min(NVM_SIZE - off.min(NVM_SIZE));
            let mut k = 0usize;
            while k < len {
                buf[k] = ps[i].data[off + k];
                k = k.saturating_add(1);
            }
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Write raw bytes to a provider (fails if read_only).
pub fn nvmem_write(id: u32, offset: u32, data: &[u8]) -> bool {
    let mut ps = PROVIDERS.lock();
    let mut i = 0usize;
    while i < MAX_PROVIDERS {
        if ps[i].active && ps[i].id == id {
            if ps[i].read_only {
                return false;
            }
            let off = offset as usize;
            let len = data.len().min(NVM_SIZE - off.min(NVM_SIZE));
            let mut k = 0usize;
            while k < len {
                ps[i].data[off + k] = data[k];
                k = k.saturating_add(1);
            }
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

// ---------------------------------------------------------------------------
// Public API: named cells
// ---------------------------------------------------------------------------

/// Register a named cell mapping. Returns true on success.
pub fn nvmem_cell_register(name: &[u8], provider_id: u32, offset: u32, nbits: u8) -> bool {
    let mut cells = CELLS.lock();
    let mut i = 0usize;
    while i < MAX_CELLS {
        if !cells[i].active {
            cells[i] = NvmemCell::empty();
            cells[i].name_len = copy_name_n(&mut cells[i].name, name);
            cells[i].provider_id = provider_id;
            cells[i].offset = offset;
            cells[i].nbits = nbits;
            cells[i].active = true;
            return true;
        }
        i = i.saturating_add(1);
    }
    false
}

/// Read a named cell into buf. Returns bytes read, or 0 if not found.
pub fn nvmem_cell_read(name: &[u8], buf: &mut [u8; MAX_CELL_BYTES]) -> usize {
    let nlen = name.len().min(CELL_NAME_LEN);
    let cells = CELLS.lock();
    let mut i = 0usize;
    while i < MAX_CELLS {
        if cells[i].active && cells[i].name_len as usize == nlen {
            let mut eq = true;
            let mut k = 0usize;
            while k < nlen {
                if cells[i].name[k] != name[k] {
                    eq = false;
                    break;
                }
                k = k.saturating_add(1);
            }
            if eq {
                let provider_id = cells[i].provider_id;
                let offset = cells[i].offset;
                let nbits = cells[i].nbits;
                let byte_len = ((nbits as usize).saturating_add(7)) / 8;
                let read_len = byte_len.min(MAX_CELL_BYTES);
                drop(cells);
                nvmem_read(provider_id, offset, &mut buf[..read_len]);
                return read_len;
            }
        }
        i = i.saturating_add(1);
    }
    0
}

/// Write a named cell. Returns true on success.
pub fn nvmem_cell_write(name: &[u8], data: &[u8]) -> bool {
    let nlen = name.len().min(CELL_NAME_LEN);
    let cells = CELLS.lock();
    let mut i = 0usize;
    while i < MAX_CELLS {
        if cells[i].active && cells[i].name_len as usize == nlen {
            let mut eq = true;
            let mut k = 0usize;
            while k < nlen {
                if cells[i].name[k] != name[k] {
                    eq = false;
                    break;
                }
                k = k.saturating_add(1);
            }
            if eq {
                let provider_id = cells[i].provider_id;
                let offset = cells[i].offset;
                drop(cells);
                return nvmem_write(provider_id, offset, data);
            }
        }
        i = i.saturating_add(1);
    }
    false
}

pub fn init() {
    // Register a simulated EEPROM (4KB, writable) with common cells
    let eeprom_id = nvmem_register(b"sim-eeprom0", NvmemType::Eeprom, 4096, false);
    if eeprom_id != 0 {
        // Pre-populate MAC address cell at offset 0 (6 bytes = 48 bits)
        nvmem_cell_register(b"mac-address", eeprom_id, 0, 48);
        // Write a simulated MAC
        nvmem_write(eeprom_id, 0, &[0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
        // Serial number cell at offset 8 (16 bytes = 128 bits)
        nvmem_cell_register(b"serial-number", eeprom_id, 8, 128);
        // Calibration data at offset 32 (32 bytes)
        nvmem_cell_register(b"calibration", eeprom_id, 32, 255);
    }
    serial_println!(
        "[nvmem] NVMEM framework initialized (max {} providers, {} cells)",
        MAX_PROVIDERS,
        MAX_CELLS
    );
}
