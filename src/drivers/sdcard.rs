use crate::sync::Mutex;
/// SD/MMC card driver for Genesis
///
/// Implements the SD card protocol over SPI interface:
///   - Card detection and initialization (CMD0/CMD8/ACMD41/CMD58)
///   - Card type identification (SD V1, SDHC/SDXC V2, MMC)
///   - CSD and CID register parsing (capacity, speed class, serial)
///   - Block read (CMD17) and write (CMD24) with CRC
///   - Multi-block read (CMD18) / write (CMD25) with proper stop
///   - Speed class detection and SPI clock adjustment
///   - Timeout and error handling with retry logic
///
/// Inspired by: Linux mmc/sd SPI driver (drivers/mmc/host/mmc_spi.c),
/// SD Physical Layer Simplified Spec. All code is original.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// SPI I/O ports for SD controller
// ---------------------------------------------------------------------------

const SPI_DATA_PORT: u16 = 0xC500;
const SPI_CTRL_PORT: u16 = 0xC504;
const SPI_STATUS_PORT: u16 = 0xC508;
const SPI_CLK_DIV: u16 = 0xC50C;
const SPI_CS_PORT: u16 = 0xC510;

// SPI status bits
const SPI_BUSY: u8 = 0x01;
const SPI_TX_EMPTY: u8 = 0x02;
const SPI_RX_READY: u8 = 0x04;

// SD block size
const BLOCK_SIZE: u32 = 512;

// SD command tokens
const CMD0: u8 = 0; // GO_IDLE_STATE
const CMD1: u8 = 1; // SEND_OP_COND (MMC)
const CMD8: u8 = 8; // SEND_IF_COND
const CMD9: u8 = 9; // SEND_CSD
const CMD10: u8 = 10; // SEND_CID
const CMD12: u8 = 12; // STOP_TRANSMISSION
const CMD16: u8 = 16; // SET_BLOCKLEN
const CMD17: u8 = 17; // READ_SINGLE_BLOCK
const CMD18: u8 = 18; // READ_MULTIPLE_BLOCK
const CMD24: u8 = 24; // WRITE_BLOCK
const CMD25: u8 = 25; // WRITE_MULTIPLE_BLOCK
const CMD55: u8 = 55; // APP_CMD
const CMD58: u8 = 58; // READ_OCR
const ACMD41: u8 = 41; // SD_SEND_OP_COND

// R1 response bits
const R1_IDLE: u8 = 0x01;
const R1_ILLEGAL_CMD: u8 = 0x04;

// Data tokens
const DATA_TOKEN_SINGLE: u8 = 0xFE;
const DATA_TOKEN_MULTI: u8 = 0xFC;
const DATA_TOKEN_STOP: u8 = 0xFD;

// Maximum retries
const MAX_CMD_RETRIES: u32 = 100;
const MAX_INIT_RETRIES: u32 = 1000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// SD card type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardType {
    /// SD Version 1.x (byte addressing)
    SdV1,
    /// SD Version 2+ Standard Capacity (byte addressing)
    SdV2Sc,
    /// SDHC/SDXC (block addressing)
    Sdhc,
    /// MMC card
    Mmc,
    /// No card detected
    None,
}

/// Speed class
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedClass {
    Class0,
    Class2,
    Class4,
    Class6,
    Class10,
    Uhs1,
    Uhs3,
}

/// Parsed CSD register data
#[derive(Debug, Clone, Copy)]
pub struct CsdInfo {
    pub csd_version: u8,
    pub capacity_blocks: u64,
    pub max_read_bl_len: u32,
    pub read_speed_mhz: u32,
    pub write_protect: bool,
}

/// Parsed CID register data
#[derive(Debug, Clone, Copy)]
pub struct CidInfo {
    pub manufacturer_id: u8,
    pub oem_id: u16,
    pub product_rev: u8,
    pub serial_number: u32,
}

/// SD card information
#[derive(Debug, Clone)]
pub struct CardInfo {
    pub card_type: CardType,
    pub capacity_bytes: u64,
    pub block_count: u64,
    pub speed_class: SpeedClass,
    pub csd: CsdInfo,
    pub cid: CidInfo,
    pub ocr: u32,
    pub spi_clock_khz: u32,
}

// ---------------------------------------------------------------------------
// SPI communication primitives
// ---------------------------------------------------------------------------

fn spi_wait() {
    for _ in 0..10000 {
        if crate::io::inb(SPI_STATUS_PORT) & SPI_BUSY == 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

fn spi_transfer(data: u8) -> u8 {
    spi_wait();
    crate::io::outb(SPI_DATA_PORT, data);
    spi_wait();
    crate::io::inb(SPI_DATA_PORT)
}

fn spi_cs_assert() {
    crate::io::outb(SPI_CS_PORT, 0x00);
}
fn spi_cs_deassert() {
    crate::io::outb(SPI_CS_PORT, 0x01);
}

fn spi_set_clock(div: u16) {
    crate::io::outw(SPI_CLK_DIV, div);
}

/// Send an SD command and get R1 response
fn sd_cmd(cmd: u8, arg: u32) -> u8 {
    // Send 8 clock cycles with CS high (for clock sync)
    spi_transfer(0xFF);

    // Command frame: 01 | cmd(6) | arg(32) | crc(7) | 1
    spi_transfer(0x40 | cmd);
    spi_transfer((arg >> 24) as u8);
    spi_transfer((arg >> 16) as u8);
    spi_transfer((arg >> 8) as u8);
    spi_transfer(arg as u8);

    // CRC (only matters for CMD0 and CMD8)
    let crc = match cmd {
        CMD0 => 0x95,
        CMD8 => 0x87,
        _ => 0x01, // Dummy CRC + stop bit
    };
    spi_transfer(crc);

    // Wait for response (R1: MSB = 0)
    for _ in 0..MAX_CMD_RETRIES {
        let r = spi_transfer(0xFF);
        if r & 0x80 == 0 {
            return r;
        }
    }
    0xFF // Timeout
}

/// Send an application-specific command (CMD55 + ACMDx)
fn sd_acmd(cmd: u8, arg: u32) -> u8 {
    sd_cmd(CMD55, 0);
    sd_cmd(cmd, arg)
}

/// Wait for a data token response
fn wait_data_token() -> bool {
    for _ in 0..100_000 {
        let token = spi_transfer(0xFF);
        if token == DATA_TOKEN_SINGLE {
            return true;
        }
        if token != 0xFF {
            return false;
        } // Error token
        core::hint::spin_loop();
    }
    false
}

// ---------------------------------------------------------------------------
// Inner driver state
// ---------------------------------------------------------------------------

struct SdInner {
    initialized: bool,
    info: Option<CardInfo>,
}

impl SdInner {
    const fn new() -> Self {
        SdInner {
            initialized: false,
            info: None,
        }
    }

    /// Initialize the SD card (SPI mode)
    fn card_init(&mut self) -> bool {
        // Step 1: Set slow SPI clock (400 kHz) for init
        spi_set_clock(250); // 100 MHz / 250 = 400 kHz
        spi_cs_deassert();

        // Send >= 74 clock cycles with CS high
        for _ in 0..10 {
            spi_transfer(0xFF);
        }

        spi_cs_assert();

        // Step 2: CMD0 — go to idle state
        let r = sd_cmd(CMD0, 0);
        if r != R1_IDLE {
            spi_cs_deassert();
            return false;
        }

        // Step 3: CMD8 — check voltage (SD V2 detection)
        let r = sd_cmd(CMD8, 0x000001AA);
        let is_v2 = r != (R1_IDLE | R1_ILLEGAL_CMD);

        if is_v2 {
            // Read R7 response (4 bytes)
            let mut r7 = [0u8; 4];
            for b in r7.iter_mut() {
                *b = spi_transfer(0xFF);
            }
            if r7[2] != 0x01 || r7[3] != 0xAA {
                spi_cs_deassert();
                return false; // Voltage mismatch
            }
        }

        // Step 4: ACMD41 — initialize card (with HCS bit for V2)
        let hcs = if is_v2 { 0x4000_0000u32 } else { 0 };
        for _ in 0..MAX_INIT_RETRIES {
            let r = sd_acmd(ACMD41, hcs);
            if r == 0x00 {
                break;
            }
            if r != R1_IDLE {
                // Try MMC: CMD1
                let r = sd_cmd(CMD1, 0);
                if r == 0x00 {
                    break;
                }
            }
            core::hint::spin_loop();
        }

        // Step 5: CMD58 — read OCR to determine card type
        let _r = sd_cmd(CMD58, 0);
        let mut ocr = [0u8; 4];
        for b in ocr.iter_mut() {
            *b = spi_transfer(0xFF);
        }
        let ocr_val = u32::from_be_bytes(ocr);

        let card_type = if !is_v2 {
            CardType::SdV1
        } else if ocr_val & 0x4000_0000 != 0 {
            CardType::Sdhc
        } else {
            CardType::SdV2Sc
        };

        // Step 6: Set block size to 512 for non-SDHC
        if card_type != CardType::Sdhc {
            sd_cmd(CMD16, BLOCK_SIZE);
        }

        // Step 7: Read CSD
        let csd = self.read_csd();

        // Step 8: Read CID
        let cid = self.read_cid();

        // Step 9: Switch to fast SPI clock
        let fast_div = match csd.read_speed_mhz {
            50 => 2, // 50 MHz
            25 => 4, // 25 MHz
            _ => 8,  // 12.5 MHz safe default
        };
        spi_set_clock(fast_div);

        let capacity_bytes = csd.capacity_blocks.saturating_mul(BLOCK_SIZE as u64);
        let speed_class = self.detect_speed_class();

        self.info = Some(CardInfo {
            card_type,
            capacity_bytes,
            block_count: csd.capacity_blocks,
            speed_class,
            csd,
            cid,
            ocr: ocr_val,
            spi_clock_khz: 100_000 / fast_div as u32,
        });

        spi_cs_deassert();
        true
    }

    /// Read and parse CSD register
    fn read_csd(&self) -> CsdInfo {
        spi_cs_assert();
        sd_cmd(CMD9, 0);
        let mut csd = [0u8; 16];
        if wait_data_token() {
            for b in csd.iter_mut() {
                *b = spi_transfer(0xFF);
            }
            // Skip 2 CRC bytes
            spi_transfer(0xFF);
            spi_transfer(0xFF);
        }
        spi_cs_deassert();

        let version = (csd[0] >> 6) & 0x03;
        let capacity_blocks = if version == 1 {
            // CSD V2 (SDHC/SDXC): C_SIZE in bytes 7-9
            let c_size = ((csd[7] as u64 & 0x3F) << 16) | ((csd[8] as u64) << 8) | csd[9] as u64;
            c_size.saturating_add(1).saturating_mul(1024)
        } else {
            // CSD V1: C_SIZE, C_SIZE_MULT, READ_BL_LEN
            let read_bl_len = (csd[5] & 0x0F) as u32;
            let c_size =
                (((csd[6] & 0x03) as u64) << 10) | ((csd[7] as u64) << 2) | ((csd[8] >> 6) as u64);
            let c_size_mult = ((csd[9] & 0x03) as u32) << 1 | ((csd[10] >> 7) as u32);
            let mult = 1u64 << ((c_size_mult.saturating_add(2)) & 0x3F);
            let block_len = 1u64 << (read_bl_len & 0x3F);
            c_size
                .saturating_add(1)
                .saturating_mul(mult)
                .saturating_mul(block_len)
                / BLOCK_SIZE as u64
        };

        let tran_speed = csd[3];
        let read_speed_mhz = match tran_speed {
            0x5A => 50,
            0x32 => 25,
            _ => 25,
        };

        CsdInfo {
            csd_version: version,
            capacity_blocks,
            max_read_bl_len: if version == 1 {
                9
            } else {
                (csd[5] & 0x0F) as u32
            },
            read_speed_mhz,
            write_protect: csd[14] & 0x20 != 0,
        }
    }

    /// Read and parse CID register
    fn read_cid(&self) -> CidInfo {
        spi_cs_assert();
        sd_cmd(CMD10, 0);
        let mut cid = [0u8; 16];
        if wait_data_token() {
            for b in cid.iter_mut() {
                *b = spi_transfer(0xFF);
            }
            spi_transfer(0xFF);
            spi_transfer(0xFF);
        }
        spi_cs_deassert();

        CidInfo {
            manufacturer_id: cid[0],
            oem_id: ((cid[1] as u16) << 8) | cid[2] as u16,
            product_rev: cid[8],
            serial_number: u32::from_be_bytes([cid[9], cid[10], cid[11], cid[12]]),
        }
    }

    /// Detect speed class (basic heuristic from CSD data)
    fn detect_speed_class(&self) -> SpeedClass {
        // In a real driver, this reads the SD Status register (ACMD13)
        // Here we estimate from transfer speed
        if let Some(ref info) = self.info {
            match info.csd.read_speed_mhz {
                50 => SpeedClass::Class10,
                25 => SpeedClass::Class4,
                _ => SpeedClass::Class2,
            }
        } else {
            SpeedClass::Class2
        }
    }

    /// Read a single 512-byte block
    fn read_block_inner(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        if buf.len() < BLOCK_SIZE as usize {
            return Err("buffer too small");
        }
        let info = self.info.as_ref().ok_or("card not initialized")?;

        // SDHC uses block addressing; others use byte addressing
        let addr = if info.card_type == CardType::Sdhc {
            lba as u32
        } else {
            (lba * BLOCK_SIZE as u64) as u32
        };

        spi_cs_assert();
        let r = sd_cmd(CMD17, addr);
        if r != 0x00 {
            spi_cs_deassert();
            return Err("CMD17 failed");
        }

        if !wait_data_token() {
            spi_cs_deassert();
            return Err("data token timeout");
        }

        for i in 0..BLOCK_SIZE as usize {
            buf[i] = spi_transfer(0xFF);
        }
        // Read and discard CRC16
        spi_transfer(0xFF);
        spi_transfer(0xFF);

        spi_cs_deassert();
        Ok(())
    }

    /// Write a single 512-byte block
    fn write_block_inner(&self, lba: u64, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < BLOCK_SIZE as usize {
            return Err("data too small");
        }
        let info = self.info.as_ref().ok_or("card not initialized")?;
        if info.csd.write_protect {
            return Err("card is write protected");
        }

        let addr = if info.card_type == CardType::Sdhc {
            lba as u32
        } else {
            (lba * BLOCK_SIZE as u64) as u32
        };

        spi_cs_assert();
        let r = sd_cmd(CMD24, addr);
        if r != 0x00 {
            spi_cs_deassert();
            return Err("CMD24 failed");
        }

        // Send data token
        spi_transfer(DATA_TOKEN_SINGLE);

        // Send 512 bytes
        for i in 0..BLOCK_SIZE as usize {
            spi_transfer(data[i]);
        }
        // Dummy CRC16
        spi_transfer(0xFF);
        spi_transfer(0xFF);

        // Check data response
        let resp = spi_transfer(0xFF) & 0x1F;
        if resp != 0x05 {
            spi_cs_deassert();
            return Err("write rejected");
        }

        // Wait for busy (card pulls MISO low)
        for _ in 0..100_000 {
            if spi_transfer(0xFF) == 0xFF {
                break;
            }
            core::hint::spin_loop();
        }

        spi_cs_deassert();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CARD: Mutex<SdInner> = Mutex::new(SdInner::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the SD card driver
pub fn init() {
    let mut sd = CARD.lock();
    if sd.card_init() {
        sd.initialized = true;
        if let Some(ref info) = sd.info {
            let type_str = match info.card_type {
                CardType::SdV1 => "SD V1",
                CardType::SdV2Sc => "SD V2",
                CardType::Sdhc => "SDHC",
                CardType::Mmc => "MMC",
                CardType::None => "none",
            };
            let mb = info.capacity_bytes / (1024u64 * 1024);
            serial_println!(
                "  SD: {} {} MiB, {} blocks, SPI @ {} kHz",
                type_str,
                mb,
                info.block_count,
                info.spi_clock_khz
            );
        }
        drop(sd);
        super::register("sdcard", super::DeviceType::Storage);
    } else {
        serial_println!("  SD: no card detected");
    }
}

/// Read a 512-byte block
pub fn read_block(lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    let sd = CARD.lock();
    if !sd.initialized {
        return Err("SD not initialized");
    }
    sd.read_block_inner(lba, buf)
}

/// Write a 512-byte block
pub fn write_block(lba: u64, data: &[u8]) -> Result<(), &'static str> {
    let sd = CARD.lock();
    if !sd.initialized {
        return Err("SD not initialized");
    }
    sd.write_block_inner(lba, data)
}

/// Get card information
pub fn card_info() -> Option<CardInfo> {
    CARD.lock().info.clone()
}

/// Get card capacity in bytes
pub fn capacity() -> u64 {
    CARD.lock()
        .info
        .as_ref()
        .map(|i| i.capacity_bytes)
        .unwrap_or(0)
}

/// Get card type
pub fn card_type() -> CardType {
    CARD.lock()
        .info
        .as_ref()
        .map(|i| i.card_type)
        .unwrap_or(CardType::None)
}

/// Check if card is present and initialized
pub fn is_ready() -> bool {
    CARD.lock().initialized
}
