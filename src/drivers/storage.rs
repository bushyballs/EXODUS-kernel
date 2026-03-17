use crate::sync::Mutex;
/// Storage driver interfaces for Genesis
///
/// Provides a unified block device abstraction over multiple storage backends:
///   - AHCI (SATA drives via HBA memory-mapped registers)
///   - NVMe (modern SSDs via submission/completion queues)
///   - Virtio-blk (virtual machine paravirtualized disks)
///   - RAM disk (memory-backed block device for testing)
///
/// The StorageManager maintains a registry of all discovered block devices
/// and routes I/O requests to the appropriate driver.
///
/// Inspired by: Linux block layer (include/linux/blkdev.h), but simplified.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Storage types
// ---------------------------------------------------------------------------

/// Storage device type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageType {
    Ahci,      // SATA via AHCI controller
    Nvme,      // NVMe SSD
    VirtioBlk, // VM virtual disk
    RamDisk,   // RAM-backed storage
    UsbMass,   // USB mass storage
}

impl StorageType {
    pub fn name(&self) -> &'static str {
        match self {
            StorageType::Ahci => "AHCI/SATA",
            StorageType::Nvme => "NVMe",
            StorageType::VirtioBlk => "Virtio-blk",
            StorageType::RamDisk => "RAM Disk",
            StorageType::UsbMass => "USB Mass Storage",
        }
    }
}

/// Block I/O error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    /// Device not found
    NotFound,
    /// Offset out of range
    OutOfRange,
    /// Device not ready or not initialized
    NotReady,
    /// Hardware error during transfer
    HardwareError,
    /// DMA error
    DmaError,
    /// Timeout waiting for device
    Timeout,
    /// Device is read-only
    ReadOnly,
    /// Generic I/O error
    IoError,
}

/// Storage device information
#[derive(Debug, Clone)]
pub struct StorageDeviceInfo {
    /// Device name (e.g., "sda", "nvme0n1")
    pub name: String,
    /// Model string from device identification
    pub model: String,
    /// Serial number
    pub serial: String,
    /// Firmware revision
    pub firmware: String,
    /// Total capacity in bytes
    pub capacity_bytes: u64,
    /// Logical block size (usually 512 or 4096)
    pub block_size: u32,
    /// Total number of logical blocks
    pub block_count: u64,
    /// Device type
    pub device_type: StorageType,
    /// Whether the device is read-only
    pub read_only: bool,
    /// Whether the device supports TRIM/discard
    pub supports_trim: bool,
    /// Maximum transfer size in blocks per operation
    pub max_transfer_blocks: u32,
}

impl StorageDeviceInfo {
    /// Capacity in human-readable format
    pub fn capacity_string(&self) -> String {
        let bytes = self.capacity_bytes;
        if bytes >= 1_099_511_627_776 {
            alloc::format!("{} TB", bytes / 1_099_511_627_776)
        } else if bytes >= 1_073_741_824 {
            alloc::format!("{} GB", bytes / 1_073_741_824)
        } else if bytes >= 1_048_576 {
            alloc::format!("{} MB", bytes / 1_048_576)
        } else {
            alloc::format!("{} KB", bytes / 1024)
        }
    }
}

// ---------------------------------------------------------------------------
// AHCI register definitions
// ---------------------------------------------------------------------------

/// AHCI HBA Memory Register offsets (from ABAR)
#[allow(dead_code)]
mod ahci_regs {
    // Generic Host Control registers
    pub const HBA_CAP: u32 = 0x00; // Host Capabilities
    pub const HBA_GHC: u32 = 0x04; // Global Host Control
    pub const HBA_IS: u32 = 0x08; // Interrupt Status
    pub const HBA_PI: u32 = 0x0C; // Ports Implemented
    pub const HBA_VS: u32 = 0x10; // Version
    pub const HBA_CCC_CTL: u32 = 0x14; // Command Completion Coalescing Control
    pub const HBA_CCC_PORTS: u32 = 0x18; // CCC Ports
    pub const HBA_CAP2: u32 = 0x24; // Host Capabilities Extended

    // GHC bits
    pub const GHC_HR: u32 = 1 << 0; // HBA Reset
    pub const GHC_IE: u32 = 1 << 1; // Interrupt Enable
    pub const GHC_AE: u32 = 1 << 31; // AHCI Enable

    // Port register offsets (from port base = ABAR + 0x100 + port * 0x80)
    pub const PORT_CLB: u32 = 0x00; // Command List Base Address
    pub const PORT_CLBU: u32 = 0x04; // Command List Base Address Upper
    pub const PORT_FB: u32 = 0x08; // FIS Base Address
    pub const PORT_FBU: u32 = 0x0C; // FIS Base Address Upper
    pub const PORT_IS: u32 = 0x10; // Interrupt Status
    pub const PORT_IE: u32 = 0x14; // Interrupt Enable
    pub const PORT_CMD: u32 = 0x18; // Command and Status
    pub const PORT_TFD: u32 = 0x20; // Task File Data
    pub const PORT_SIG: u32 = 0x24; // Signature
    pub const PORT_SSTS: u32 = 0x28; // SATA Status (SCR0: SStatus)
    pub const PORT_SCTL: u32 = 0x2C; // SATA Control (SCR2: SControl)
    pub const PORT_SERR: u32 = 0x30; // SATA Error (SCR1: SError)
    pub const PORT_SACT: u32 = 0x34; // SATA Active (SCR3: SActive)
    pub const PORT_CI: u32 = 0x38; // Command Issue

    // Port CMD bits
    pub const CMD_ST: u32 = 1 << 0; // Start
    pub const CMD_SUD: u32 = 1 << 1; // Spin-Up Device
    pub const CMD_POD: u32 = 1 << 2; // Power On Device
    pub const CMD_FRE: u32 = 1 << 4; // FIS Receive Enable
    pub const CMD_FR: u32 = 1 << 14; // FIS Receive Running
    pub const CMD_CR: u32 = 1 << 15; // Command List Running

    // Port SSTS (SStatus) fields
    pub const SSTS_DET_MASK: u32 = 0xF;
    pub const SSTS_DET_PRESENT: u32 = 3; // Device detected and communication established
    pub const SSTS_SPD_MASK: u32 = 0xF0;

    // Device signature values
    pub const SIG_ATA: u32 = 0x00000101; // SATA drive
    pub const SIG_ATAPI: u32 = 0xEB140101; // SATAPI device
    pub const SIG_SEMB: u32 = 0xC33C0101; // Enclosure management bridge
    pub const SIG_PM: u32 = 0x96690101; // Port multiplier

    /// Port base address given ABAR and port number
    pub fn port_base(abar: u64, port: u8) -> u64 {
        abar.saturating_add(0x100)
            .saturating_add((port as u64).saturating_mul(0x80))
    }
}

// ---------------------------------------------------------------------------
// AHCI driver state
// ---------------------------------------------------------------------------

/// Information about a single AHCI port with an attached device
#[derive(Debug, Clone)]
pub struct AhciPort {
    /// Port number (0-31)
    pub port_num: u8,
    /// Device signature
    pub signature: u32,
    /// Whether device is present and communication established
    pub device_present: bool,
    /// SATA link speed (1 = Gen1/1.5Gbps, 2 = Gen2/3Gbps, 3 = Gen3/6Gbps)
    pub link_speed: u8,
    /// Device info (filled after IDENTIFY command)
    pub info: Option<StorageDeviceInfo>,
}

/// AHCI Host Bus Adapter driver
pub struct AhciDriver {
    /// PCI bus/device/function
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_function: u8,
    /// ABAR (AHCI Base Address Register) — BAR5 from PCI config
    pub abar: u64,
    /// Number of ports supported
    pub num_ports: u8,
    /// Number of command slots
    pub num_cmd_slots: u8,
    /// Ports implemented bitmap
    pub ports_implemented: u32,
    /// Detected ports with devices
    pub ports: Vec<AhciPort>,
    /// AHCI version
    pub version: u32,
    /// Whether 64-bit addressing is supported
    pub supports_64bit: bool,
    /// Whether NCQ (Native Command Queuing) is supported
    pub supports_ncq: bool,
}

impl AhciDriver {
    /// Create a new AHCI driver and read HBA capabilities
    pub fn new(bus: u8, device: u8, function: u8, abar: u64) -> Self {
        let cap = unsafe { core::ptr::read_volatile(abar as *const u32) };
        let pi =
            unsafe { core::ptr::read_volatile((abar + ahci_regs::HBA_PI as u64) as *const u32) };
        let vs =
            unsafe { core::ptr::read_volatile((abar + ahci_regs::HBA_VS as u64) as *const u32) };

        let num_ports = (((cap >> 0) & 0x1F) as u8).saturating_add(1);
        let num_cmd_slots = (((cap >> 8) & 0x1F) as u8).saturating_add(1);
        let supports_64bit = cap & (1 << 31) != 0;
        let supports_ncq = cap & (1 << 30) != 0;

        AhciDriver {
            pci_bus: bus,
            pci_device: device,
            pci_function: function,
            abar,
            num_ports,
            num_cmd_slots,
            ports_implemented: pi,
            ports: Vec::new(),
            version: vs,
            supports_64bit,
            supports_ncq,
        }
    }

    /// Enable AHCI mode and reset the HBA
    pub fn reset(&self) {
        // Enable AHCI mode (GHC.AE = 1)
        let ghc = unsafe {
            core::ptr::read_volatile((self.abar + ahci_regs::HBA_GHC as u64) as *const u32)
        };
        unsafe {
            core::ptr::write_volatile(
                (self.abar + ahci_regs::HBA_GHC as u64) as *mut u32,
                ghc | ahci_regs::GHC_AE,
            );
        }

        // HBA Reset (GHC.HR = 1)
        unsafe {
            core::ptr::write_volatile(
                (self.abar + ahci_regs::HBA_GHC as u64) as *mut u32,
                ghc | ahci_regs::GHC_AE | ahci_regs::GHC_HR,
            );
        }

        // Wait for HR to clear
        for _ in 0..100_000 {
            let val = unsafe {
                core::ptr::read_volatile((self.abar + ahci_regs::HBA_GHC as u64) as *const u32)
            };
            if val & ahci_regs::GHC_HR == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Re-enable AHCI mode after reset
        unsafe {
            core::ptr::write_volatile(
                (self.abar + ahci_regs::HBA_GHC as u64) as *mut u32,
                ahci_regs::GHC_AE,
            );
        }
    }

    /// Scan all implemented ports for attached devices
    pub fn scan_ports(&mut self) {
        for port in 0..32u8 {
            if self.ports_implemented & (1 << port) == 0 {
                continue;
            }

            let base = ahci_regs::port_base(self.abar, port);
            let ssts = unsafe {
                core::ptr::read_volatile((base + ahci_regs::PORT_SSTS as u64) as *const u32)
            };
            let sig = unsafe {
                core::ptr::read_volatile((base + ahci_regs::PORT_SIG as u64) as *const u32)
            };

            let det = ssts & ahci_regs::SSTS_DET_MASK;
            let spd = ((ssts & ahci_regs::SSTS_SPD_MASK) >> 4) as u8;
            let device_present = det == ahci_regs::SSTS_DET_PRESENT;

            if device_present {
                let dev_type = match sig {
                    ahci_regs::SIG_ATA => "SATA disk",
                    ahci_regs::SIG_ATAPI => "SATAPI device",
                    ahci_regs::SIG_SEMB => "Enclosure management",
                    ahci_regs::SIG_PM => "Port multiplier",
                    _ => "unknown",
                };
                let speed_str = match spd {
                    1 => "1.5 Gbps (Gen1)",
                    2 => "3.0 Gbps (Gen2)",
                    3 => "6.0 Gbps (Gen3)",
                    _ => "unknown",
                };
                serial_println!(
                    "  Storage: AHCI port {}: {} at {}",
                    port,
                    dev_type,
                    speed_str
                );

                self.ports.push(AhciPort {
                    port_num: port,
                    signature: sig,
                    device_present: true,
                    link_speed: spd,
                    info: None,
                });
            }
        }
    }

    /// Stop a port's command engine (must be done before setting up CLB/FB)
    pub fn stop_port(&self, port: u8) {
        let base = ahci_regs::port_base(self.abar, port);
        let cmd =
            unsafe { core::ptr::read_volatile((base + ahci_regs::PORT_CMD as u64) as *const u32) };

        // Clear ST (stop command processing) and FRE (stop FIS receive)
        unsafe {
            core::ptr::write_volatile(
                (base + ahci_regs::PORT_CMD as u64) as *mut u32,
                cmd & !(ahci_regs::CMD_ST | ahci_regs::CMD_FRE),
            );
        }

        // Wait for CR (Command List Running) and FR (FIS Receive Running) to clear
        for _ in 0..500_000 {
            let val = unsafe {
                core::ptr::read_volatile((base + ahci_regs::PORT_CMD as u64) as *const u32)
            };
            if val & (ahci_regs::CMD_CR | ahci_regs::CMD_FR) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
        serial_println!("  Storage: AHCI port {} stop timed out", port);
    }

    /// Start a port's command engine after setting up CLB/FB
    pub fn start_port(&self, port: u8) {
        let base = ahci_regs::port_base(self.abar, port);

        // Wait until CR is clear
        for _ in 0..500_000 {
            let cmd = unsafe {
                core::ptr::read_volatile((base + ahci_regs::PORT_CMD as u64) as *const u32)
            };
            if cmd & ahci_regs::CMD_CR == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Set FRE, then ST
        let cmd =
            unsafe { core::ptr::read_volatile((base + ahci_regs::PORT_CMD as u64) as *const u32) };
        unsafe {
            core::ptr::write_volatile(
                (base + ahci_regs::PORT_CMD as u64) as *mut u32,
                cmd | ahci_regs::CMD_FRE,
            );
        }
        // Small delay for FRE to take effect
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
        let cmd =
            unsafe { core::ptr::read_volatile((base + ahci_regs::PORT_CMD as u64) as *const u32) };
        unsafe {
            core::ptr::write_volatile(
                (base + ahci_regs::PORT_CMD as u64) as *mut u32,
                cmd | ahci_regs::CMD_ST,
            );
        }
    }

    /// Initialize a port: allocate and set up Command List Base and FIS Base,
    /// clear SERR, and enable the port for command processing.
    ///
    /// Each port requires:
    ///   - Command List: 32 command headers * 32 bytes = 1024 bytes (1 KB), 1 KB aligned
    ///   - FIS Receive Area: 256 bytes, 256-byte aligned
    ///   - Command Table per slot: 128 bytes header + PRDT entries
    ///
    /// We allocate a single 4 KB frame that holds:
    ///   offset 0x000..0x400: Command List (32 headers)
    ///   offset 0x400..0x500: FIS Receive Area (256 bytes)
    ///   offset 0x500..0x580: Command Table for slot 0 (128 bytes, minimal)
    pub fn init_port(&self, port: u8) -> bool {
        let base = ahci_regs::port_base(self.abar, port);

        // Stop port first
        self.stop_port(port);

        // Allocate one physical frame (4 KB) for CLB + FB + one command table
        let frame = crate::memory::frame_allocator::allocate_frames(1);
        if frame == 0 {
            serial_println!(
                "  Storage: AHCI port {} — failed to allocate DMA frame",
                port
            );
            return false;
        }

        // Identity-map the DMA frame with WRITABLE | NO_CACHE
        let flags = crate::memory::paging::flags::WRITABLE | crate::memory::paging::flags::NO_CACHE;
        let _ = crate::memory::paging::map_page(frame, frame, flags);

        // Zero the entire 4 KB frame
        unsafe {
            core::ptr::write_bytes(frame as *mut u8, 0, 4096);
        }

        let clb_phys = frame as u64; // Command List Base: offset 0x000
        let fb_phys = frame as u64 + 0x400; // FIS Base: offset 0x400
        let ct0_phys = frame as u64 + 0x500; // Command Table 0: offset 0x500

        // Set Command List Base Address (CLB + CLBU)
        unsafe {
            core::ptr::write_volatile(
                (base + ahci_regs::PORT_CLB as u64) as *mut u32,
                clb_phys as u32,
            );
            core::ptr::write_volatile(
                (base + ahci_regs::PORT_CLBU as u64) as *mut u32,
                (clb_phys >> 32) as u32,
            );
        }

        // Set FIS Base Address (FB + FBU)
        unsafe {
            core::ptr::write_volatile(
                (base + ahci_regs::PORT_FB as u64) as *mut u32,
                fb_phys as u32,
            );
            core::ptr::write_volatile(
                (base + ahci_regs::PORT_FBU as u64) as *mut u32,
                (fb_phys >> 32) as u32,
            );
        }

        // Set up Command Header 0 to point to Command Table 0
        // Command Header format (32 bytes):
        //   DW0: [15:0] PRDTL (entries) | [4:0] CFL (command FIS length in DWORDs)
        //        bit 6: W=write, bit 5: A=ATAPI, bit 7: P=prefetchable
        //   DW1: PRDBC (bytes transferred)
        //   DW2: CTBA (Command Table Base Address, lower 32)
        //   DW3: CTBAU (upper 32)
        //   DW4-7: reserved
        let header0 = clb_phys as *mut u32;
        unsafe {
            // DW0: CFL=5 (H2D Register FIS is 5 DWORDs = 20 bytes), PRDTL=0 for now
            core::ptr::write_volatile(header0, 5);
            // DW1: PRDBC = 0
            core::ptr::write_volatile(header0.add(1), 0);
            // DW2: CTBA lower
            core::ptr::write_volatile(header0.add(2), ct0_phys as u32);
            // DW3: CTBAU upper
            core::ptr::write_volatile(header0.add(3), (ct0_phys >> 32) as u32);
        }

        // Clear SATA Error register
        unsafe {
            core::ptr::write_volatile(
                (base + ahci_regs::PORT_SERR as u64) as *mut u32,
                0xFFFFFFFF, // write 1 to clear all error bits
            );
        }

        // Clear port Interrupt Status
        unsafe {
            core::ptr::write_volatile((base + ahci_regs::PORT_IS as u64) as *mut u32, 0xFFFFFFFF);
        }

        // Start the port
        self.start_port(port);

        serial_println!(
            "  Storage: AHCI port {} initialized (CLB={:#x}, FB={:#x})",
            port,
            clb_phys,
            fb_phys
        );
        true
    }

    /// Issue an ATA IDENTIFY DEVICE (0xEC) command on the given port
    /// and extract model, serial, firmware, and capacity.
    ///
    /// Returns a StorageDeviceInfo if the command succeeds.
    pub fn identify_device(&self, port: u8) -> Option<StorageDeviceInfo> {
        let base = ahci_regs::port_base(self.abar, port);

        // Read CLB to find where command structures live
        let clb_lo =
            unsafe { core::ptr::read_volatile((base + ahci_regs::PORT_CLB as u64) as *const u32) };
        let clb_hi =
            unsafe { core::ptr::read_volatile((base + ahci_regs::PORT_CLBU as u64) as *const u32) };
        let clb = (clb_hi as u64) << 32 | clb_lo as u64;
        if clb == 0 {
            serial_println!(
                "  Storage: AHCI port {} — CLB not set, port not initialized",
                port
            );
            return None;
        }

        // Allocate a 4 KB frame for the IDENTIFY result buffer (512 bytes needed)
        let data_frame = crate::memory::frame_allocator::allocate_frames(1);
        if data_frame == 0 {
            serial_println!(
                "  Storage: AHCI port {} — failed to allocate IDENTIFY buffer",
                port
            );
            return None;
        }
        let flags = crate::memory::paging::flags::WRITABLE | crate::memory::paging::flags::NO_CACHE;
        let _ = crate::memory::paging::map_page(data_frame, data_frame, flags);
        unsafe {
            core::ptr::write_bytes(data_frame as *mut u8, 0, 4096);
        }

        let data_phys = data_frame as u64;

        // Read Command Table base from header 0
        let header0 = clb as *mut u32;
        let ct_lo = unsafe { core::ptr::read_volatile(header0.add(2)) };
        let ct_hi = unsafe { core::ptr::read_volatile(header0.add(3)) };
        let ct_base = (ct_hi as u64) << 32 | ct_lo as u64;
        if ct_base == 0 {
            return None;
        }

        // Set up Command Header 0:
        //   CFL=5 (H2D FIS = 5 DWORDs), PRDTL=1 (one PRD entry), clear W bit
        unsafe {
            core::ptr::write_volatile(header0, 5 | (1 << 16)); // CFL=5, PRDTL=1
            core::ptr::write_volatile(header0.add(1), 0); // Clear PRDBC
        }

        // Set up Command Table at ct_base:
        //   Bytes 0..63: Command FIS (H2D Register FIS)
        //   Bytes 64..79: ATAPI command (unused for ATA)
        //   Bytes 80..127: reserved
        //   Bytes 128+: PRDT entries (16 bytes each)
        let ct = ct_base as *mut u32;

        // H2D Register FIS (FIS type = 0x27, C bit = 1)
        // DW0: [7:0]=FIS type, [15:8]=PM port | C bit, [23:16]=command, [31:24]=features
        // DW1: [7:0]=LBA low, [15:8]=LBA mid, [23:16]=LBA high, [31:24]=device
        // DW2: [7:0]=LBA(3), [15:8]=LBA(4), [23:16]=LBA(5), [31:24]=features(exp)
        // DW3: [15:0]=count, [31:16]=reserved
        // DW4: reserved
        unsafe {
            // DW0: FIS_TYPE_REG_H2D=0x27, C=1 (bit 7 of byte 1), command=0xEC (IDENTIFY)
            core::ptr::write_volatile(ct, 0x00EC_8027);
            // DW1: device = 0
            core::ptr::write_volatile(ct.add(1), 0);
            // DW2: all zero
            core::ptr::write_volatile(ct.add(2), 0);
            // DW3: count = 0 (IDENTIFY doesn't use count)
            core::ptr::write_volatile(ct.add(3), 0);
            // DW4: reserved
            core::ptr::write_volatile(ct.add(4), 0);
        }

        // PRDT entry 0 (at ct_base + 0x80, 16 bytes per entry)
        // DW0: Data Base Address (lower 32)
        // DW1: Data Base Address (upper 32)
        // DW2: reserved
        // DW3: [21:0]=byte count - 1, bit 31=interrupt on completion
        let prdt = (ct_base + 0x80) as *mut u32;
        unsafe {
            core::ptr::write_volatile(prdt, data_phys as u32);
            core::ptr::write_volatile(prdt.add(1), (data_phys >> 32) as u32);
            core::ptr::write_volatile(prdt.add(2), 0);
            core::ptr::write_volatile(prdt.add(3), 511 | (1 << 31)); // 512 bytes - 1, IOC=1
        }

        // Clear port interrupt status
        unsafe {
            core::ptr::write_volatile((base + ahci_regs::PORT_IS as u64) as *mut u32, 0xFFFFFFFF);
        }

        // Issue command in slot 0: set bit 0 in CI register
        unsafe {
            core::ptr::write_volatile((base + ahci_regs::PORT_CI as u64) as *mut u32, 1);
        }

        // Poll for completion: wait until CI bit 0 clears or TFD indicates error
        let mut success = false;
        for _ in 0..1_000_000 {
            let ci = unsafe {
                core::ptr::read_volatile((base + ahci_regs::PORT_CI as u64) as *const u32)
            };
            if ci & 1 == 0 {
                // Command completed
                let tfd = unsafe {
                    core::ptr::read_volatile((base + ahci_regs::PORT_TFD as u64) as *const u32)
                };
                // TFD bits [0]=ERR, [3]=DRQ; both should be 0 on success, bit [7]=BSY should be 0
                if tfd & 0x89 == 0 {
                    success = true;
                } else {
                    serial_println!(
                        "  Storage: AHCI port {} IDENTIFY TFD error: {:#x}",
                        port,
                        tfd
                    );
                }
                break;
            }

            // Check for errors in IS (TFES = Task File Error Status)
            let is_val = unsafe {
                core::ptr::read_volatile((base + ahci_regs::PORT_IS as u64) as *const u32)
            };
            if is_val & (1 << 30) != 0 {
                // TFES
                serial_println!("  Storage: AHCI port {} IDENTIFY task file error", port);
                break;
            }

            core::hint::spin_loop();
        }

        if !success {
            return None;
        }

        // Parse the 512-byte IDENTIFY response
        let identify = data_phys as *const u16;
        let read_word =
            |idx: usize| -> u16 { unsafe { core::ptr::read_volatile(identify.add(idx)) } };

        // Extract ATA string: words are byte-swapped (big-endian pairs)
        let extract_ata_string = |start: usize, count: usize| -> String {
            let mut s = String::with_capacity(count * 2);
            for i in 0..count {
                let word = read_word(start + i);
                let hi = (word >> 8) as u8;
                let lo = (word & 0xFF) as u8;
                if hi >= 0x20 && hi < 0x7F {
                    s.push(hi as char);
                }
                if lo >= 0x20 && lo < 0x7F {
                    s.push(lo as char);
                }
            }
            // Trim trailing spaces
            while s.ends_with(' ') {
                s.pop();
            }
            s
        };

        // Words 10-19: Serial number (20 chars)
        let serial = extract_ata_string(10, 10);
        // Words 23-26: Firmware revision (8 chars)
        let firmware = extract_ata_string(23, 4);
        // Words 27-46: Model number (40 chars)
        let model = extract_ata_string(27, 20);

        // Words 100-103: Total addressable sectors (LBA48), 48-bit value
        let lba48_lo = read_word(100) as u64 | ((read_word(101) as u64) << 16);
        let lba48_hi = read_word(102) as u64 | ((read_word(103) as u64) << 16);
        let total_sectors = lba48_lo | (lba48_hi << 32);

        // Word 106: Physical/Logical Sector Size
        let w106 = read_word(106);
        let logical_sector_size = if w106 & (1 << 12) != 0 {
            // Words 117-118 contain logical sector size in words
            let lss_words = read_word(117) as u32 | ((read_word(118) as u32) << 16);
            lss_words * 2 // Convert words to bytes
        } else {
            512u32
        };

        // Word 169: TRIM support (Data Set Management)
        let supports_trim = read_word(169) & 1 != 0;

        let capacity_bytes = total_sectors.saturating_mul(logical_sector_size as u64);

        serial_println!(
            "  Storage: AHCI port {} IDENTIFY: model='{}' serial='{}' fw='{}' sectors={} size={}",
            port,
            model,
            serial,
            firmware,
            total_sectors,
            if capacity_bytes >= 1_073_741_824 {
                alloc::format!("{} GB", capacity_bytes / 1_073_741_824)
            } else {
                alloc::format!("{} MB", capacity_bytes / 1_048_576)
            }
        );

        Some(StorageDeviceInfo {
            name: String::new(), // Caller assigns name
            model,
            serial,
            firmware,
            capacity_bytes,
            block_size: logical_sector_size,
            block_count: total_sectors,
            device_type: StorageType::Ahci,
            read_only: false,
            supports_trim,
            max_transfer_blocks: 256,
        })
    }

    /// AHCI version as a human-readable string
    pub fn version_string(&self) -> String {
        let major = (self.version >> 16) & 0xFFFF;
        let minor = self.version & 0xFFFF;
        alloc::format!("{}.{}", major, minor)
    }
}

// ---------------------------------------------------------------------------
// Storage manager (global device registry)
// ---------------------------------------------------------------------------

/// The global storage manager tracks all discovered block devices
pub struct StorageManager {
    /// All discovered storage devices
    pub devices: Vec<StorageDeviceInfo>,
    /// AHCI driver instances
    pub ahci_drivers: Vec<AhciDriver>,
    /// Next device index for naming
    next_idx: u32,
}

impl StorageManager {
    pub const fn new() -> Self {
        StorageManager {
            devices: Vec::new(),
            ahci_drivers: Vec::new(),
            next_idx: 0,
        }
    }

    /// Register a new block device
    pub fn register_device(&mut self, mut info: StorageDeviceInfo) -> u32 {
        let idx = self.next_idx;
        if info.name.is_empty() {
            info.name = alloc::format!("blk{}", idx);
        }
        self.next_idx = self.next_idx.saturating_add(1);
        serial_println!(
            "  Storage: registered {} ({}, {})",
            info.name,
            info.device_type.name(),
            info.capacity_string()
        );
        self.devices.push(info);
        idx
    }

    /// Find a device by name
    pub fn find_by_name(&self, name: &str) -> Option<&StorageDeviceInfo> {
        self.devices.iter().find(|d| d.name == name)
    }

    /// List all devices
    pub fn list(&self) -> &[StorageDeviceInfo] {
        &self.devices
    }

    /// Total number of registered devices
    pub fn count(&self) -> usize {
        self.devices.len()
    }
}

static STORAGE: Mutex<StorageManager> = Mutex::new(StorageManager::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize storage drivers — scan PCI for AHCI and NVMe controllers
pub fn init() {
    let mut mgr = STORAGE.lock();

    // Scan for AHCI controllers (class 01h, subclass 06h)
    let ahci_devices = super::pci::find_by_class(0x01, 0x06);
    for dev in &ahci_devices {
        serial_println!(
            "  Storage: AHCI controller at {} ({:04x}:{:04x})",
            dev.bdf_string(),
            dev.vendor_id,
            dev.device_id
        );

        // AHCI uses BAR5 (index 5) for ABAR
        let (abar, is_mmio) = super::pci::read_bar(dev.bus, dev.device, dev.function, 5);
        if !is_mmio || abar == 0 {
            serial_println!("  Storage: AHCI BAR5 is not valid MMIO ({:#x})", abar);
            continue;
        }

        // Map ABAR MMIO region (AHCI registers are typically within 8 KB)
        let map_size: usize = 0x2000;
        for i in 0..(map_size / 0x1000) {
            let page = abar as usize + i * 0x1000;
            let flags =
                crate::memory::paging::flags::WRITABLE | crate::memory::paging::flags::NO_CACHE;
            let _ = crate::memory::paging::map_page(page, page, flags);
        }

        let mut ahci = AhciDriver::new(dev.bus, dev.device, dev.function, abar);
        serial_println!(
            "  Storage: AHCI v{}, {} ports, {} cmd slots, 64bit={}, NCQ={}",
            ahci.version_string(),
            ahci.num_ports,
            ahci.num_cmd_slots,
            ahci.supports_64bit,
            ahci.supports_ncq
        );

        ahci.reset();
        ahci.scan_ports();

        // Initialize and identify each detected SATA drive
        let port_nums: Vec<u8> = ahci
            .ports
            .iter()
            .filter(|p| p.signature == ahci_regs::SIG_ATA)
            .map(|p| p.port_num)
            .collect();

        for port_num in &port_nums {
            if !ahci.init_port(*port_num) {
                continue;
            }

            // Issue IDENTIFY DEVICE to get model/serial/capacity
            if let Some(mut info) = ahci.identify_device(*port_num) {
                info.name = alloc::format!("sd{}", (b'a' + mgr.next_idx as u8) as char);
                mgr.register_device(info);
            } else {
                // Fallback: register with unknown details
                let ni = mgr.next_idx;
                mgr.register_device(StorageDeviceInfo {
                    name: alloc::format!("sd{}", (b'a' + ni as u8) as char),
                    model: alloc::format!("SATA Drive (port {})", port_num),
                    serial: String::new(),
                    firmware: String::new(),
                    capacity_bytes: 0,
                    block_size: 512,
                    block_count: 0,
                    device_type: StorageType::Ahci,
                    read_only: false,
                    supports_trim: false,
                    max_transfer_blocks: 256,
                });
            }
        }

        mgr.ahci_drivers.push(ahci);
    }

    // Scan for NVMe controllers (class 01h, subclass 08h)
    let nvme_devices = super::pci::find_by_class(0x01, 0x08);
    for dev in &nvme_devices {
        serial_println!(
            "  Storage: NVMe controller at {} ({:04x}:{:04x})",
            dev.bdf_string(),
            dev.vendor_id,
            dev.device_id
        );

        let (bar0, is_mmio) = super::pci::read_bar(dev.bus, dev.device, dev.function, 0);
        if !is_mmio || bar0 == 0 {
            serial_println!("  Storage: NVMe BAR0 is not valid MMIO");
            continue;
        }

        // Map NVMe BAR0 (registers + doorbell, typically 16 KB)
        let map_size: usize = 0x4000;
        for i in 0..(map_size / 0x1000) {
            let page = bar0 as usize + i * 0x1000;
            let flags =
                crate::memory::paging::flags::WRITABLE | crate::memory::paging::flags::NO_CACHE;
            let _ = crate::memory::paging::map_page(page, page, flags);
        }

        // Read NVMe registers
        let nvme_cap = unsafe { core::ptr::read_volatile(bar0 as *const u64) }; // CAP (64-bit at offset 0x00)
        let nvme_vs = unsafe { core::ptr::read_volatile((bar0 + 0x08) as *const u32) };
        let nvme_cc = unsafe { core::ptr::read_volatile((bar0 + 0x14) as *const u32) }; // CC
        let nvme_csts = unsafe { core::ptr::read_volatile((bar0 + 0x1C) as *const u32) }; // CSTS

        let vs_major = (nvme_vs >> 16) & 0xFFFF;
        let vs_minor = (nvme_vs >> 8) & 0xFF;
        let vs_tertiary = nvme_vs & 0xFF;

        // CAP register fields
        let mqes = ((nvme_cap & 0xFFFF) as u16).saturating_add(1); // Maximum Queue Entries Supported
        let dstrd = ((nvme_cap >> 32) & 0xF) as u8; // Doorbell Stride (2^(2+DSTRD))
        let mpsmin = ((nvme_cap >> 48) & 0xF) as u8; // Memory Page Size Minimum (2^(12+MPSMIN))
        let _mpsmax = ((nvme_cap >> 52) & 0xF) as u8; // Memory Page Size Maximum
        let css = ((nvme_cap >> 37) & 0xFF) as u8; // Command Sets Supported
        let _nvm_css = css & 1 != 0; // NVM command set supported
        let cqr = (nvme_cap >> 16) & 1; // Contiguous Queues Required

        let _doorbell_stride = 4u32 << ((2u32.saturating_add(dstrd as u32)) & 0x1F); // Bytes per doorbell register pair
        let min_page_size = 1u32 << ((12u32.saturating_add(mpsmin as u32)) & 0x1F);

        let ready = nvme_csts & 1 != 0;
        let enabled = nvme_cc & 1 != 0;

        serial_println!(
            "  Storage: NVMe v{}.{}.{} at BAR0={:#x}",
            vs_major,
            vs_minor,
            vs_tertiary,
            bar0
        );
        serial_println!(
            "  Storage: NVMe CAP: MQES={}, DSTRD={}, MPSMIN={}K, CQR={}, ready={}, enabled={}",
            mqes,
            dstrd,
            min_page_size / 1024,
            cqr,
            ready,
            enabled
        );

        // Disable controller before configuring admin queues (CC.EN = 0)
        if enabled {
            unsafe {
                core::ptr::write_volatile((bar0 + 0x14) as *mut u32, nvme_cc & !1u32);
            }
            // Wait for CSTS.RDY to become 0
            for _ in 0..500_000 {
                let csts = unsafe { core::ptr::read_volatile((bar0 + 0x1C) as *const u32) };
                if csts & 1 == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }

        // Allocate frames for Admin Submission Queue and Completion Queue
        // ASQ: 64 bytes per entry, MQES entries (cap at 64 for admin)
        // ACQ: 16 bytes per entry, MQES entries (cap at 64 for admin)
        let admin_q_size = 64usize.min(mqes as usize);
        let asq_frame = crate::memory::frame_allocator::allocate_frames(1);
        let acq_frame = crate::memory::frame_allocator::allocate_frames(1);

        if asq_frame != 0 && acq_frame != 0 {
            let pflags =
                crate::memory::paging::flags::WRITABLE | crate::memory::paging::flags::NO_CACHE;
            let _ = crate::memory::paging::map_page(asq_frame, asq_frame, pflags);
            let _ = crate::memory::paging::map_page(acq_frame, acq_frame, pflags);
            unsafe {
                core::ptr::write_bytes(asq_frame as *mut u8, 0, 4096);
                core::ptr::write_bytes(acq_frame as *mut u8, 0, 4096);
            }

            // Set AQA (Admin Queue Attributes): ASQS and ACQS (0-based)
            let admin_q_m1 = (admin_q_size as u32).saturating_sub(1);
            let aqa = (admin_q_m1 << 16) | admin_q_m1;
            unsafe {
                core::ptr::write_volatile((bar0 + 0x24) as *mut u32, aqa);
                // ASQ (Admin Submission Queue Base Address) at offset 0x28 (64-bit)
                core::ptr::write_volatile((bar0 + 0x28) as *mut u32, asq_frame as u32);
                core::ptr::write_volatile(
                    (bar0 + 0x2C) as *mut u32,
                    ((asq_frame as u64) >> 32) as u32,
                );
                // ACQ (Admin Completion Queue Base Address) at offset 0x30 (64-bit)
                core::ptr::write_volatile((bar0 + 0x30) as *mut u32, acq_frame as u32);
                core::ptr::write_volatile(
                    (bar0 + 0x34) as *mut u32,
                    ((acq_frame as u64) >> 32) as u32,
                );
            }

            // Configure CC and enable:
            //   MPS = MPSMIN, CSS = 0 (NVM), IOSQES = 6 (64 bytes), IOCQES = 4 (16 bytes), EN = 1
            let cc_val = (mpsmin as u32) << 7   // MPS
                       | (0u32) << 4             // CSS = NVM
                       | (6u32) << 16            // IOSQES = 2^6 = 64 bytes
                       | (4u32) << 20            // IOCQES = 2^4 = 16 bytes
                       | 1u32; // EN = 1
            unsafe {
                core::ptr::write_volatile((bar0 + 0x14) as *mut u32, cc_val);
            }

            // Wait for CSTS.RDY = 1
            let mut ctrl_ready = false;
            for _ in 0..1_000_000 {
                let csts = unsafe { core::ptr::read_volatile((bar0 + 0x1C) as *const u32) };
                if csts & 1 != 0 {
                    ctrl_ready = true;
                    break;
                }
                if csts & 2 != 0 {
                    // CFS (Controller Fatal Status)
                    serial_println!("  Storage: NVMe controller fatal error during enable");
                    break;
                }
                core::hint::spin_loop();
            }

            if ctrl_ready {
                serial_println!("  Storage: NVMe controller enabled, admin queues configured (ASQ={:#x}, ACQ={:#x}, depth={})",
                    asq_frame, acq_frame, admin_q_size);
            } else {
                serial_println!("  Storage: NVMe controller enable timed out");
            }
        } else {
            serial_println!("  Storage: NVMe failed to allocate admin queue frames");
        }

        let ni = mgr.next_idx;
        mgr.register_device(StorageDeviceInfo {
            name: alloc::format!("nvme{}n1", ni),
            model: alloc::format!("NVMe SSD ({:04x}:{:04x})", dev.vendor_id, dev.device_id),
            serial: String::new(),
            firmware: String::new(),
            capacity_bytes: 0, // Would be filled by Identify Namespace command via admin queue
            block_size: 512,
            block_count: 0,
            device_type: StorageType::Nvme,
            read_only: false,
            supports_trim: true,
            max_transfer_blocks: 512,
        });
    }

    // Scan for Virtio block devices (class 00h, for virtio-pci)
    // Virtio-blk is identified by vendor 0x1AF4, device 0x1001 (legacy) or 0x1042 (modern)
    let virtio_legacy = super::pci::find_by_id(0x1AF4, 0x1001);
    let virtio_modern = super::pci::find_by_id(0x1AF4, 0x1042);

    for dev in virtio_legacy.iter().chain(virtio_modern.iter()) {
        serial_println!(
            "  Storage: Virtio-blk device at {} ({:04x}:{:04x})",
            dev.bdf_string(),
            dev.vendor_id,
            dev.device_id
        );

        let ni = mgr.next_idx;
        mgr.register_device(StorageDeviceInfo {
            name: alloc::format!("vd{}", (b'a' + ni as u8) as char),
            model: String::from("Virtio Block Device"),
            serial: String::new(),
            firmware: String::new(),
            capacity_bytes: 0,
            block_size: 512,
            block_count: 0,
            device_type: StorageType::VirtioBlk,
            read_only: false,
            supports_trim: false,
            max_transfer_blocks: 256,
        });
    }

    let total = mgr.devices.len();
    drop(mgr);

    if total > 0 {
        super::register("storage", super::DeviceType::Storage);
        serial_println!("  Storage: {} device(s) registered", total);
    } else {
        serial_println!("  Storage: no storage controllers found");
    }
}

/// Get the number of registered storage devices
pub fn device_count() -> usize {
    STORAGE.lock().count()
}

/// List all storage device info
pub fn list_devices() -> Vec<StorageDeviceInfo> {
    STORAGE.lock().devices.clone()
}

/// Create a RAM disk block device
pub fn create_ramdisk(size_bytes: usize) -> u32 {
    let block_size = 512u32;
    let block_count = (size_bytes as u64) / block_size as u64;

    let mut mgr = STORAGE.lock();
    let ni = mgr.next_idx;
    let idx = mgr.register_device(StorageDeviceInfo {
        name: alloc::format!("ram{}", ni),
        model: String::from("RAM Disk"),
        serial: String::new(),
        firmware: String::new(),
        capacity_bytes: size_bytes as u64,
        block_size,
        block_count,
        device_type: StorageType::RamDisk,
        read_only: false,
        supports_trim: false,
        max_transfer_blocks: 256,
    });
    idx
}
