//! PCI Bus Enumeration and Configuration
//!
//! Provides PCI device discovery and configuration space access for Genesis OS.
//! Supports both Configuration Space Access Mechanism #1 (I/O ports) and MMIO.

use core::fmt;

/// PCI Configuration Space I/O ports
const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// PCI Class Codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PciClass {
    MassStorage = 0x01,
    Network = 0x02,
    Display = 0x03,
    Multimedia = 0x04,
    Memory = 0x05,
    Bridge = 0x06,
    Unknown = 0xFF,
}

impl From<u8> for PciClass {
    fn from(val: u8) -> Self {
        match val {
            0x01 => PciClass::MassStorage,
            0x02 => PciClass::Network,
            0x03 => PciClass::Display,
            0x04 => PciClass::Multimedia,
            0x05 => PciClass::Memory,
            0x06 => PciClass::Bridge,
            _ => PciClass::Unknown,
        }
    }
}

/// PCI Mass Storage Subclass
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MassStorageSubclass {
    SCSI = 0x00,
    IDE = 0x01,
    Floppy = 0x02,
    IPI = 0x03,
    RAID = 0x04,
    ATA = 0x05,
    SATA = 0x06,
    SAS = 0x07,
    NVM = 0x08, // NVMe
    Unknown = 0xFF,
}

impl From<u8> for MassStorageSubclass {
    fn from(val: u8) -> Self {
        match val {
            0x00 => MassStorageSubclass::SCSI,
            0x01 => MassStorageSubclass::IDE,
            0x02 => MassStorageSubclass::Floppy,
            0x03 => MassStorageSubclass::IPI,
            0x04 => MassStorageSubclass::RAID,
            0x05 => MassStorageSubclass::ATA,
            0x06 => MassStorageSubclass::SATA,
            0x07 => MassStorageSubclass::SAS,
            0x08 => MassStorageSubclass::NVM,
            _ => MassStorageSubclass::Unknown,
        }
    }
}

/// PCI Device Address (Bus, Device, Function)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl PciAddress {
    pub fn new(bus: u8, device: u8, function: u8) -> Self {
        PciAddress {
            bus,
            device,
            function,
        }
    }

    /// Convert to configuration address format
    fn config_address(&self, offset: u8) -> u32 {
        let bus = (self.bus as u32) << 16;
        let device = (self.device as u32) << 11;
        let function = (self.function as u32) << 8;
        let offset = (offset as u32) & 0xFC;
        0x8000_0000 | bus | device | function | offset
    }
}

impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }
}

/// PCI Device Information
#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub address: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: PciClass,
    pub subclass: u8,
    pub prog_if: u8,
    pub revision: u8,
    pub header_type: u8,
    pub bars: [u32; 6],
}

impl PciDevice {
    /// Read configuration space register
    pub fn read_config(&self, offset: u8) -> u32 {
        pci_read_config(self.address, offset)
    }

    /// Write configuration space register
    pub fn write_config(&self, offset: u8, value: u32) {
        pci_write_config(self.address, offset, value);
    }

    /// Get BAR (Base Address Register) value
    pub fn get_bar(&self, index: usize) -> Option<u64> {
        if index >= 6 {
            return None;
        }

        let bar = self.bars[index];

        // Memory space BAR
        if bar & 0x1 == 0 {
            let is_64bit = (bar & 0x6) == 0x4;

            if is_64bit && index < 5 {
                // 64-bit BAR uses two consecutive registers
                let low = (bar & 0xFFFF_FFF0) as u64;
                let high = (self.bars[index + 1] as u64) << 32;
                Some(high | low)
            } else {
                // 32-bit BAR
                Some((bar & 0xFFFF_FFF0) as u64)
            }
        } else {
            // I/O space BAR (not used for NVMe)
            Some((bar & 0xFFFF_FFFC) as u64)
        }
    }

    /// Enable PCI bus mastering (required for DMA)
    pub fn enable_bus_mastering(&self) {
        let mut command = (self.read_config(0x04) & 0xFFFF) as u16;
        command |= 0x04; // Bus Master Enable bit
        self.write_config(0x04, command as u32);
    }

    /// Enable memory space access
    pub fn enable_memory_space(&self) {
        let mut command = (self.read_config(0x04) & 0xFFFF) as u16;
        command |= 0x02; // Memory Space Enable bit
        self.write_config(0x04, command as u32);
    }

    /// Disable interrupts (use MSI/MSI-X instead)
    pub fn disable_legacy_interrupts(&self) {
        let mut command = (self.read_config(0x04) & 0xFFFF) as u16;
        command |= 0x0400; // Interrupt Disable bit
        self.write_config(0x04, command as u32);
    }
}

/// Read from PCI configuration space
pub fn pci_read_config(address: PciAddress, offset: u8) -> u32 {
    unsafe {
        // Write address to CONFIG_ADDRESS
        let config_addr = address.config_address(offset);
        outl(PCI_CONFIG_ADDRESS, config_addr);

        // Read data from CONFIG_DATA
        inl(PCI_CONFIG_DATA)
    }
}

/// Write to PCI configuration space
pub fn pci_write_config(address: PciAddress, offset: u8, value: u32) {
    unsafe {
        // Write address to CONFIG_ADDRESS
        let config_addr = address.config_address(offset);
        outl(PCI_CONFIG_ADDRESS, config_addr);

        // Write data to CONFIG_DATA
        outl(PCI_CONFIG_DATA, value);
    }
}

/// Scan PCI bus for devices
pub fn enumerate_devices() -> [Option<PciDevice>; 256] {
    let mut devices = [None; 256];
    let mut device_count = 0;

    // Scan all possible PCI locations
    for bus in 0..=255u8 {
        for device in 0..32u8 {
            for function in 0..8u8 {
                let address = PciAddress::new(bus, device, function);

                // Read vendor ID to check if device exists
                let vendor_id = (pci_read_config(address, 0x00) & 0xFFFF) as u16;

                if vendor_id == 0xFFFF {
                    continue; // No device present
                }

                // Read device configuration
                let config0 = pci_read_config(address, 0x00);
                let config2 = pci_read_config(address, 0x08);
                let config3 = pci_read_config(address, 0x0C);

                let device_id = (config0 >> 16) as u16;
                let revision = (config2 & 0xFF) as u8;
                let prog_if = ((config2 >> 8) & 0xFF) as u8;
                let subclass = ((config2 >> 16) & 0xFF) as u8;
                let class = PciClass::from(((config2 >> 24) & 0xFF) as u8);
                let header_type = ((config3 >> 16) & 0xFF) as u8;

                // Read BARs
                let mut bars = [0u32; 6];
                for i in 0..6 {
                    bars[i] = pci_read_config(address, 0x10 + (i as u8 * 4));
                }

                let pci_device = PciDevice {
                    address,
                    vendor_id,
                    device_id,
                    class,
                    subclass,
                    prog_if,
                    revision,
                    header_type,
                    bars,
                };

                if device_count < 256 {
                    devices[device_count] = Some(pci_device);
                    device_count += 1;
                }

                // If not a multi-function device, skip remaining functions
                if function == 0 && (header_type & 0x80) == 0 {
                    break;
                }
            }
        }
    }

    devices
}

/// Find all NVMe controllers on the PCI bus
pub fn find_nvme_controllers() -> [Option<PciDevice>; 16] {
    let mut nvme_devices = [None; 16];
    let mut nvme_count = 0;

    let all_devices = enumerate_devices();

    for device_opt in all_devices.iter() {
        if let Some(device) = device_opt {
            // Check if this is a Mass Storage device with NVM subclass
            if device.class == PciClass::MassStorage {
                let subclass = MassStorageSubclass::from(device.subclass);

                if subclass == MassStorageSubclass::NVM && device.prog_if == 0x02 {
                    // This is an NVMe controller
                    if nvme_count < 16 {
                        nvme_devices[nvme_count] = Some(*device);
                        nvme_count += 1;
                    }
                }
            }
        }
    }

    nvme_devices
}

// ============================================================================
// Low-level I/O Port Operations
// ============================================================================

#[inline]
unsafe fn outl(port: u16, value: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx") port,
        in("eax") value,
        options(nomem, nostack, preserves_flags)
    );
}

#[inline]
unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    core::arch::asm!(
        "in eax, dx",
        out("eax") value,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    value
}

/// Initialize PCI subsystem
pub fn init() {
    // PCI enumeration will be done on-demand
    // No initialization required for legacy I/O port access
}
