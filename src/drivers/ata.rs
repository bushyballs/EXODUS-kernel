use crate::io::{inb, inw, outb};
use crate::memory::frame_allocator;
use crate::memory::frame_allocator::FRAME_SIZE;
use crate::sync::Mutex;
/// ATA PIO disk driver for Genesis — built from scratch
///
/// Implements ATA PIO mode for reading/writing sectors to IDE hard drives.
/// PIO (Programmed I/O) is the simplest ATA mode — no DMA needed.
///
/// Supports:
///   - PIO mode read/write (ports 0x1F0-0x1F7 for primary, 0x170-0x177 for secondary)
///   - LBA48 addressing (drives > 128GB)
///   - READ SECTORS EXT / WRITE SECTORS EXT commands
///   - Drive selection (master/slave via drive/head register)
///   - BSY/DRQ polling with timeout
///   - IDENTIFY DEVICE: model string, capacity, feature flags
///   - ATAPI support: PACKET command for CD-ROM
///   - Error handling: ERR bit, error register parsing
///   - DMA setup (if BMIDE detected): PRDT, start/stop DMA
///
/// Primary ATA channel: I/O ports 0x1F0-0x1F7, control 0x3F6
/// Secondary ATA channel: I/O ports 0x170-0x177, control 0x376
///
/// No external crates. All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

/// ATA I/O port offsets (relative to channel base)
const ATA_DATA: u16 = 0; // Read/write PIO data (16-bit)
const ATA_ERROR: u16 = 1; // Read: error register
const ATA_FEATURES: u16 = 1; // Write: features register
const ATA_SECT_COUNT: u16 = 2; // Sector count
const ATA_LBA_LO: u16 = 3; // LBA bits 0-7
const ATA_LBA_MID: u16 = 4; // LBA bits 8-15
const ATA_LBA_HI: u16 = 5; // LBA bits 16-23
const ATA_DRIVE_HEAD: u16 = 6; // Drive/head register
const ATA_STATUS: u16 = 7; // Read: status register
const ATA_COMMAND: u16 = 7; // Write: command register

/// ATA status bits
const ATA_SR_BSY: u8 = 0x80; // Busy
const ATA_SR_DRDY: u8 = 0x40; // Drive ready
const ATA_SR_DF: u8 = 0x20; // Drive fault
const ATA_SR_DSC: u8 = 0x10; // Drive seek complete
const ATA_SR_DRQ: u8 = 0x08; // Data request
const ATA_SR_CORR: u8 = 0x04; // Corrected data
const ATA_SR_IDX: u8 = 0x02; // Index
const ATA_SR_ERR: u8 = 0x01; // Error

/// ATA error register bits
const ATA_ERR_AMNF: u8 = 0x01; // Address mark not found
const ATA_ERR_TK0NF: u8 = 0x02; // Track 0 not found
const ATA_ERR_ABRT: u8 = 0x04; // Command aborted
const ATA_ERR_MCR: u8 = 0x08; // Media change request
const ATA_ERR_IDNF: u8 = 0x10; // ID not found
const ATA_ERR_MC: u8 = 0x20; // Media changed
const ATA_ERR_UNC: u8 = 0x40; // Uncorrectable data error
const ATA_ERR_BBK: u8 = 0x80; // Bad block detected

/// ATA commands
const ATA_CMD_READ_PIO: u8 = 0x20; // Read Sectors (28-bit LBA)
const ATA_CMD_READ_PIO_EXT: u8 = 0x24; // Read Sectors EXT (48-bit LBA)
const ATA_CMD_READ_DMA: u8 = 0xC8; // Read DMA (28-bit)
const ATA_CMD_READ_DMA_EXT: u8 = 0x25; // Read DMA EXT (48-bit)
const ATA_CMD_WRITE_PIO: u8 = 0x30; // Write Sectors (28-bit)
const ATA_CMD_WRITE_PIO_EXT: u8 = 0x34; // Write Sectors EXT (48-bit)
const ATA_CMD_WRITE_DMA: u8 = 0xCA; // Write DMA (28-bit)
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35; // Write DMA EXT (48-bit)
const ATA_CMD_CACHE_FLUSH: u8 = 0xE7; // Flush Cache
const ATA_CMD_CACHE_FLUSH_EXT: u8 = 0xEA; // Flush Cache EXT
const ATA_CMD_IDENTIFY: u8 = 0xEC; // Identify Device (ATA)
const ATA_CMD_IDENTIFY_PACKET: u8 = 0xA1; // Identify Packet Device (ATAPI)
const ATA_CMD_PACKET: u8 = 0xA0; // ATAPI Packet Command
const ATA_CMD_SET_FEATURES: u8 = 0xEF; // Set Features
const ATA_CMD_DEVICE_RESET: u8 = 0x08; // Device Reset

/// ATAPI commands (sent via ATA PACKET command)
const ATAPI_CMD_READ: u8 = 0xA8; // ATAPI Read (12)
const ATAPI_CMD_READ_CAPACITY: u8 = 0x25; // Read Capacity
const ATAPI_CMD_TEST_UNIT_READY: u8 = 0x00; // Test Unit Ready
const ATAPI_CMD_REQUEST_SENSE: u8 = 0x03; // Request Sense
const ATAPI_CMD_EJECT: u8 = 0x1B; // Start/Stop Unit (Eject)

/// Bus Master IDE (BMIDE) register offsets
const BMIDE_COMMAND: u16 = 0x00;
const BMIDE_STATUS: u16 = 0x02;
const BMIDE_PRDT: u16 = 0x04;

/// BMIDE command bits
const BMIDE_CMD_START: u8 = 0x01;
const BMIDE_CMD_READ: u8 = 0x08; // 1 = read (device to memory)

/// BMIDE status bits
const BMIDE_STS_ACTIVE: u8 = 0x01;
const BMIDE_STS_ERR: u8 = 0x02;
const BMIDE_STS_IRQ: u8 = 0x04;

/// Sector size in bytes
pub const SECTOR_SIZE: usize = 512;

/// Maximum sectors for a single 28-bit LBA PIO transfer
const MAX_SECTORS_28: u8 = 255;
/// Maximum sectors for a single 48-bit LBA PIO transfer
const MAX_SECTORS_48: u16 = 65535;

/// Physical Region Descriptor Table entry for DMA
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct PrdtEntry {
    phys_addr: u32,  // Physical buffer address
    byte_count: u16, // Transfer size (0 = 64KB)
    flags: u16,      // Bit 15 = end of table
}

/// An ATA drive
#[derive(Debug, Clone)]
pub struct AtaDrive {
    pub channel: AtaChannel,
    pub drive: u8, // 0 = master, 1 = slave
    pub present: bool,
    pub atapi: bool,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub sectors: u64,
    pub lba48: bool,
    pub dma_supported: bool,
    pub udma_mode: u8,
    pub max_sectors_per_transfer: u16,
}

/// ATA channel (primary or secondary)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaChannel {
    Primary,
    Secondary,
}

impl AtaChannel {
    fn base_port(&self) -> u16 {
        match self {
            AtaChannel::Primary => 0x1F0,
            AtaChannel::Secondary => 0x170,
        }
    }

    fn control_port(&self) -> u16 {
        match self {
            AtaChannel::Primary => 0x3F6,
            AtaChannel::Secondary => 0x376,
        }
    }

    fn bmide_base(&self) -> Option<u16> {
        // BMIDE is at PCI BAR4 + 0 for primary, + 8 for secondary
        // We'll detect this from PCI during init
        None // filled in during init if detected
    }
}

/// ATA error information
#[derive(Debug, Clone)]
pub struct AtaError {
    pub status: u8,
    pub error: u8,
    pub description: &'static str,
}

impl AtaError {
    fn from_regs(base: u16) -> Self {
        let status = inb(base + ATA_STATUS);
        let error = inb(base + ATA_ERROR);
        let desc = if error & ATA_ERR_ABRT != 0 {
            "command aborted"
        } else if error & ATA_ERR_IDNF != 0 {
            "sector not found"
        } else if error & ATA_ERR_UNC != 0 {
            "uncorrectable data error"
        } else if error & ATA_ERR_BBK != 0 {
            "bad block"
        } else if error & ATA_ERR_AMNF != 0 {
            "address mark not found"
        } else if status & ATA_SR_DF != 0 {
            "drive fault"
        } else {
            "unknown error"
        };
        AtaError {
            status,
            error,
            description: desc,
        }
    }
}

/// Global list of detected drives
static ATA_DRIVES: Mutex<Vec<AtaDrive>> = Mutex::new(Vec::new());

/// Bus Master IDE base address (from PCI)
static BMIDE_BASE: Mutex<Option<u16>> = Mutex::new(None);

/// Wait for BSY to clear with timeout
fn wait_bsy(base: u16) -> bool {
    for _ in 0..500_000u32 {
        if inb(base + ATA_STATUS) & ATA_SR_BSY == 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Wait for DRQ (data ready) with timeout
fn wait_drq(base: u16) -> Result<(), AtaError> {
    for _ in 0..500_000u32 {
        let status = inb(base + ATA_STATUS);
        if status & ATA_SR_ERR != 0 {
            return Err(AtaError::from_regs(base));
        }
        if status & ATA_SR_DF != 0 {
            return Err(AtaError::from_regs(base));
        }
        if status & ATA_SR_DRQ != 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err(AtaError {
        status: inb(base + ATA_STATUS),
        error: 0,
        description: "DRQ timeout",
    })
}

/// Wait for drive ready (BSY clear and DRDY set)
fn wait_ready(base: u16) -> bool {
    for _ in 0..500_000u32 {
        let status = inb(base + ATA_STATUS);
        if status & ATA_SR_BSY == 0 && status & ATA_SR_DRDY != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// 400ns delay by reading the alternate status register 4 times
fn io_delay(ctrl_port: u16) {
    for _ in 0..4 {
        inb(ctrl_port);
    }
}

/// Select a drive on a channel
fn select_drive(base: u16, ctrl_port: u16, drive: u8) {
    outb(base + ATA_DRIVE_HEAD, 0xA0 | ((drive & 1) << 4));
    io_delay(ctrl_port);
}

/// Software reset a channel (both drives)
fn soft_reset(ctrl_port: u16) {
    // Set SRST bit (bit 2) in device control register
    outb(ctrl_port, 0x04);
    // Wait at least 5 microseconds
    io_delay(ctrl_port);
    io_delay(ctrl_port);
    // Clear SRST bit, disable nIEN (enable interrupts)
    outb(ctrl_port, 0x00);
    // Wait for BSY to clear
    for _ in 0..500_000u32 {
        let status = inb(ctrl_port);
        if status & ATA_SR_BSY == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

/// Parse an ATA string from identify data (byte-swapped)
fn parse_ata_string(identify: &[u16], start_word: usize, num_words: usize) -> String {
    let total_bytes = num_words * 2;
    let mut bytes = Vec::with_capacity(total_bytes);
    for i in 0..num_words {
        if start_word + i >= identify.len() {
            break;
        }
        let word = identify[start_word + i];
        bytes.push((word >> 8) as u8);
        bytes.push((word & 0xFF) as u8);
    }
    String::from_utf8_lossy(&bytes).trim().into()
}

/// Identify a drive (returns model string and sector count)
fn identify_drive(channel: AtaChannel, drive: u8) -> Option<AtaDrive> {
    let base = channel.base_port();
    let ctrl = channel.control_port();

    select_drive(base, ctrl, drive);

    // Clear sector count and LBA registers
    outb(base + ATA_SECT_COUNT, 0);
    outb(base + ATA_LBA_LO, 0);
    outb(base + ATA_LBA_MID, 0);
    outb(base + ATA_LBA_HI, 0);

    // Send IDENTIFY command
    outb(base + ATA_COMMAND, ATA_CMD_IDENTIFY);

    // Check if drive exists
    io_delay(ctrl);
    let status = inb(base + ATA_STATUS);
    if status == 0 {
        return None; // no drive
    }

    // Wait for BSY to clear
    if !wait_bsy(base) {
        return None;
    }

    // Check for ATAPI signature
    let mid = inb(base + ATA_LBA_MID);
    let hi = inb(base + ATA_LBA_HI);

    let mut is_atapi = false;
    if mid != 0 || hi != 0 {
        // Could be ATAPI (0x14, 0xEB) or SATA (0x3C, 0xC3) or (0x69, 0x96)
        if mid == 0x14 && hi == 0xEB {
            is_atapi = true;
            // Send IDENTIFY PACKET DEVICE instead
            outb(base + ATA_COMMAND, ATA_CMD_IDENTIFY_PACKET);
            if !wait_bsy(base) {
                return None;
            }
        } else {
            return None; // unknown device type
        }
    }

    // Wait for DRQ or ERR
    if wait_drq(base).is_err() {
        return None;
    }

    // Read 256 words of identify data
    let mut identify = [0u16; 256];
    for i in 0..256 {
        identify[i] = inw(base + ATA_DATA);
    }

    // Extract model string (words 27-46)
    let model = parse_ata_string(&identify, 27, 20);

    // Extract serial (words 10-19)
    let serial = parse_ata_string(&identify, 10, 10);

    // Extract firmware revision (words 23-26)
    let firmware = parse_ata_string(&identify, 23, 4);

    // LBA48 support (word 83 bit 10)
    let lba48 = identify[83] & (1 << 10) != 0;

    // Sector count
    let sectors = if lba48 {
        (identify[100] as u64)
            | ((identify[101] as u64) << 16)
            | ((identify[102] as u64) << 32)
            | ((identify[103] as u64) << 48)
    } else {
        (identify[60] as u64) | ((identify[61] as u64) << 16)
    };

    // DMA support (word 49 bit 8)
    let dma_supported = identify[49] & (1 << 8) != 0;

    // UDMA mode (word 88)
    let udma_modes = identify[88];
    let mut udma_mode = 0u8;
    for i in (0..7).rev() {
        if udma_modes & (1 << (i + 8)) != 0 {
            // Active mode
            udma_mode = i + 1;
            break;
        }
    }

    // Maximum sectors per transfer (word 47 bits 7:0)
    let max_sectors = (identify[47] & 0xFF) as u16;
    let max_sectors_per_transfer = if max_sectors == 0 { 1 } else { max_sectors };

    Some(AtaDrive {
        channel,
        drive,
        present: true,
        atapi: is_atapi,
        model,
        serial,
        firmware,
        sectors,
        lba48,
        dma_supported,
        udma_mode,
        max_sectors_per_transfer,
    })
}

/// Send LBA48 parameters to the drive
fn setup_lba48(base: u16, drive: u8, lba: u64, count: u16) {
    // LBA48: write high bytes first, then low bytes
    // High bytes of sector count and LBA
    outb(base + ATA_SECT_COUNT, ((count >> 8) & 0xFF) as u8);
    outb(base + ATA_LBA_LO, ((lba >> 24) & 0xFF) as u8);
    outb(base + ATA_LBA_MID, ((lba >> 32) & 0xFF) as u8);
    outb(base + ATA_LBA_HI, ((lba >> 40) & 0xFF) as u8);

    // Low bytes of sector count and LBA
    outb(base + ATA_SECT_COUNT, (count & 0xFF) as u8);
    outb(base + ATA_LBA_LO, (lba & 0xFF) as u8);
    outb(base + ATA_LBA_MID, ((lba >> 8) & 0xFF) as u8);
    outb(base + ATA_LBA_HI, ((lba >> 16) & 0xFF) as u8);

    // Drive/Head: LBA mode
    outb(base + ATA_DRIVE_HEAD, 0xE0 | ((drive & 1) << 4));
}

/// Send LBA28 parameters to the drive
fn setup_lba28(base: u16, drive: u8, lba: u64, count: u8) {
    outb(
        base + ATA_DRIVE_HEAD,
        0xE0 | ((drive & 1) << 4) | ((lba >> 24) as u8 & 0x0F),
    );
    outb(base + ATA_SECT_COUNT, count);
    outb(base + ATA_LBA_LO, (lba & 0xFF) as u8);
    outb(base + ATA_LBA_MID, ((lba >> 8) & 0xFF) as u8);
    outb(base + ATA_LBA_HI, ((lba >> 16) & 0xFF) as u8);
}

/// Read sectors using LBA48 PIO
fn read_sectors_lba48(
    base: u16,
    ctrl: u16,
    drive: u8,
    lba: u64,
    count: u16,
    buf: &mut [u8],
) -> Result<(), &'static str> {
    if !wait_bsy(base) {
        return Err("drive busy timeout");
    }

    setup_lba48(base, drive, lba, count);
    outb(base + ATA_COMMAND, ATA_CMD_READ_PIO_EXT);

    io_delay(ctrl);

    for sector in 0..count as usize {
        wait_drq(base).map_err(|e| e.description)?;

        let offset = sector * SECTOR_SIZE;
        for i in 0..256 {
            let word = inw(base + ATA_DATA);
            if offset + i * 2 + 1 < buf.len() {
                buf[offset + i * 2] = (word & 0xFF) as u8;
                buf[offset + i * 2 + 1] = (word >> 8) as u8;
            }
        }
    }

    Ok(())
}

/// Write sectors using LBA48 PIO
fn write_sectors_lba48(
    base: u16,
    ctrl: u16,
    drive: u8,
    lba: u64,
    count: u16,
    data: &[u8],
) -> Result<(), &'static str> {
    if !wait_bsy(base) {
        return Err("drive busy timeout");
    }

    setup_lba48(base, drive, lba, count);
    outb(base + ATA_COMMAND, ATA_CMD_WRITE_PIO_EXT);

    io_delay(ctrl);

    for sector in 0..count as usize {
        wait_drq(base).map_err(|e| e.description)?;

        let offset = sector * SECTOR_SIZE;
        for i in 0..256 {
            let word = if offset + i * 2 + 1 < data.len() {
                (data[offset + i * 2] as u16) | ((data[offset + i * 2 + 1] as u16) << 8)
            } else {
                0
            };
            crate::io::outw(base + ATA_DATA, word);
        }
    }

    // Flush cache
    outb(base + ATA_COMMAND, ATA_CMD_CACHE_FLUSH_EXT);
    if !wait_bsy(base) {
        return Err("flush timeout");
    }

    Ok(())
}

/// Send an ATAPI PACKET command
fn atapi_send_packet(
    base: u16,
    ctrl: u16,
    drive: u8,
    packet: &[u8; 12],
    buf: &mut [u8],
) -> Result<usize, &'static str> {
    select_drive(base, ctrl, drive);

    if !wait_bsy(base) {
        return Err("drive busy");
    }

    // Set up for ATAPI command
    outb(base + ATA_FEATURES, 0); // PIO mode
    outb(base + ATA_LBA_MID, (buf.len() & 0xFF) as u8); // byte count low
    outb(base + ATA_LBA_HI, ((buf.len() >> 8) & 0xFF) as u8); // byte count high
    outb(base + ATA_COMMAND, ATA_CMD_PACKET);

    // Wait for DRQ to send the packet
    wait_drq(base).map_err(|e| e.description)?;

    // Send the 12-byte command packet as 6 words
    for i in 0..6 {
        let word = (packet[i * 2] as u16) | ((packet[i * 2 + 1] as u16) << 8);
        crate::io::outw(base + ATA_DATA, word);
    }

    io_delay(ctrl);

    // Wait for response data
    if !wait_bsy(base) {
        return Err("ATAPI command timeout");
    }

    let status = inb(base + ATA_STATUS);
    if status & ATA_SR_ERR != 0 {
        return Err("ATAPI command error");
    }

    if status & ATA_SR_DRQ == 0 {
        return Ok(0); // no data to read
    }

    // Read the response data
    let byte_count_lo = inb(base + ATA_LBA_MID) as usize;
    let byte_count_hi = inb(base + ATA_LBA_HI) as usize;
    let byte_count = byte_count_lo | (byte_count_hi << 8); // max 0xFFFF
                                                           // Saturate: clamp to buf.len() before dividing, prevent unbounded reads
    let safe_byte_count = byte_count.min(buf.len());
    let word_count = (safe_byte_count.saturating_add(1)) / 2;

    let mut total = 0usize;
    for _i in 0..word_count {
        let word = inw(base + ATA_DATA);
        if total < buf.len() {
            buf[total] = (word & 0xFF) as u8;
            total += 1;
        }
        if total < buf.len() {
            buf[total] = (word >> 8) as u8;
            total += 1;
        }
    }

    Ok(total.min(byte_count))
}

/// Read capacity from an ATAPI device
fn atapi_read_capacity(base: u16, ctrl: u16, drive: u8) -> Option<(u32, u32)> {
    let mut packet = [0u8; 12];
    packet[0] = ATAPI_CMD_READ_CAPACITY;

    let mut buf = [0u8; 8];
    match atapi_send_packet(base, ctrl, drive, &packet, &mut buf) {
        Ok(n) if n >= 8 => {
            let last_lba = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let block_size = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
            Some((last_lba + 1, block_size))
        }
        _ => None,
    }
}

/// Read sectors from an ATA drive (28-bit LBA PIO)
///
/// Reads `count` sectors starting at `lba` into `buf`.
/// `buf` must be at least `count * 512` bytes.
pub fn read_sectors(
    drive_idx: usize,
    lba: u64,
    count: u8,
    buf: &mut [u8],
) -> Result<(), &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.present {
        return Err("drive not present");
    }
    if drive.atapi {
        return Err("use atapi_read for ATAPI devices");
    }
    if buf.len() < (count as usize) * SECTOR_SIZE {
        return Err("buffer too small");
    }

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();
    let drive_bit = drive.drive;

    // Use LBA48 if the drive supports it and the address requires it
    if drive.lba48 && (lba > 0x0FFFFFFF || count == 0) {
        return read_sectors_lba48(base, ctrl, drive_bit, lba, count as u16, buf);
    }

    // LBA28 path
    if !wait_bsy(base) {
        return Err("drive busy timeout");
    }

    setup_lba28(base, drive_bit, lba, count);
    outb(base + ATA_COMMAND, ATA_CMD_READ_PIO);

    io_delay(ctrl);

    for sector in 0..count as usize {
        wait_drq(base).map_err(|e| e.description)?;

        let offset = sector.saturating_mul(SECTOR_SIZE);
        for i in 0..256usize {
            let word = inw(base + ATA_DATA);
            let byte_lo = offset.saturating_add(i.saturating_mul(2));
            let byte_hi = byte_lo.saturating_add(1);
            if byte_hi < buf.len() {
                buf[byte_lo] = (word & 0xFF) as u8;
                buf[byte_hi] = (word >> 8) as u8;
            }
        }
    }

    Ok(())
}

/// Read sectors using LBA48 addressing (for large drives or large transfers)
pub fn read_sectors_ext(
    drive_idx: usize,
    lba: u64,
    count: u16,
    buf: &mut [u8],
) -> Result<(), &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.present {
        return Err("drive not present");
    }
    if !drive.lba48 {
        return Err("drive does not support LBA48");
    }
    if buf.len() < (count as usize) * SECTOR_SIZE {
        return Err("buffer too small");
    }

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();
    read_sectors_lba48(base, ctrl, drive.drive, lba, count, buf)
}

/// Write sectors to an ATA drive (28-bit LBA PIO)
pub fn write_sectors(
    drive_idx: usize,
    lba: u64,
    count: u8,
    data: &[u8],
) -> Result<(), &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.present {
        return Err("drive not present");
    }
    if drive.atapi {
        return Err("write not supported on ATAPI");
    }
    if data.len() < (count as usize) * SECTOR_SIZE {
        return Err("data too small");
    }

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();
    let drive_bit = drive.drive;

    if drive.lba48 && (lba > 0x0FFFFFFF || count == 0) {
        return write_sectors_lba48(base, ctrl, drive_bit, lba, count as u16, data);
    }

    if !wait_bsy(base) {
        return Err("drive busy timeout");
    }

    setup_lba28(base, drive_bit, lba, count);
    outb(base + ATA_COMMAND, ATA_CMD_WRITE_PIO);

    io_delay(ctrl);

    for sector in 0..count as usize {
        wait_drq(base).map_err(|e| e.description)?;

        let offset = sector.saturating_mul(SECTOR_SIZE);
        for i in 0..256usize {
            let byte_lo = offset.saturating_add(i.saturating_mul(2));
            let byte_hi = byte_lo.saturating_add(1);
            let word = if byte_hi < data.len() {
                (data[byte_lo] as u16) | ((data[byte_hi] as u16) << 8)
            } else {
                0
            };
            crate::io::outw(base + ATA_DATA, word);
        }
    }

    // Flush cache
    outb(base + ATA_COMMAND, ATA_CMD_CACHE_FLUSH);
    if !wait_bsy(base) {
        return Err("flush cache timeout");
    }

    Ok(())
}

/// Write sectors using LBA48 addressing
pub fn write_sectors_ext(
    drive_idx: usize,
    lba: u64,
    count: u16,
    data: &[u8],
) -> Result<(), &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.present {
        return Err("drive not present");
    }
    if !drive.lba48 {
        return Err("drive does not support LBA48");
    }
    if data.len() < (count as usize) * SECTOR_SIZE {
        return Err("data too small");
    }

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();
    write_sectors_lba48(base, ctrl, drive.drive, lba, count, data)
}

/// Read from an ATAPI device (CD-ROM)
pub fn atapi_read(
    drive_idx: usize,
    lba: u32,
    count: u32,
    buf: &mut [u8],
) -> Result<usize, &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.present {
        return Err("drive not present");
    }
    if !drive.atapi {
        return Err("not an ATAPI device");
    }

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();

    // Build ATAPI READ (12) command
    let mut packet = [0u8; 12];
    packet[0] = ATAPI_CMD_READ;
    packet[2] = ((lba >> 24) & 0xFF) as u8;
    packet[3] = ((lba >> 16) & 0xFF) as u8;
    packet[4] = ((lba >> 8) & 0xFF) as u8;
    packet[5] = (lba & 0xFF) as u8;
    packet[6] = ((count >> 24) & 0xFF) as u8;
    packet[7] = ((count >> 16) & 0xFF) as u8;
    packet[8] = ((count >> 8) & 0xFF) as u8;
    packet[9] = (count & 0xFF) as u8;

    atapi_send_packet(base, ctrl, drive.drive, &packet, buf)
}

/// Eject ATAPI media
pub fn atapi_eject(drive_idx: usize) -> Result<(), &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.atapi {
        return Err("not an ATAPI device");
    }

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();

    let mut packet = [0u8; 12];
    packet[0] = ATAPI_CMD_EJECT;
    packet[4] = 0x02; // eject

    let mut buf = [0u8; 0];
    atapi_send_packet(base, ctrl, drive.drive, &packet, &mut buf)?;
    Ok(())
}

/// Get list of detected drives
pub fn drives() -> Vec<AtaDrive> {
    ATA_DRIVES.lock().clone()
}

/// Get drive count
pub fn drive_count() -> usize {
    ATA_DRIVES.lock().len()
}

/// Get a specific drive's info
pub fn drive_info(idx: usize) -> Option<AtaDrive> {
    ATA_DRIVES.lock().get(idx).cloned()
}

/// DMA transfer state for managing in-progress DMA operations
struct DmaState {
    prdt_phys: usize,   // Physical address of the PRDT
    buffer_phys: usize, // Physical address of the DMA buffer
    active: bool,
}

static DMA_STATE: Mutex<Option<DmaState>> = Mutex::new(None);

/// Set up the PRDT (Physical Region Descriptor Table) for a DMA transfer
fn setup_prdt(
    bmide_base: u16,
    buffer_phys: usize,
    byte_count: u32,
    is_primary: bool,
) -> Result<usize, &'static str> {
    let prdt_frame = frame_allocator::allocate_frame().ok_or("out of memory for PRDT")?;
    unsafe {
        core::ptr::write_bytes(prdt_frame.addr as *mut u8, 0, FRAME_SIZE);
    }

    // Build a single PRDT entry pointing to our buffer
    // For simplicity, one entry per transfer (max 64KB per entry)
    let entry_addr = prdt_frame.addr;
    let phys_addr = buffer_phys as u32;
    let count = if byte_count == 0 || byte_count > 0x10000 {
        0u16
    } else {
        byte_count as u16
    };
    let flags: u16 = 0x8000; // End of table bit

    unsafe {
        // PrdtEntry is packed: phys_addr(4) + byte_count(2) + flags(2) = 8 bytes
        core::ptr::write_volatile(entry_addr as *mut u32, phys_addr);
        core::ptr::write_volatile((entry_addr + 4) as *mut u16, count);
        core::ptr::write_volatile((entry_addr + 6) as *mut u16, flags);
    }

    // Write PRDT address to BMIDE register
    let bmide_offset = if is_primary { 0 } else { 8 };
    let prdt_port = bmide_base + bmide_offset + BMIDE_PRDT;

    // PRDT address register is 32-bit
    crate::io::outl(prdt_port, prdt_frame.addr as u32);

    Ok(prdt_frame.addr)
}

/// Read sectors using DMA transfer (28-bit LBA)
fn read_sectors_dma28(
    base: u16,
    _ctrl: u16,
    drive: u8,
    lba: u64,
    count: u8,
    bmide_base: u16,
    is_primary: bool,
    buf: &mut [u8],
) -> Result<(), &'static str> {
    let byte_count = (count as u32) * SECTOR_SIZE as u32;

    // Allocate DMA buffer
    let dma_frame = frame_allocator::allocate_frame().ok_or("out of memory for DMA")?;
    unsafe {
        core::ptr::write_bytes(dma_frame.addr as *mut u8, 0, FRAME_SIZE);
    }

    // Setup PRDT
    let _prdt_addr = setup_prdt(bmide_base, dma_frame.addr, byte_count, is_primary)?;

    // Stop any previous DMA
    let bmide_offset = if is_primary { 0 } else { 8 };
    let cmd_port = bmide_base + bmide_offset + BMIDE_COMMAND;
    let status_port = bmide_base + bmide_offset + BMIDE_STATUS;
    crate::io::outb(cmd_port, 0);

    // Clear error and interrupt bits in BMIDE status
    let old_status = crate::io::inb(status_port);
    crate::io::outb(status_port, old_status | BMIDE_STS_ERR | BMIDE_STS_IRQ);

    // Wait for drive ready
    if !wait_bsy(base) {
        return Err("drive busy before DMA read");
    }

    // Set up LBA28 parameters
    setup_lba28(base, drive, lba, count);

    // Issue READ DMA command
    outb(base + ATA_COMMAND, ATA_CMD_READ_DMA);

    // Start DMA transfer (read = device to memory)
    crate::io::outb(cmd_port, BMIDE_CMD_START | BMIDE_CMD_READ);

    // Wait for DMA completion
    for _ in 0..1_000_000u32 {
        let bm_status = crate::io::inb(status_port);
        if bm_status & BMIDE_STS_IRQ != 0 {
            // DMA completed, check for errors
            if bm_status & BMIDE_STS_ERR != 0 {
                crate::io::outb(cmd_port, 0); // stop DMA
                crate::io::outb(status_port, BMIDE_STS_ERR | BMIDE_STS_IRQ);
                return Err("DMA transfer error");
            }
            // Stop DMA engine
            crate::io::outb(cmd_port, 0);
            // Clear interrupt
            crate::io::outb(status_port, BMIDE_STS_IRQ);

            // Copy data from DMA buffer to user buffer
            let copy_len = byte_count as usize;
            if copy_len <= buf.len() {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        dma_frame.addr as *const u8,
                        buf.as_mut_ptr(),
                        copy_len,
                    );
                }
            }
            return Ok(());
        }
        if bm_status & BMIDE_STS_ACTIVE == 0 {
            // DMA stopped but no IRQ -- check ATA status
            let ata_status = inb(base + ATA_STATUS);
            if ata_status & ATA_SR_ERR != 0 {
                crate::io::outb(cmd_port, 0);
                return Err("ATA error during DMA read");
            }
        }
        core::hint::spin_loop();
    }

    crate::io::outb(cmd_port, 0); // stop DMA on timeout
    Err("DMA read timeout")
}

/// Write sectors using DMA transfer (28-bit LBA)
fn write_sectors_dma28(
    base: u16,
    _ctrl: u16,
    drive: u8,
    lba: u64,
    count: u8,
    bmide_base: u16,
    is_primary: bool,
    data: &[u8],
) -> Result<(), &'static str> {
    let byte_count = (count as u32) * SECTOR_SIZE as u32;

    // Allocate DMA buffer and copy data into it
    let dma_frame = frame_allocator::allocate_frame().ok_or("out of memory for DMA")?;
    let copy_len = (byte_count as usize).min(data.len()).min(FRAME_SIZE);
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), dma_frame.addr as *mut u8, copy_len);
    }

    // Setup PRDT
    let _prdt_addr = setup_prdt(bmide_base, dma_frame.addr, byte_count, is_primary)?;

    // Stop any previous DMA
    let bmide_offset = if is_primary { 0 } else { 8 };
    let cmd_port = bmide_base + bmide_offset + BMIDE_COMMAND;
    let status_port = bmide_base + bmide_offset + BMIDE_STATUS;
    crate::io::outb(cmd_port, 0);

    // Clear error and interrupt bits
    let old_status = crate::io::inb(status_port);
    crate::io::outb(status_port, old_status | BMIDE_STS_ERR | BMIDE_STS_IRQ);

    if !wait_bsy(base) {
        return Err("drive busy before DMA write");
    }

    // Set up LBA28 parameters
    setup_lba28(base, drive, lba, count);

    // Issue WRITE DMA command
    outb(base + ATA_COMMAND, ATA_CMD_WRITE_DMA);

    // Start DMA transfer (write = memory to device, no READ bit)
    crate::io::outb(cmd_port, BMIDE_CMD_START);

    // Wait for DMA completion
    for _ in 0..1_000_000u32 {
        let bm_status = crate::io::inb(status_port);
        if bm_status & BMIDE_STS_IRQ != 0 {
            if bm_status & BMIDE_STS_ERR != 0 {
                crate::io::outb(cmd_port, 0);
                crate::io::outb(status_port, BMIDE_STS_ERR | BMIDE_STS_IRQ);
                return Err("DMA write transfer error");
            }
            crate::io::outb(cmd_port, 0);
            crate::io::outb(status_port, BMIDE_STS_IRQ);

            // Flush cache after write
            outb(base + ATA_COMMAND, ATA_CMD_CACHE_FLUSH);
            if !wait_bsy(base) {
                return Err("flush timeout after DMA write");
            }
            return Ok(());
        }
        core::hint::spin_loop();
    }

    crate::io::outb(cmd_port, 0);
    Err("DMA write timeout")
}

/// Read sectors using DMA with LBA48 addressing
fn read_sectors_dma48(
    base: u16,
    _ctrl: u16,
    drive: u8,
    lba: u64,
    count: u16,
    bmide_base: u16,
    is_primary: bool,
    buf: &mut [u8],
) -> Result<(), &'static str> {
    let byte_count = (count as u32) * SECTOR_SIZE as u32;

    let dma_frame = frame_allocator::allocate_frame().ok_or("out of memory for DMA")?;
    unsafe {
        core::ptr::write_bytes(dma_frame.addr as *mut u8, 0, FRAME_SIZE);
    }

    let _prdt_addr = setup_prdt(bmide_base, dma_frame.addr, byte_count, is_primary)?;

    let bmide_offset = if is_primary { 0 } else { 8 };
    let cmd_port = bmide_base + bmide_offset + BMIDE_COMMAND;
    let status_port = bmide_base + bmide_offset + BMIDE_STATUS;
    crate::io::outb(cmd_port, 0);

    let old_status = crate::io::inb(status_port);
    crate::io::outb(status_port, old_status | BMIDE_STS_ERR | BMIDE_STS_IRQ);

    if !wait_bsy(base) {
        return Err("drive busy before DMA48 read");
    }

    setup_lba48(base, drive, lba, count);
    outb(base + ATA_COMMAND, ATA_CMD_READ_DMA_EXT);
    crate::io::outb(cmd_port, BMIDE_CMD_START | BMIDE_CMD_READ);

    for _ in 0..1_000_000u32 {
        let bm_status = crate::io::inb(status_port);
        if bm_status & BMIDE_STS_IRQ != 0 {
            if bm_status & BMIDE_STS_ERR != 0 {
                crate::io::outb(cmd_port, 0);
                crate::io::outb(status_port, BMIDE_STS_ERR | BMIDE_STS_IRQ);
                return Err("DMA48 transfer error");
            }
            crate::io::outb(cmd_port, 0);
            crate::io::outb(status_port, BMIDE_STS_IRQ);

            let copy_len = (byte_count as usize).min(buf.len());
            unsafe {
                core::ptr::copy_nonoverlapping(
                    dma_frame.addr as *const u8,
                    buf.as_mut_ptr(),
                    copy_len,
                );
            }
            return Ok(());
        }
        core::hint::spin_loop();
    }

    crate::io::outb(cmd_port, 0);
    Err("DMA48 read timeout")
}

/// Write sectors using DMA with LBA48 addressing
fn write_sectors_dma48(
    base: u16,
    _ctrl: u16,
    drive: u8,
    lba: u64,
    count: u16,
    bmide_base: u16,
    is_primary: bool,
    data: &[u8],
) -> Result<(), &'static str> {
    let byte_count = (count as u32) * SECTOR_SIZE as u32;

    let dma_frame = frame_allocator::allocate_frame().ok_or("out of memory for DMA")?;
    let copy_len = (byte_count as usize).min(data.len()).min(FRAME_SIZE);
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), dma_frame.addr as *mut u8, copy_len);
    }

    let _prdt_addr = setup_prdt(bmide_base, dma_frame.addr, byte_count, is_primary)?;

    let bmide_offset = if is_primary { 0 } else { 8 };
    let cmd_port = bmide_base + bmide_offset + BMIDE_COMMAND;
    let status_port = bmide_base + bmide_offset + BMIDE_STATUS;
    crate::io::outb(cmd_port, 0);

    let old_status = crate::io::inb(status_port);
    crate::io::outb(status_port, old_status | BMIDE_STS_ERR | BMIDE_STS_IRQ);

    if !wait_bsy(base) {
        return Err("drive busy before DMA48 write");
    }

    setup_lba48(base, drive, lba, count);
    outb(base + ATA_COMMAND, ATA_CMD_WRITE_DMA_EXT);
    crate::io::outb(cmd_port, BMIDE_CMD_START);

    for _ in 0..1_000_000u32 {
        let bm_status = crate::io::inb(status_port);
        if bm_status & BMIDE_STS_IRQ != 0 {
            if bm_status & BMIDE_STS_ERR != 0 {
                crate::io::outb(cmd_port, 0);
                crate::io::outb(status_port, BMIDE_STS_ERR | BMIDE_STS_IRQ);
                return Err("DMA48 write transfer error");
            }
            crate::io::outb(cmd_port, 0);
            crate::io::outb(status_port, BMIDE_STS_IRQ);

            outb(base + ATA_COMMAND, ATA_CMD_CACHE_FLUSH_EXT);
            if !wait_bsy(base) {
                return Err("flush timeout after DMA48 write");
            }
            return Ok(());
        }
        core::hint::spin_loop();
    }

    crate::io::outb(cmd_port, 0);
    Err("DMA48 write timeout")
}

/// Public interface: Read sectors using DMA (auto-selects LBA28 or LBA48)
/// Falls back to PIO if BMIDE is not available
pub fn read_sectors_dma(
    drive_idx: usize,
    lba: u64,
    count: u16,
    buf: &mut [u8],
) -> Result<(), &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.present {
        return Err("drive not present");
    }
    if drive.atapi {
        return Err("DMA read not supported on ATAPI via this path");
    }
    if !drive.dma_supported {
        return Err("drive does not support DMA");
    }
    if buf.len() < (count as usize) * SECTOR_SIZE {
        return Err("buffer too small");
    }

    let bmide = BMIDE_BASE.lock();
    let bmide_base = match *bmide {
        Some(b) => b,
        None => return Err("BMIDE controller not available"),
    };

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();
    let is_primary = drive.channel == AtaChannel::Primary;
    let drive_bit = drive.drive;

    // Transfer in chunks that fit in one 4K frame
    let sectors_per_frame = (FRAME_SIZE / SECTOR_SIZE) as u16;
    let mut remaining = count;
    let mut current_lba = lba;
    let mut buf_offset = 0usize;

    while remaining > 0 {
        let batch = remaining.min(sectors_per_frame);

        if drive.lba48 && (current_lba > 0x0FFFFFFF || batch > 255) {
            read_sectors_dma48(
                base,
                ctrl,
                drive_bit,
                current_lba,
                batch,
                bmide_base,
                is_primary,
                &mut buf[buf_offset..],
            )?;
        } else {
            read_sectors_dma28(
                base,
                ctrl,
                drive_bit,
                current_lba,
                batch as u8,
                bmide_base,
                is_primary,
                &mut buf[buf_offset..],
            )?;
        }

        remaining = remaining.saturating_sub(batch);
        current_lba = current_lba.saturating_add(batch as u64);
        buf_offset = buf_offset.saturating_add((batch as usize).saturating_mul(SECTOR_SIZE));
    }

    Ok(())
}

/// Public interface: Write sectors using DMA (auto-selects LBA28 or LBA48)
pub fn write_sectors_dma(
    drive_idx: usize,
    lba: u64,
    count: u16,
    data: &[u8],
) -> Result<(), &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.present {
        return Err("drive not present");
    }
    if drive.atapi {
        return Err("DMA write not supported on ATAPI");
    }
    if !drive.dma_supported {
        return Err("drive does not support DMA");
    }
    if data.len() < (count as usize) * SECTOR_SIZE {
        return Err("data too small");
    }

    let bmide = BMIDE_BASE.lock();
    let bmide_base = match *bmide {
        Some(b) => b,
        None => return Err("BMIDE controller not available"),
    };

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();
    let is_primary = drive.channel == AtaChannel::Primary;
    let drive_bit = drive.drive;

    let sectors_per_frame = (FRAME_SIZE / SECTOR_SIZE) as u16;
    let mut remaining = count;
    let mut current_lba = lba;
    let mut data_offset = 0usize;

    while remaining > 0 {
        let batch = remaining.min(sectors_per_frame);

        if drive.lba48 && (current_lba > 0x0FFFFFFF || batch > 255) {
            write_sectors_dma48(
                base,
                ctrl,
                drive_bit,
                current_lba,
                batch,
                bmide_base,
                is_primary,
                &data[data_offset..],
            )?;
        } else {
            write_sectors_dma28(
                base,
                ctrl,
                drive_bit,
                current_lba,
                batch as u8,
                bmide_base,
                is_primary,
                &data[data_offset..],
            )?;
        }

        remaining = remaining.saturating_sub(batch);
        current_lba = current_lba.saturating_add(batch as u64);
        data_offset = data_offset.saturating_add((batch as usize).saturating_mul(SECTOR_SIZE));
    }

    Ok(())
}

/// Check if DMA is available for a drive
pub fn is_dma_available(drive_idx: usize) -> bool {
    let drives = ATA_DRIVES.lock();
    let bmide = BMIDE_BASE.lock();
    if bmide.is_none() {
        return false;
    }
    drives
        .get(drive_idx)
        .map(|d| d.present && d.dma_supported)
        .unwrap_or(false)
}

/// ATAPI: Send a SCSI-style command with optional DMA
pub fn atapi_test_unit_ready(drive_idx: usize) -> Result<(), &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.atapi {
        return Err("not an ATAPI device");
    }

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();

    let mut packet = [0u8; 12];
    packet[0] = ATAPI_CMD_TEST_UNIT_READY;

    let mut buf = [0u8; 0];
    atapi_send_packet(base, ctrl, drive.drive, &packet, &mut buf)?;
    Ok(())
}

/// ATAPI: Request Sense data (error information)
pub fn atapi_request_sense(drive_idx: usize) -> Result<[u8; 18], &'static str> {
    let drives = ATA_DRIVES.lock();
    let drive = drives.get(drive_idx).ok_or("invalid drive index")?;
    if !drive.atapi {
        return Err("not an ATAPI device");
    }

    let base = drive.channel.base_port();
    let ctrl = drive.channel.control_port();

    let mut packet = [0u8; 12];
    packet[0] = ATAPI_CMD_REQUEST_SENSE;
    packet[4] = 18; // allocation length

    let mut buf = [0u8; 18];
    atapi_send_packet(base, ctrl, drive.drive, &packet, &mut buf)?;
    Ok(buf)
}

/// Detect Bus Master IDE controller from PCI
fn detect_bmide() -> Option<u16> {
    // Look for IDE controller (class 01, subclass 01) on PCI
    let devices = crate::drivers::pci::find_by_class(0x01, 0x01);
    for dev in &devices {
        let (bar4, is_io) = crate::drivers::pci::read_bar(dev.bus, dev.device, dev.function, 4);
        if !is_io || bar4 == 0 {
            continue;
        }
        let bmide_base = (bar4 & 0xFFFC) as u16;
        serial_println!("  ATA: Bus Master IDE at {:#x}", bmide_base);

        // Enable bus mastering on the IDE controller
        crate::drivers::pci::enable_bus_master(dev.bus, dev.device, dev.function);

        return Some(bmide_base);
    }
    None
}

/// Initialize ATA driver — probe both channels for drives
pub fn init() {
    // Detect BMIDE controller
    *BMIDE_BASE.lock() = detect_bmide();

    let mut found = Vec::new();

    // Probe primary master/slave
    for drive in 0..2u8 {
        if let Some(mut ata) = identify_drive(AtaChannel::Primary, drive) {
            // Divide first to avoid overflow: sectors / 2048 gives MiB for 512B sectors
            let size_mb = ata.sectors / 2048;
            let dev_type = if ata.atapi { "ATAPI" } else { "ATA" };
            serial_println!(
                "  ATA: {} {} [{}] \"{}\" {}MB, LBA48={}, DMA={}, UDMA{}",
                if drive == 0 {
                    "Primary Master"
                } else {
                    "Primary Slave"
                },
                dev_type,
                ata.serial,
                ata.model,
                size_mb,
                ata.lba48,
                ata.dma_supported,
                ata.udma_mode
            );

            // For ATAPI, try to get capacity
            if ata.atapi {
                let base = ata.channel.base_port();
                let ctrl = ata.channel.control_port();
                if let Some((blocks, block_size)) = atapi_read_capacity(base, ctrl, drive) {
                    let cap_mb = (blocks as u64) * (block_size as u64) / (1024 * 1024);
                    serial_println!(
                        "    Capacity: {} blocks x {}B = {}MB",
                        blocks,
                        block_size,
                        cap_mb
                    );
                    ata.sectors = blocks as u64;
                }
            }

            found.push(ata);
        }
    }

    // Probe secondary master/slave
    for drive in 0..2u8 {
        if let Some(mut ata) = identify_drive(AtaChannel::Secondary, drive) {
            // Divide first to avoid overflow: sectors / 2048 gives MiB for 512B sectors
            let size_mb = ata.sectors / 2048;
            let dev_type = if ata.atapi { "ATAPI" } else { "ATA" };
            serial_println!(
                "  ATA: {} {} [{}] \"{}\" {}MB, LBA48={}, DMA={}",
                if drive == 0 {
                    "Secondary Master"
                } else {
                    "Secondary Slave"
                },
                dev_type,
                ata.serial,
                ata.model,
                size_mb,
                ata.lba48,
                ata.dma_supported
            );

            if ata.atapi {
                let base = ata.channel.base_port();
                let ctrl = ata.channel.control_port();
                if let Some((blocks, _block_size)) = atapi_read_capacity(base, ctrl, drive) {
                    ata.sectors = blocks as u64;
                }
            }

            found.push(ata);
        }
    }

    if found.is_empty() {
        serial_println!("  ATA: no drives found");
    }

    *ATA_DRIVES.lock() = found;
}
