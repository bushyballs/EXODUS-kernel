use crate::memory::frame_allocator;
use crate::memory::frame_allocator::FRAME_SIZE;
use crate::sync::Mutex;
/// AHCI (Advanced Host Controller Interface) / SATA driver for Genesis
///
/// Supports SATA drives via the AHCI standard (Intel specification).
/// Uses PCI device class 0x01 subclass 0x06 (SATA controller).
///
/// Implements:
///   - HBA memory register access via PCI BAR5
///   - Port enumeration via PI (Ports Implemented) register
///   - Port initialization: stop/start command engine, setup cmd/FIS buffers
///   - Command table with CFIS (H2D Register FIS) for ATA commands
///   - IDENTIFY DEVICE command with 512-byte identify data parsing
///   - READ DMA EXT / WRITE DMA EXT with PRDT entries
///   - Device type detection (SATA disk, SATAPI, port multiplier)
///   - Interrupt handling: IS register, port IS, clear
///   - Error recovery: COMRESET on error, command retry
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

/// AHCI register offsets (HBA Memory Registers)
const HBA_CAP: usize = 0x00; // Host Capabilities
const HBA_GHC: usize = 0x04; // Global Host Control
const HBA_IS: usize = 0x08; // Interrupt Status
const HBA_PI: usize = 0x0C; // Ports Implemented
const HBA_VS: usize = 0x10; // Version
const HBA_CCC_CTL: usize = 0x14; // Command Completion Coalescing Control
const HBA_CCC_PTS: usize = 0x18; // Command Completion Coalescing Ports
const HBA_EM_LOC: usize = 0x1C; // Enclosure Management Location
const HBA_EM_CTL: usize = 0x20; // Enclosure Management Control
const HBA_CAP2: usize = 0x24; // Host Capabilities Extended
const HBA_BOHC: usize = 0x28; // BIOS/OS Handoff Control

/// GHC register bits
const GHC_HR: u32 = 1 << 0; // HBA Reset
const GHC_IE: u32 = 1 << 1; // Interrupt Enable
const GHC_AE: u32 = 1 << 31; // AHCI Enable

/// Port register offsets (relative to port base: 0x100 + port * 0x80)
const PORT_CLB: usize = 0x00; // Command List Base Address (lower 32)
const PORT_CLBU: usize = 0x04; // Command List Base Address (upper 32)
const PORT_FB: usize = 0x08; // FIS Base Address (lower 32)
const PORT_FBU: usize = 0x0C; // FIS Base Address (upper 32)
const PORT_IS: usize = 0x10; // Interrupt Status
const PORT_IE: usize = 0x14; // Interrupt Enable
const PORT_CMD: usize = 0x18; // Command and Status
const PORT_TFD: usize = 0x20; // Task File Data
const PORT_SIG: usize = 0x24; // Signature
const PORT_SSTS: usize = 0x28; // SATA Status (SCR0: SStatus)
const PORT_SCTL: usize = 0x2C; // SATA Control (SCR2: SControl)
const PORT_SERR: usize = 0x30; // SATA Error (SCR1: SError)
const PORT_SACT: usize = 0x34; // SATA Active
const PORT_CI: usize = 0x38; // Command Issue

/// Port CMD register bits
const PORT_CMD_ST: u32 = 1 << 0; // Start (command processing)
const PORT_CMD_SUD: u32 = 1 << 1; // Spin-Up Device
const PORT_CMD_POD: u32 = 1 << 2; // Power On Device
const PORT_CMD_FRE: u32 = 1 << 4; // FIS Receive Enable
const PORT_CMD_FR: u32 = 1 << 14; // FIS Receive Running
const PORT_CMD_CR: u32 = 1 << 15; // Command List Running

/// Port IS (Interrupt Status) bits
const PORT_IS_DHRS: u32 = 1 << 0; // Device to Host Register FIS
const PORT_IS_PSS: u32 = 1 << 1; // PIO Setup FIS
const PORT_IS_DSS: u32 = 1 << 2; // DMA Setup FIS
const PORT_IS_SDBS: u32 = 1 << 3; // Set Device Bits FIS
const PORT_IS_DPS: u32 = 1 << 5; // Descriptor Processed
const PORT_IS_TFES: u32 = 1 << 30; // Task File Error Status

/// Port signature values
const SATA_SIG_ATA: u32 = 0x00000101; // SATA drive
const SATA_SIG_ATAPI: u32 = 0xEB140101; // SATAPI drive
const SATA_SIG_SEMB: u32 = 0xC33C0101; // Enclosure management bridge
const SATA_SIG_PM: u32 = 0x96690101; // Port multiplier

/// FIS types
const FIS_TYPE_REG_H2D: u8 = 0x27; // Register FIS: Host to Device
const FIS_TYPE_REG_D2H: u8 = 0x34; // Register FIS: Device to Host
const FIS_TYPE_DMA_ACT: u8 = 0x39; // DMA Activate FIS
const FIS_TYPE_DMA_SETUP: u8 = 0x41; // DMA Setup FIS
const FIS_TYPE_DATA: u8 = 0x46; // Data FIS
const FIS_TYPE_BIST: u8 = 0x58; // BIST Activate FIS
const FIS_TYPE_PIO_SETUP: u8 = 0x5F; // PIO Setup FIS

/// ATA commands
const ATA_CMD_IDENTIFY: u8 = 0xEC;
const ATA_CMD_IDENTIFY_PACKET: u8 = 0xA1;
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;
const ATA_CMD_SET_FEATURES: u8 = 0xEF;

/// Maximum number of PRDT entries per command
const MAX_PRDT_ENTRIES: usize = 8;
/// Sector size
const SECTOR_SIZE: usize = 512;
/// Maximum sectors per single DMA command
const MAX_SECTORS_PER_CMD: u16 = 128;

/// Command List Header (one per command slot, 32 bytes each)
#[repr(C)]
#[derive(Clone, Copy)]
struct CmdHeader {
    /// DW0: Command FIS length (in DWORDs), flags
    dw0: u16,
    /// Flags continued + PRDTL (PRD Table Length)
    prdtl: u16,
    /// PRD Byte Count (updated by HBA after transfer)
    prdbc: u32,
    /// Command Table Base Address (lower 32)
    ctba: u32,
    /// Command Table Base Address (upper 32)
    ctbau: u32,
    /// Reserved
    reserved: [u32; 4],
}

/// Command Table: CFIS (Command FIS) + PRDT entries
/// Total size must be 128-byte aligned
#[repr(C)]
struct CmdTable {
    /// Command FIS (up to 64 bytes)
    cfis: [u8; 64],
    /// ATAPI Command (12-16 bytes, padded to 16)
    acmd: [u8; 16],
    /// Reserved
    reserved: [u8; 48],
    /// Physical Region Descriptor Table entries
    prdt: [PrdtEntry; MAX_PRDT_ENTRIES],
}

/// Physical Region Descriptor Table Entry (16 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
struct PrdtEntry {
    /// Data Base Address (lower 32)
    dba: u32,
    /// Data Base Address (upper 32)
    dbau: u32,
    /// Reserved
    reserved: u32,
    /// Byte Count (bit 0 must be 1 = even byte count), bit 31 = interrupt on completion
    dbc: u32,
}

/// H2D Register FIS (Host to Device)
#[repr(C)]
#[derive(Clone, Copy)]
struct FisRegH2D {
    fis_type: u8,   // FIS_TYPE_REG_H2D
    flags: u8,      // bit 7: command/control, bits 3:0: port multiplier
    command: u8,    // ATA command
    feature_lo: u8, // Feature register (7:0)
    lba0: u8,       // LBA 7:0
    lba1: u8,       // LBA 15:8
    lba2: u8,       // LBA 23:16
    device: u8,     // Device register
    lba3: u8,       // LBA 31:24
    lba4: u8,       // LBA 39:32
    lba5: u8,       // LBA 47:40
    feature_hi: u8, // Feature register (15:8)
    count_lo: u8,   // Sector count (7:0)
    count_hi: u8,   // Sector count (15:8)
    icc: u8,        // Isochronous command completion
    control: u8,    // Control register
    reserved: [u8; 4],
}

/// AHCI port info
#[derive(Debug, Clone)]
pub struct AhciPort {
    pub port_num: u8,
    pub device_type: AhciDeviceType,
    pub sectors: u64,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub initialized: bool,
    pub cmd_list_addr: usize,
    pub fis_addr: usize,
    pub cmd_table_addrs: [usize; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AhciDeviceType {
    Sata,
    Satapi,
    Semb,
    PortMultiplier,
    None,
}

pub struct AhciController {
    pub base_addr: usize,
    pub ports: Vec<AhciPort>,
    pub num_ports: u8,
    pub num_cmd_slots: u8,
    pub supports_64bit: bool,
    pub supports_ncq: bool,
}

impl AhciController {
    /// Read a 32-bit register from the ABAR
    fn read32(&self, offset: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base_addr + offset) as *const u32) }
    }

    /// Write a 32-bit register
    fn write32(&self, offset: usize, value: u32) {
        unsafe { core::ptr::write_volatile((self.base_addr + offset) as *mut u32, value) }
    }

    /// Port base address offset
    #[inline(always)]
    fn port_base(&self, port: u8) -> usize {
        // port is always < 32 at call sites; use saturating to be safe
        0x100usize.saturating_add((port as usize).saturating_mul(0x80))
    }

    /// Read a port register
    fn port_read(&self, port: u8, offset: usize) -> u32 {
        self.read32(self.port_base(port) + offset)
    }

    /// Write a port register
    fn port_write(&self, port: u8, offset: usize, value: u32) {
        self.write32(self.port_base(port) + offset, value)
    }

    /// Check what type of device is on a port
    fn check_port_type(&self, port: u8) -> AhciDeviceType {
        let ssts = self.port_read(port, PORT_SSTS);
        let ipm = (ssts >> 8) & 0x0F;
        let det = ssts & 0x0F;

        if det != 3 || ipm != 1 {
            return AhciDeviceType::None;
        }

        let sig = self.port_read(port, PORT_SIG);
        match sig {
            SATA_SIG_ATA => AhciDeviceType::Sata,
            SATA_SIG_ATAPI => AhciDeviceType::Satapi,
            SATA_SIG_SEMB => AhciDeviceType::Semb,
            SATA_SIG_PM => AhciDeviceType::PortMultiplier,
            _ => AhciDeviceType::Sata, // default to SATA
        }
    }

    /// Stop the command engine for a port
    fn stop_cmd_engine(&self, port: u8) {
        let mut cmd = self.port_read(port, PORT_CMD);

        // Clear ST (start)
        cmd &= !PORT_CMD_ST;
        self.port_write(port, PORT_CMD, cmd);

        // Wait for CR (Command List Running) to clear
        for _ in 0..500_000u32 {
            let cmd = self.port_read(port, PORT_CMD);
            if cmd & PORT_CMD_CR == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Clear FRE (FIS Receive Enable)
        cmd = self.port_read(port, PORT_CMD);
        cmd &= !PORT_CMD_FRE;
        self.port_write(port, PORT_CMD, cmd);

        // Wait for FR (FIS Receive Running) to clear
        for _ in 0..500_000u32 {
            let cmd = self.port_read(port, PORT_CMD);
            if cmd & PORT_CMD_FR == 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }

    /// Start the command engine for a port
    fn start_cmd_engine(&self, port: u8) {
        // Wait for CR to clear before starting
        for _ in 0..500_000u32 {
            let cmd = self.port_read(port, PORT_CMD);
            if cmd & PORT_CMD_CR == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        let mut cmd = self.port_read(port, PORT_CMD);
        cmd |= PORT_CMD_FRE; // Enable FIS receive first
        self.port_write(port, PORT_CMD, cmd);

        cmd |= PORT_CMD_ST; // Then enable command processing
        self.port_write(port, PORT_CMD, cmd);
    }

    /// Initialize a port's command list and FIS receive buffers
    fn init_port(&self, port_info: &mut AhciPort) -> bool {
        let port = port_info.port_num;

        // Stop the command engine
        self.stop_cmd_engine(port);

        // Allocate command list (1KB aligned, 32 slots * 32 bytes = 1024 bytes)
        let cmd_list_frame = match frame_allocator::allocate_frame() {
            Some(f) => f,
            None => return false,
        };
        unsafe {
            core::ptr::write_bytes(cmd_list_frame.addr as *mut u8, 0, FRAME_SIZE);
        }
        port_info.cmd_list_addr = cmd_list_frame.addr;

        // Allocate FIS receive buffer (256 bytes, 256-byte aligned)
        let fis_frame = match frame_allocator::allocate_frame() {
            Some(f) => f,
            None => return false,
        };
        unsafe {
            core::ptr::write_bytes(fis_frame.addr as *mut u8, 0, FRAME_SIZE);
        }
        port_info.fis_addr = fis_frame.addr;

        // Set command list and FIS base addresses
        self.port_write(port, PORT_CLB, cmd_list_frame.addr as u32);
        self.port_write(port, PORT_CLBU, (cmd_list_frame.addr as u64 >> 32) as u32);
        self.port_write(port, PORT_FB, fis_frame.addr as u32);
        self.port_write(port, PORT_FBU, (fis_frame.addr as u64 >> 32) as u32);

        // Allocate command tables (one per command slot, 256 bytes each)
        let num_slots = self.num_cmd_slots.min(32) as usize;
        for i in 0..num_slots {
            let ct_frame = match frame_allocator::allocate_frame() {
                Some(f) => f,
                None => return false,
            };
            unsafe {
                core::ptr::write_bytes(ct_frame.addr as *mut u8, 0, FRAME_SIZE);
            }
            port_info.cmd_table_addrs[i] = ct_frame.addr;

            // Set command table address in command list header
            let header_addr = cmd_list_frame.addr.saturating_add(i.saturating_mul(32));
            let header = unsafe { &mut *(header_addr as *mut CmdHeader) };
            header.ctba = ct_frame.addr as u32;
            header.ctbau = (ct_frame.addr as u64 >> 32) as u32;
        }

        // Clear port interrupt status
        self.port_write(port, PORT_IS, 0xFFFFFFFF);

        // Clear SATA error register
        self.port_write(port, PORT_SERR, 0xFFFFFFFF);

        // Enable port interrupts
        self.port_write(
            port,
            PORT_IE,
            PORT_IS_DHRS | PORT_IS_PSS | PORT_IS_DSS | PORT_IS_SDBS | PORT_IS_TFES,
        );

        // Start the command engine
        self.start_cmd_engine(port);

        port_info.initialized = true;
        true
    }

    /// Build an H2D Register FIS for an ATA command
    fn build_h2d_fis(command: u8, lba: u64, count: u16, device: u8) -> FisRegH2D {
        FisRegH2D {
            fis_type: FIS_TYPE_REG_H2D,
            flags: 0x80, // Command bit set (this is a command, not control)
            command,
            feature_lo: 0,
            lba0: (lba & 0xFF) as u8,
            lba1: ((lba >> 8) & 0xFF) as u8,
            lba2: ((lba >> 16) & 0xFF) as u8,
            device,
            lba3: ((lba >> 24) & 0xFF) as u8,
            lba4: ((lba >> 32) & 0xFF) as u8,
            lba5: ((lba >> 40) & 0xFF) as u8,
            feature_hi: 0,
            count_lo: (count & 0xFF) as u8,
            count_hi: ((count >> 8) & 0xFF) as u8,
            icc: 0,
            control: 0,
            reserved: [0; 4],
        }
    }

    /// Issue a command on a port and wait for completion
    fn issue_command(&self, port_info: &AhciPort, slot: u32) -> Result<(), &'static str> {
        let port = port_info.port_num;

        // Make sure the slot is not currently in use
        let ci = self.port_read(port, PORT_CI);
        if ci & (1 << slot) != 0 {
            return Err("command slot busy");
        }

        // Fence: ensure command table (CFIS + PRDT) writes are visible before CI doorbell
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        // Issue the command
        self.port_write(port, PORT_CI, 1 << slot);

        // Wait for completion: CI bit clears, or error
        for _ in 0..1_000_000u32 {
            let ci = self.port_read(port, PORT_CI);
            if ci & (1 << slot) == 0 {
                // Check for errors
                let is = self.port_read(port, PORT_IS);
                if is & PORT_IS_TFES != 0 {
                    // Task file error
                    let tfd = self.port_read(port, PORT_TFD);
                    let error = ((tfd >> 8) & 0xFF) as u8;
                    let status = (tfd & 0xFF) as u8;
                    serial_println!(
                        "  AHCI: port {} task file error: status={:#x} err={:#x}",
                        port,
                        status,
                        error
                    );
                    self.port_write(port, PORT_IS, PORT_IS_TFES);
                    return Err("task file error");
                }
                // Clear interrupt status
                self.port_write(port, PORT_IS, is);
                return Ok(());
            }

            // Check for fatal error
            let is = self.port_read(port, PORT_IS);
            if is & PORT_IS_TFES != 0 {
                self.port_write(port, PORT_IS, PORT_IS_TFES);
                return Err("task file error during wait");
            }

            core::hint::spin_loop();
        }

        Err("command timeout")
    }

    /// Send IDENTIFY DEVICE command to a port, parse the 512-byte response
    fn identify_device(&self, port_info: &mut AhciPort) -> bool {
        let port = port_info.port_num;
        let slot: u32 = 0;

        // Allocate a buffer for the identify data (512 bytes)
        let data_frame = match frame_allocator::allocate_frame() {
            Some(f) => f,
            None => return false,
        };
        unsafe {
            core::ptr::write_bytes(data_frame.addr as *mut u8, 0, FRAME_SIZE);
        }

        // Build the command in slot 0
        let header_addr = port_info
            .cmd_list_addr
            .saturating_add((slot as usize).saturating_mul(32));
        let header = unsafe { &mut *(header_addr as *mut CmdHeader) };

        // Command FIS length = 5 DWORDs (20 bytes for H2D FIS), Write=0 (device to host)
        header.dw0 = 5; // CFL = 5 DWORDs
        header.prdtl = 1; // 1 PRDT entry
        header.prdbc = 0;

        // Set up the command table
        let slot_idx = slot as usize;
        if slot_idx >= port_info.cmd_table_addrs.len() {
            serial_println!("  AHCI: identify slot {} out of range", slot);
            return false;
        }
        let ct_addr = port_info.cmd_table_addrs[slot_idx];
        let ct = unsafe { &mut *(ct_addr as *mut CmdTable) };

        // Zero the command FIS area
        for b in ct.cfis.iter_mut() {
            *b = 0;
        }

        // Build H2D Register FIS for IDENTIFY DEVICE
        let cmd = if port_info.device_type == AhciDeviceType::Satapi {
            ATA_CMD_IDENTIFY_PACKET
        } else {
            ATA_CMD_IDENTIFY
        };

        let fis = Self::build_h2d_fis(cmd, 0, 0, 0);
        let fis_bytes = unsafe {
            core::slice::from_raw_parts(
                &fis as *const FisRegH2D as *const u8,
                core::mem::size_of::<FisRegH2D>(),
            )
        };
        ct.cfis[..fis_bytes.len()].copy_from_slice(fis_bytes);

        // Set up PRDT entry pointing to our data buffer
        ct.prdt[0].dba = data_frame.addr as u32;
        ct.prdt[0].dbau = (data_frame.addr as u64 >> 32) as u32;
        ct.prdt[0].reserved = 0;
        ct.prdt[0].dbc = 511; // 512 bytes - 1 (byte count is 0-based)

        // Issue command and wait
        if self.issue_command(port_info, slot).is_err() {
            serial_println!("  AHCI: IDENTIFY DEVICE failed on port {}", port);
            return false;
        }

        // Parse the 512-byte identify data
        let identify = unsafe { core::slice::from_raw_parts(data_frame.addr as *const u16, 256) };

        // Model string (words 27-46, ATA strings are byte-swapped)
        let mut model_bytes = [0u8; 40];
        for i in 0..20 {
            let word = identify[27 + i];
            model_bytes[i * 2] = (word >> 8) as u8;
            model_bytes[i * 2 + 1] = (word & 0xFF) as u8;
        }
        port_info.model = String::from_utf8_lossy(&model_bytes).trim().into();

        // Serial number (words 10-19)
        let mut serial_bytes = [0u8; 20];
        for i in 0..10 {
            let word = identify[10 + i];
            serial_bytes[i * 2] = (word >> 8) as u8;
            serial_bytes[i * 2 + 1] = (word & 0xFF) as u8;
        }
        port_info.serial = String::from_utf8_lossy(&serial_bytes).trim().into();

        // Firmware revision (words 23-26)
        let mut fw_bytes = [0u8; 8];
        for i in 0..4 {
            let word = identify[23 + i];
            fw_bytes[i * 2] = (word >> 8) as u8;
            fw_bytes[i * 2 + 1] = (word & 0xFF) as u8;
        }
        port_info.firmware = String::from_utf8_lossy(&fw_bytes).trim().into();

        // LBA48 sector count (words 100-103)
        let lba48_supported = identify[83] & (1 << 10) != 0;
        if lba48_supported {
            port_info.sectors = (identify[100] as u64)
                | ((identify[101] as u64) << 16)
                | ((identify[102] as u64) << 32)
                | ((identify[103] as u64) << 48);
        } else {
            // LBA28 sector count (words 60-61)
            port_info.sectors = (identify[60] as u64) | ((identify[61] as u64) << 16);
        }

        let size_mb = port_info.sectors * SECTOR_SIZE as u64 / (1024 * 1024);
        serial_println!(
            "  AHCI: port {} - {} ({}) {}MB, FW: {}",
            port,
            port_info.model,
            port_info.serial,
            size_mb,
            port_info.firmware
        );

        true
    }

    /// Issue a COMRESET on a port to recover from errors
    fn comreset(&self, port: u8) {
        // Stop command engine
        self.stop_cmd_engine(port);

        // Issue COMRESET via SControl register
        // DET field (bits 3:0) = 1 means perform initialization sequence
        let sctl = self.port_read(port, PORT_SCTL);
        self.port_write(port, PORT_SCTL, (sctl & !0xF) | 1);

        // Wait at least 1ms for COMRESET to be sent
        crate::time::clock::sleep_ms(2);

        // Clear DET to allow device to re-establish communication
        let sctl = self.port_read(port, PORT_SCTL);
        self.port_write(port, PORT_SCTL, sctl & !0xF);

        // Wait for device detection (DET = 3 in SStatus)
        for _ in 0..100 {
            let ssts = self.port_read(port, PORT_SSTS);
            if (ssts & 0xF) == 3 {
                break;
            }
            crate::time::clock::sleep_ms(10);
        }

        // Clear SERR
        self.port_write(port, PORT_SERR, 0xFFFFFFFF);

        // Start command engine
        self.start_cmd_engine(port);
    }

    /// Handle interrupts for the AHCI controller
    pub fn handle_interrupt(&self) {
        let global_is = self.read32(HBA_IS);
        if global_is == 0 {
            return;
        }

        for port_idx in 0..32u8 {
            if global_is & (1 << port_idx) == 0 {
                continue;
            }

            let port_is = self.port_read(port_idx, PORT_IS);

            if port_is & PORT_IS_TFES != 0 {
                // Task file error — log it
                let tfd = self.port_read(port_idx, PORT_TFD);
                serial_println!("  AHCI: port {} error, TFD={:#x}", port_idx, tfd);
            }

            // Clear port interrupt status
            self.port_write(port_idx, PORT_IS, port_is);
        }

        // Clear global interrupt status
        self.write32(HBA_IS, global_is);
    }

    /// Read sectors from a port using READ DMA EXT
    fn read_dma(
        &self,
        port_info: &AhciPort,
        lba: u64,
        count: u16,
        buf_addr: usize,
    ) -> Result<(), &'static str> {
        if !port_info.initialized {
            return Err("port not initialized");
        }
        if count == 0 || count > MAX_SECTORS_PER_CMD {
            return Err("invalid sector count");
        }

        let slot: u32 = 0;
        let transfer_size = count as u32 * SECTOR_SIZE as u32;

        // Set up command header
        let header_addr = port_info
            .cmd_list_addr
            .saturating_add((slot as usize).saturating_mul(32));
        let header = unsafe { &mut *(header_addr as *mut CmdHeader) };
        header.dw0 = 5; // CFL = 5 DWORDs (H2D Register FIS)
        header.prdtl = 1;
        header.prdbc = 0;

        // Set up command table
        let slot_idx = slot as usize;
        if slot_idx >= port_info.cmd_table_addrs.len() {
            return Err("command slot index out of range");
        }
        let ct_addr = port_info.cmd_table_addrs[slot_idx];
        let ct = unsafe { &mut *(ct_addr as *mut CmdTable) };
        for b in ct.cfis.iter_mut() {
            *b = 0;
        }

        // Build H2D FIS for READ DMA EXT
        let fis = Self::build_h2d_fis(ATA_CMD_READ_DMA_EXT, lba, count, 0x40); // LBA mode
        let fis_bytes = unsafe {
            core::slice::from_raw_parts(
                &fis as *const FisRegH2D as *const u8,
                core::mem::size_of::<FisRegH2D>(),
            )
        };
        ct.cfis[..fis_bytes.len()].copy_from_slice(fis_bytes);

        // Set up PRDT
        ct.prdt[0].dba = buf_addr as u32;
        ct.prdt[0].dbau = (buf_addr as u64 >> 32) as u32;
        ct.prdt[0].reserved = 0;
        ct.prdt[0].dbc = transfer_size - 1; // 0-based

        self.issue_command(port_info, slot)
    }

    /// Write sectors to a port using WRITE DMA EXT
    fn write_dma(
        &self,
        port_info: &AhciPort,
        lba: u64,
        count: u16,
        buf_addr: usize,
    ) -> Result<(), &'static str> {
        if !port_info.initialized {
            return Err("port not initialized");
        }
        if count == 0 || count > MAX_SECTORS_PER_CMD {
            return Err("invalid sector count");
        }

        let slot: u32 = 0;
        let transfer_size = count as u32 * SECTOR_SIZE as u32;

        let header_addr = port_info
            .cmd_list_addr
            .saturating_add((slot as usize).saturating_mul(32));
        let header = unsafe { &mut *(header_addr as *mut CmdHeader) };
        header.dw0 = 5 | (1 << 6); // CFL = 5, Write bit set
        header.prdtl = 1;
        header.prdbc = 0;

        let slot_idx = slot as usize;
        if slot_idx >= port_info.cmd_table_addrs.len() {
            return Err("command slot index out of range");
        }
        let ct_addr = port_info.cmd_table_addrs[slot_idx];
        let ct = unsafe { &mut *(ct_addr as *mut CmdTable) };
        for b in ct.cfis.iter_mut() {
            *b = 0;
        }

        let fis = Self::build_h2d_fis(ATA_CMD_WRITE_DMA_EXT, lba, count, 0x40);
        let fis_bytes = unsafe {
            core::slice::from_raw_parts(
                &fis as *const FisRegH2D as *const u8,
                core::mem::size_of::<FisRegH2D>(),
            )
        };
        ct.cfis[..fis_bytes.len()].copy_from_slice(fis_bytes);

        ct.prdt[0].dba = buf_addr as u32;
        ct.prdt[0].dbau = (buf_addr as u64 >> 32) as u32;
        ct.prdt[0].reserved = 0;
        ct.prdt[0].dbc = transfer_size - 1;

        self.issue_command(port_info, slot)
    }

    /// Flush cache on a port
    fn flush(&self, port_info: &AhciPort) -> Result<(), &'static str> {
        if !port_info.initialized {
            return Err("port not initialized");
        }

        let slot: u32 = 0;
        let header_addr = port_info
            .cmd_list_addr
            .saturating_add((slot as usize).saturating_mul(32));
        let header = unsafe { &mut *(header_addr as *mut CmdHeader) };
        header.dw0 = 5;
        header.prdtl = 0; // no data transfer
        header.prdbc = 0;

        let slot_idx = slot as usize;
        if slot_idx >= port_info.cmd_table_addrs.len() {
            return Err("command slot index out of range");
        }
        let ct_addr = port_info.cmd_table_addrs[slot_idx];
        let ct = unsafe { &mut *(ct_addr as *mut CmdTable) };
        for b in ct.cfis.iter_mut() {
            *b = 0;
        }

        let fis = Self::build_h2d_fis(ATA_CMD_FLUSH_CACHE_EXT, 0, 0, 0);
        let fis_bytes = unsafe {
            core::slice::from_raw_parts(
                &fis as *const FisRegH2D as *const u8,
                core::mem::size_of::<FisRegH2D>(),
            )
        };
        ct.cfis[..fis_bytes.len()].copy_from_slice(fis_bytes);

        self.issue_command(port_info, slot)
    }

    /// Probe all ports and initialize those with devices
    fn probe_ports(&mut self) {
        let pi = self.read32(HBA_PI);

        for i in 0..32u8 {
            if pi & (1 << i) == 0 {
                continue;
            }

            let dev_type = self.check_port_type(i);
            if dev_type == AhciDeviceType::None {
                continue;
            }

            let mut port = AhciPort {
                port_num: i,
                device_type: dev_type,
                sectors: 0,
                model: alloc::format!("SATA-port{}", i),
                serial: String::new(),
                firmware: String::new(),
                initialized: false,
                cmd_list_addr: 0,
                fis_addr: 0,
                cmd_table_addrs: [0; 32],
            };

            serial_println!("  AHCI: port {} = {:?}", i, dev_type);

            // Initialize port memory structures
            if self.init_port(&mut port) {
                // Send IDENTIFY DEVICE
                if dev_type == AhciDeviceType::Sata || dev_type == AhciDeviceType::Satapi {
                    self.identify_device(&mut port);
                }
            }

            self.ports.push(port);
        }
    }

    /// Perform an HBA reset
    fn hba_reset(&self) {
        // Set HR bit in GHC
        let ghc = self.read32(HBA_GHC);
        self.write32(HBA_GHC, ghc | GHC_HR);

        // Wait for HR to clear (HBA sets it back to 0 when reset is complete)
        for _ in 0..1_000_000u32 {
            if self.read32(HBA_GHC) & GHC_HR == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Re-enable AHCI mode
        let ghc = self.read32(HBA_GHC);
        self.write32(HBA_GHC, ghc | GHC_AE);
    }
}

/// NCQ (Native Command Queuing) support constants
const ATA_CMD_READ_FPDMA_QUEUED: u8 = 0x60;
const ATA_CMD_WRITE_FPDMA_QUEUED: u8 = 0x61;
/// Maximum NCQ queue depth (per SATA spec)
const MAX_NCQ_DEPTH: u8 = 32;

/// Command slot allocation bitmap for tracking which slots are in use
struct SlotAllocator {
    /// Bitmap of free slots (bit set = free)
    free_bitmap: u32,
    /// Total number of available slots
    total_slots: u8,
}

impl SlotAllocator {
    fn new(num_slots: u8) -> Self {
        let mask = if num_slots >= 32 {
            0xFFFFFFFF
        } else {
            (1u32 << num_slots) - 1
        };
        SlotAllocator {
            free_bitmap: mask,
            total_slots: num_slots,
        }
    }

    /// Allocate a free command slot, returns slot number or None
    fn alloc(&mut self) -> Option<u32> {
        if self.free_bitmap == 0 {
            return None;
        }
        // Find lowest set bit
        let slot = self.free_bitmap.trailing_zeros();
        if slot >= self.total_slots as u32 {
            return None;
        }
        self.free_bitmap &= !(1 << slot);
        Some(slot)
    }

    /// Free a command slot
    fn free(&mut self, slot: u32) {
        if slot < self.total_slots as u32 {
            self.free_bitmap |= 1 << slot;
        }
    }

    /// Check if a slot is currently in use
    fn is_used(&self, slot: u32) -> bool {
        slot < self.total_slots as u32 && (self.free_bitmap & (1 << slot)) == 0
    }

    /// Get number of free slots
    fn free_count(&self) -> u32 {
        self.free_bitmap.count_ones()
    }

    /// Get number of used slots
    fn used_count(&self) -> u32 {
        self.total_slots as u32 - self.free_count()
    }
}

impl AhciController {
    /// Find a free command slot for a port
    fn find_free_slot(&self, port: u8) -> Option<u32> {
        let ci = self.port_read(port, PORT_CI);
        let sact = self.port_read(port, PORT_SACT);
        let busy = ci | sact;

        for slot in 0..self.num_cmd_slots as u32 {
            if busy & (1 << slot) == 0 {
                return Some(slot);
            }
        }
        None
    }

    /// Issue an NCQ command (READ FPDMA QUEUED / WRITE FPDMA QUEUED)
    /// NCQ allows up to 32 commands outstanding simultaneously
    fn issue_ncq_read(
        &self,
        port_info: &AhciPort,
        lba: u64,
        count: u16,
        buf_addr: usize,
    ) -> Result<u32, &'static str> {
        if !port_info.initialized {
            return Err("port not initialized");
        }
        if !self.supports_ncq {
            return Err("NCQ not supported");
        }

        // Find a free command slot
        let slot = self
            .find_free_slot(port_info.port_num)
            .ok_or("no free command slots")?;

        let transfer_size = count as u32 * SECTOR_SIZE as u32;

        // Set up command header
        let header_addr = port_info
            .cmd_list_addr
            .saturating_add((slot as usize).saturating_mul(32));
        let header = unsafe { &mut *(header_addr as *mut CmdHeader) };
        header.dw0 = 5; // CFL = 5 DWORDs
        header.prdtl = 1;
        header.prdbc = 0;

        // Set up command table
        let slot_idx = slot as usize;
        if slot_idx >= port_info.cmd_table_addrs.len() {
            return Err("command slot index out of range");
        }
        let ct_addr = port_info.cmd_table_addrs[slot_idx];
        let ct = unsafe { &mut *(ct_addr as *mut CmdTable) };
        for b in ct.cfis.iter_mut() {
            *b = 0;
        }

        // Build NCQ FIS -- different from regular H2D FIS
        // For FPDMA, the FIS encodes the tag in the sector count register
        ct.cfis[0] = FIS_TYPE_REG_H2D;
        ct.cfis[1] = 0x80; // Command bit
        ct.cfis[2] = ATA_CMD_READ_FPDMA_QUEUED;
        ct.cfis[3] = (count & 0xFF) as u8; // Feature (7:0) = sector count low

        ct.cfis[4] = (lba & 0xFF) as u8; // LBA 7:0
        ct.cfis[5] = ((lba >> 8) & 0xFF) as u8; // LBA 15:8
        ct.cfis[6] = ((lba >> 16) & 0xFF) as u8; // LBA 23:16
        ct.cfis[7] = 0x40; // Device: LBA mode

        ct.cfis[8] = ((lba >> 24) & 0xFF) as u8; // LBA 31:24
        ct.cfis[9] = ((lba >> 32) & 0xFF) as u8; // LBA 39:32
        ct.cfis[10] = ((lba >> 40) & 0xFF) as u8; // LBA 47:40
        ct.cfis[11] = ((count >> 8) & 0xFF) as u8; // Feature (15:8) = sector count high

        // Sector Count register encodes the tag (NCQ tag = slot number)
        ct.cfis[12] = (slot << 3) as u8; // Tag in bits 7:3

        // PRDT entry
        ct.prdt[0].dba = buf_addr as u32;
        ct.prdt[0].dbau = (buf_addr as u64 >> 32) as u32;
        ct.prdt[0].reserved = 0;
        ct.prdt[0].dbc = transfer_size - 1;

        // For NCQ, set SACT (SActive) bit BEFORE CI
        // Fence: ensure command table writes are visible before doorbell
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.port_write(port_info.port_num, PORT_SACT, 1 << slot);
        self.port_write(port_info.port_num, PORT_CI, 1 << slot);

        Ok(slot)
    }

    /// Issue an NCQ write command
    fn issue_ncq_write(
        &self,
        port_info: &AhciPort,
        lba: u64,
        count: u16,
        buf_addr: usize,
    ) -> Result<u32, &'static str> {
        if !port_info.initialized {
            return Err("port not initialized");
        }
        if !self.supports_ncq {
            return Err("NCQ not supported");
        }

        let slot = self
            .find_free_slot(port_info.port_num)
            .ok_or("no free command slots")?;
        let transfer_size = count as u32 * SECTOR_SIZE as u32;

        let header_addr = port_info
            .cmd_list_addr
            .saturating_add((slot as usize).saturating_mul(32));
        let header = unsafe { &mut *(header_addr as *mut CmdHeader) };
        header.dw0 = 5 | (1 << 6); // CFL = 5, Write bit set
        header.prdtl = 1;
        header.prdbc = 0;

        let slot_idx = slot as usize;
        if slot_idx >= port_info.cmd_table_addrs.len() {
            return Err("command slot index out of range");
        }
        let ct_addr = port_info.cmd_table_addrs[slot_idx];
        let ct = unsafe { &mut *(ct_addr as *mut CmdTable) };
        for b in ct.cfis.iter_mut() {
            *b = 0;
        }

        ct.cfis[0] = FIS_TYPE_REG_H2D;
        ct.cfis[1] = 0x80;
        ct.cfis[2] = ATA_CMD_WRITE_FPDMA_QUEUED;
        ct.cfis[3] = (count & 0xFF) as u8;

        ct.cfis[4] = (lba & 0xFF) as u8;
        ct.cfis[5] = ((lba >> 8) & 0xFF) as u8;
        ct.cfis[6] = ((lba >> 16) & 0xFF) as u8;
        ct.cfis[7] = 0x40;

        ct.cfis[8] = ((lba >> 24) & 0xFF) as u8;
        ct.cfis[9] = ((lba >> 32) & 0xFF) as u8;
        ct.cfis[10] = ((lba >> 40) & 0xFF) as u8;
        ct.cfis[11] = ((count >> 8) & 0xFF) as u8;

        ct.cfis[12] = (slot << 3) as u8;

        ct.prdt[0].dba = buf_addr as u32;
        ct.prdt[0].dbau = (buf_addr as u64 >> 32) as u32;
        ct.prdt[0].reserved = 0;
        ct.prdt[0].dbc = transfer_size - 1;

        // Fence: ensure command table writes are visible before doorbell
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.port_write(port_info.port_num, PORT_SACT, 1 << slot);
        self.port_write(port_info.port_num, PORT_CI, 1 << slot);

        Ok(slot)
    }

    /// Wait for a specific NCQ command to complete
    fn wait_ncq_slot(&self, port: u8, slot: u32) -> Result<(), &'static str> {
        for _ in 0..1_000_000u32 {
            let sact = self.port_read(port, PORT_SACT);
            if sact & (1 << slot) == 0 {
                // Check for errors
                let is = self.port_read(port, PORT_IS);
                if is & PORT_IS_TFES != 0 {
                    let tfd = self.port_read(port, PORT_TFD);
                    let _error = ((tfd >> 8) & 0xFF) as u8;
                    self.port_write(port, PORT_IS, PORT_IS_TFES);
                    return Err("NCQ task file error");
                }
                self.port_write(port, PORT_IS, is);
                return Ok(());
            }

            let is = self.port_read(port, PORT_IS);
            if is & PORT_IS_TFES != 0 {
                self.port_write(port, PORT_IS, PORT_IS_TFES);
                return Err("NCQ error during wait");
            }
            core::hint::spin_loop();
        }
        Err("NCQ command timeout")
    }

    /// Port multiplier: read a register from a device behind a port multiplier
    fn pm_read_register(&self, port: u8, pm_port: u8, register: u8) -> Result<u32, &'static str> {
        // Use a D2H Register FIS to access PM registers
        // This is done via the command slot mechanism with a special FIS
        // For now, we access the PM control registers via SError/SControl
        // PM devices are addressed by setting bits 3:0 of FIS byte 1 to the PM port

        // Read SCR register from PM port
        // PM registers are accessed via READ LOG EXT or SET FEATURES
        let _ = (port, pm_port, register);
        Err("port multiplier register access not yet implemented")
    }

    /// Check if a port has a port multiplier attached
    fn has_port_multiplier(&self, port: u8) -> bool {
        let sig = self.port_read(port, PORT_SIG);
        sig == SATA_SIG_PM
    }

    /// Get the number of ports behind a port multiplier
    fn pm_port_count(&self, _port: u8) -> u8 {
        // A port multiplier can have up to 15 device ports (0-14) + 1 control port (15)
        // The actual count is read from the PM's GSCR[2] register
        // For now, return the maximum
        15
    }

    /// Get detailed port status information
    fn port_status_detail(&self, port: u8) -> (u8, u8, u8) {
        let ssts = self.port_read(port, PORT_SSTS);
        let det = (ssts & 0x0F) as u8; // Device detection
        let spd = ((ssts >> 4) & 0x0F) as u8; // Speed negotiated
        let ipm = ((ssts >> 8) & 0x0F) as u8; // Interface power management

        (det, spd, ipm)
    }

    /// Get SATA link speed string
    fn link_speed_string(spd: u8) -> &'static str {
        match spd {
            1 => "1.5 Gb/s (Gen1)",
            2 => "3.0 Gb/s (Gen2)",
            3 => "6.0 Gb/s (Gen3)",
            _ => "unknown",
        }
    }

    /// Enable aggressive link power management
    fn enable_alpm(&self, port: u8) {
        // Set the Aggressive Link Power Management Policy via SControl
        let sctl = self.port_read(port, PORT_SCTL);
        // IPM bits (11:8): allow PARTIAL and SLUMBER
        let new_sctl = (sctl & !0xF00) | 0x300;
        self.port_write(port, PORT_SCTL, new_sctl);
    }

    /// Disable aggressive link power management (force active)
    fn disable_alpm(&self, port: u8) {
        let sctl = self.port_read(port, PORT_SCTL);
        let new_sctl = sctl & !0xF00;
        self.port_write(port, PORT_SCTL, new_sctl);
    }

    /// Get command slot usage for a port
    fn slot_usage(&self, port: u8) -> (u32, u32) {
        let ci = self.port_read(port, PORT_CI);
        let sact = self.port_read(port, PORT_SACT);
        let busy = ci | sact;
        let in_use = busy.count_ones();
        let total = self.num_cmd_slots as u32;
        (in_use, total)
    }
}

static AHCI: Mutex<Option<AhciController>> = Mutex::new(None);

/// Initialize AHCI by scanning PCI for SATA controllers
pub fn init() -> bool {
    // Look for AHCI controller on PCI bus
    // Class 0x01 (Mass Storage), Subclass 0x06 (SATA), Prog IF 0x01 (AHCI)
    let sata_devices = crate::drivers::pci::find_by_class(0x01, 0x06);
    let dev = match sata_devices.first() {
        Some(d) => d.clone(),
        None => return false,
    };

    let (bar5, is_mmio) = crate::drivers::pci::read_bar(dev.bus, dev.device, dev.function, 5);
    if bar5 == 0 || !is_mmio {
        return false;
    }

    serial_println!(
        "  AHCI: controller at PCI {} BAR5={:#x}",
        dev.bdf_string(),
        bar5
    );

    // Enable bus mastering and memory access
    crate::drivers::pci::enable_bus_master(dev.bus, dev.device, dev.function);
    crate::drivers::pci::enable_memory_space(dev.bus, dev.device, dev.function);

    // Prefer MSI over legacy pin-based interrupts.
    // AHCI vector 0x32 (IDT entry 50) — adjust to match your IDT allocation.
    const AHCI_IRQ_VECTOR: u8 = 0x32;
    if !crate::drivers::pci_msi::try_upgrade_to_msi(
        dev.bus,
        dev.device,
        dev.function,
        0, // apic_id 0 = bootstrap CPU
        AHCI_IRQ_VECTOR,
        "ahci",
    ) {
        serial_println!(
            "  AHCI: MSI not available, using legacy IRQ {}",
            dev.interrupt_line
        );
    }

    // Map BAR5 region (typically 4KB-8KB)
    let mmio_base = bar5 as usize;
    let map_size = 8 * 1024;
    let pages = (map_size + 0xFFF) / 0x1000;
    for i in 0..pages {
        let page = mmio_base + i * 0x1000;
        let flags = crate::memory::paging::flags::WRITABLE | crate::memory::paging::flags::NO_CACHE;
        let _ = crate::memory::paging::map_page(page, page, flags);
    }

    let mut ctrl = AhciController {
        base_addr: mmio_base,
        ports: Vec::new(),
        num_ports: 0,
        num_cmd_slots: 0,
        supports_64bit: false,
        supports_ncq: false,
    };

    let cap = ctrl.read32(HBA_CAP);
    ctrl.num_ports = ((cap & 0x1F) + 1) as u8;
    ctrl.num_cmd_slots = (((cap >> 8) & 0x1F) + 1) as u8;
    ctrl.supports_64bit = (cap & (1 << 31)) != 0;
    ctrl.supports_ncq = (cap & (1 << 30)) != 0;

    let ver = ctrl.read32(HBA_VS);
    serial_println!(
        "  AHCI: version {}.{}, {} ports, {} cmd slots, 64bit={}, NCQ={}",
        ver >> 16,
        ver & 0xFFFF,
        ctrl.num_ports,
        ctrl.num_cmd_slots,
        ctrl.supports_64bit,
        ctrl.supports_ncq
    );

    // Enable AHCI mode
    let ghc = ctrl.read32(HBA_GHC);
    ctrl.write32(HBA_GHC, ghc | GHC_AE);

    // Enable global interrupts
    let ghc = ctrl.read32(HBA_GHC);
    ctrl.write32(HBA_GHC, ghc | GHC_IE);

    ctrl.probe_ports();
    let port_count = ctrl.ports.len();

    *AHCI.lock() = Some(ctrl);

    if port_count > 0 {
        super::register("ahci-sata", super::DeviceType::Storage);
        serial_println!("  AHCI: {} SATA device(s) found", port_count);
        return true;
    }

    false
}

/// List detected AHCI ports
pub fn ports() -> Vec<AhciPort> {
    AHCI.lock()
        .as_ref()
        .map(|c| c.ports.clone())
        .unwrap_or_default()
}

/// Read sectors from an AHCI port
pub fn read_sectors(port: u8, lba: u64, count: u16, buf: &mut [u8]) -> Result<(), &'static str> {
    if buf.len() < (count as usize) * SECTOR_SIZE {
        return Err("buffer too small");
    }

    let ahci = AHCI.lock();
    let ctrl = ahci.as_ref().ok_or("AHCI not initialized")?;

    let port_info = ctrl
        .ports
        .iter()
        .find(|p| p.port_num == port)
        .ok_or("port not found")?;

    if !port_info.initialized {
        return Err("port not initialized");
    }

    // Allocate a DMA buffer for the transfer
    let total_bytes = count as usize * SECTOR_SIZE;
    let _frames_needed = (total_bytes + FRAME_SIZE - 1) / FRAME_SIZE;
    let dma_frame = frame_allocator::allocate_frame().ok_or("out of memory")?;
    unsafe {
        core::ptr::write_bytes(dma_frame.addr as *mut u8, 0, FRAME_SIZE);
    }

    // For transfers larger than one frame, we'd need scatter-gather.
    // For now, limit to one frame worth of sectors at a time.
    let max_sectors_per_frame = (FRAME_SIZE / SECTOR_SIZE) as u16;
    let mut remaining = count;
    let mut current_lba = lba;
    let mut buf_offset = 0usize;

    while remaining > 0 {
        let batch = remaining.min(max_sectors_per_frame);

        ctrl.read_dma(port_info, current_lba, batch, dma_frame.addr)?;

        let bytes = batch as usize * SECTOR_SIZE;
        let end_offset = buf_offset.saturating_add(bytes);
        if end_offset > buf.len() {
            return Err("read buffer overflow");
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                dma_frame.addr as *const u8,
                buf[buf_offset..end_offset].as_mut_ptr(),
                bytes,
            );
        }

        remaining = remaining.saturating_sub(batch);
        current_lba = current_lba.saturating_add(batch as u64);
        buf_offset = end_offset;
    }

    Ok(())
}

/// Write sectors to an AHCI port
pub fn write_sectors(port: u8, lba: u64, count: u16, data: &[u8]) -> Result<(), &'static str> {
    if data.len() < (count as usize) * SECTOR_SIZE {
        return Err("data too small");
    }

    let ahci = AHCI.lock();
    let ctrl = ahci.as_ref().ok_or("AHCI not initialized")?;

    let port_info = ctrl
        .ports
        .iter()
        .find(|p| p.port_num == port)
        .ok_or("port not found")?;

    if !port_info.initialized {
        return Err("port not initialized");
    }

    let dma_frame = frame_allocator::allocate_frame().ok_or("out of memory")?;

    let max_sectors_per_frame = (FRAME_SIZE / SECTOR_SIZE) as u16;
    let mut remaining = count;
    let mut current_lba = lba;
    let mut data_offset = 0usize;

    while remaining > 0 {
        let batch = remaining.min(max_sectors_per_frame);
        let bytes = batch as usize * SECTOR_SIZE;
        let end_offset = data_offset.saturating_add(bytes);
        if end_offset > data.len() {
            return Err("write data buffer overflow");
        }

        unsafe {
            core::ptr::copy_nonoverlapping(
                data[data_offset..end_offset].as_ptr(),
                dma_frame.addr as *mut u8,
                bytes,
            );
        }

        ctrl.write_dma(port_info, current_lba, batch, dma_frame.addr)?;

        remaining = remaining.saturating_sub(batch);
        current_lba = current_lba.saturating_add(batch as u64);
        data_offset = end_offset;
    }

    Ok(())
}

/// Handle AHCI interrupt
pub fn handle_interrupt() {
    if let Some(ref ctrl) = *AHCI.lock() {
        ctrl.handle_interrupt();
    }
}

/// Perform COMRESET on a port to recover from errors
pub fn reset_port(port: u8) -> Result<(), &'static str> {
    let ahci = AHCI.lock();
    let ctrl = ahci.as_ref().ok_or("AHCI not initialized")?;
    ctrl.comreset(port);
    Ok(())
}

/// Read sectors using NCQ (Native Command Queuing) for higher performance
/// NCQ allows multiple outstanding commands for the drive to reorder internally
pub fn read_sectors_ncq(
    port: u8,
    lba: u64,
    count: u16,
    buf: &mut [u8],
) -> Result<(), &'static str> {
    if buf.len() < (count as usize) * SECTOR_SIZE {
        return Err("buffer too small");
    }

    let ahci = AHCI.lock();
    let ctrl = ahci.as_ref().ok_or("AHCI not initialized")?;

    if !ctrl.supports_ncq {
        return Err("NCQ not supported by this controller");
    }

    let port_info = ctrl
        .ports
        .iter()
        .find(|p| p.port_num == port)
        .ok_or("port not found")?;

    if !port_info.initialized {
        return Err("port not initialized");
    }

    // Allocate DMA buffer
    let dma_frame = frame_allocator::allocate_frame().ok_or("out of memory")?;
    unsafe {
        core::ptr::write_bytes(dma_frame.addr as *mut u8, 0, FRAME_SIZE);
    }

    let max_sectors_per_frame = (FRAME_SIZE / SECTOR_SIZE) as u16;
    let mut remaining = count;
    let mut current_lba = lba;
    let mut buf_offset = 0usize;

    while remaining > 0 {
        let batch = remaining.min(max_sectors_per_frame);

        let slot = ctrl.issue_ncq_read(port_info, current_lba, batch, dma_frame.addr)?;
        ctrl.wait_ncq_slot(port, slot)?;

        let bytes = batch as usize * SECTOR_SIZE;
        let end_offset = buf_offset.saturating_add(bytes);
        if end_offset > buf.len() {
            return Err("NCQ read buffer overflow");
        }
        unsafe {
            core::ptr::copy_nonoverlapping(
                dma_frame.addr as *const u8,
                buf[buf_offset..end_offset].as_mut_ptr(),
                bytes,
            );
        }

        remaining = remaining.saturating_sub(batch);
        current_lba = current_lba.saturating_add(batch as u64);
        buf_offset = end_offset;
    }

    Ok(())
}

/// Write sectors using NCQ for higher performance
pub fn write_sectors_ncq(port: u8, lba: u64, count: u16, data: &[u8]) -> Result<(), &'static str> {
    if data.len() < (count as usize) * SECTOR_SIZE {
        return Err("data too small");
    }

    let ahci = AHCI.lock();
    let ctrl = ahci.as_ref().ok_or("AHCI not initialized")?;

    if !ctrl.supports_ncq {
        return Err("NCQ not supported by this controller");
    }

    let port_info = ctrl
        .ports
        .iter()
        .find(|p| p.port_num == port)
        .ok_or("port not found")?;

    if !port_info.initialized {
        return Err("port not initialized");
    }

    let dma_frame = frame_allocator::allocate_frame().ok_or("out of memory")?;

    let max_sectors_per_frame = (FRAME_SIZE / SECTOR_SIZE) as u16;
    let mut remaining = count;
    let mut current_lba = lba;
    let mut data_offset = 0usize;

    while remaining > 0 {
        let batch = remaining.min(max_sectors_per_frame);
        let bytes = batch as usize * SECTOR_SIZE;
        let end_offset = data_offset.saturating_add(bytes);
        if end_offset > data.len() {
            return Err("NCQ write data buffer overflow");
        }

        unsafe {
            core::ptr::copy_nonoverlapping(
                data[data_offset..end_offset].as_ptr(),
                dma_frame.addr as *mut u8,
                bytes,
            );
        }

        let slot = ctrl.issue_ncq_write(port_info, current_lba, batch, dma_frame.addr)?;
        ctrl.wait_ncq_slot(port, slot)?;

        remaining = remaining.saturating_sub(batch);
        current_lba = current_lba.saturating_add(batch as u64);
        data_offset = end_offset;
    }

    Ok(())
}

/// Check if NCQ is supported by the AHCI controller
pub fn supports_ncq() -> bool {
    AHCI.lock()
        .as_ref()
        .map(|c| c.supports_ncq)
        .unwrap_or(false)
}

/// Get command slot usage for a port (in_use, total)
pub fn slot_usage(port: u8) -> Option<(u32, u32)> {
    AHCI.lock().as_ref().map(|ctrl| ctrl.slot_usage(port))
}

/// Get detailed SATA link status for a port
/// Returns (detection_state, speed, power_management_state)
pub fn link_status(port: u8) -> Option<(u8, u8, u8)> {
    AHCI.lock()
        .as_ref()
        .map(|ctrl| ctrl.port_status_detail(port))
}

/// Get SATA link speed description for a port
pub fn link_speed(port: u8) -> Option<&'static str> {
    AHCI.lock().as_ref().map(|ctrl| {
        let (_, spd, _) = ctrl.port_status_detail(port);
        AhciController::link_speed_string(spd)
    })
}

/// Flush cache on an AHCI port
pub fn flush_cache(port: u8) -> Result<(), &'static str> {
    let ahci = AHCI.lock();
    let ctrl = ahci.as_ref().ok_or("AHCI not initialized")?;
    let port_info = ctrl
        .ports
        .iter()
        .find(|p| p.port_num == port)
        .ok_or("port not found")?;
    ctrl.flush(port_info)
}

/// Get HBA capabilities as a readable summary
pub fn capabilities() -> Option<(u8, u8, bool, bool)> {
    AHCI.lock().as_ref().map(|ctrl| {
        (
            ctrl.num_ports,
            ctrl.num_cmd_slots,
            ctrl.supports_64bit,
            ctrl.supports_ncq,
        )
    })
}
