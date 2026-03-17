use crate::io::{inl, outl};
use crate::sync::Mutex;
/// PCI bus driver for Genesis
///
/// Scans the PCI bus to discover hardware devices.
/// Each device is identified by vendor ID + device ID.
/// Drivers register interest in specific device IDs.
///
/// PCI configuration space is accessed via I/O ports 0xCF8 (address) and 0xCFC (data).
/// Supports full bus enumeration, BAR decoding (32/64-bit MMIO + I/O space),
/// BAR size detection, MSI/MSI-X interrupt setup, capability list walking,
/// and device enable/disable control.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// PCI configuration address port
const PCI_CONFIG_ADDR: u16 = 0xCF8;
/// PCI configuration data port
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// PCI capability IDs
const PCI_CAP_ID_PM: u8 = 0x01; // Power Management
const PCI_CAP_ID_AGP: u8 = 0x02; // AGP
const PCI_CAP_ID_MSI: u8 = 0x05; // Message Signaled Interrupts
const PCI_CAP_ID_PCIE: u8 = 0x10; // PCI Express
const PCI_CAP_ID_MSIX: u8 = 0x11; // MSI-X
const PCI_CAP_ID_VENDOR: u8 = 0x09; // Vendor Specific

/// PCI command register bits
const PCI_CMD_IO_SPACE: u16 = 1 << 0;
const PCI_CMD_MEMORY_SPACE: u16 = 1 << 1;
const PCI_CMD_BUS_MASTER: u16 = 1 << 2;
const PCI_CMD_INTERRUPT_DISABLE: u16 = 1 << 10;

/// PCI configuration space offsets
const PCI_VENDOR_ID: u8 = 0x00;
const PCI_DEVICE_ID: u8 = 0x02;
const PCI_COMMAND: u8 = 0x04;
const PCI_STATUS: u8 = 0x06;
const PCI_REVISION_ID: u8 = 0x08;
const PCI_PROG_IF: u8 = 0x09;
const PCI_SUBCLASS: u8 = 0x0A;
const PCI_CLASS_CODE: u8 = 0x0B;
const PCI_CACHE_LINE_SIZE: u8 = 0x0C;
const PCI_HEADER_TYPE: u8 = 0x0E;
const PCI_BAR0: u8 = 0x10;
const PCI_CAPABILITIES_PTR: u8 = 0x34;
const PCI_INTERRUPT_LINE: u8 = 0x3C;
const PCI_INTERRUPT_PIN: u8 = 0x3D;

/// BAR type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarType {
    /// Memory-mapped I/O, 32-bit address
    Memory32,
    /// Memory-mapped I/O, 64-bit address (consumes two BAR slots)
    Memory64,
    /// I/O port space
    IoPort,
    /// BAR is not present or invalid
    None,
}

/// Decoded Base Address Register
#[derive(Debug, Clone, Copy)]
pub struct Bar {
    pub bar_type: BarType,
    pub address: u64,
    pub size: u64,
    pub prefetchable: bool,
}

impl Bar {
    const fn empty() -> Self {
        Bar {
            bar_type: BarType::None,
            address: 0,
            size: 0,
            prefetchable: false,
        }
    }
}

/// PCI capability entry found while walking the capability list
#[derive(Debug, Clone, Copy)]
pub struct PciCapability {
    pub id: u8,
    pub offset: u8,
}

/// A discovered PCI device
#[derive(Debug, Clone)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
    pub revision_id: u8,
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
    pub bars: [Bar; 6],
    pub capabilities: Vec<PciCapability>,
    pub msi_offset: Option<u8>,
    pub msix_offset: Option<u8>,
    pub pcie_offset: Option<u8>,
    pub pm_offset: Option<u8>,
}

impl PciDevice {
    /// Get the BDF (Bus:Device.Function) as a string
    pub fn bdf_string(&self) -> alloc::string::String {
        alloc::format!("{:02x}:{:02x}.{}", self.bus, self.device, self.function)
    }

    /// Get the class name
    pub fn class_name(&self) -> &'static str {
        match (self.class, self.subclass) {
            (0x00, 0x00) => "Non-VGA Unclassified",
            (0x00, 0x01) => "VGA-Compatible Unclassified",
            (0x01, 0x00) => "SCSI Controller",
            (0x01, 0x01) => "IDE Controller",
            (0x01, 0x04) => "RAID Controller",
            (0x01, 0x05) => "ATA Controller",
            (0x01, 0x06) => "SATA Controller",
            (0x01, 0x07) => "SAS Controller",
            (0x01, 0x08) => "NVMe Controller",
            (0x02, 0x00) => "Ethernet Controller",
            (0x02, 0x80) => "Network Controller",
            (0x03, 0x00) => "VGA Controller",
            (0x03, 0x01) => "XGA Controller",
            (0x04, 0x00) => "Video Device",
            (0x04, 0x01) => "Audio Controller",
            (0x04, 0x03) => "HD Audio Controller",
            (0x05, 0x00) => "RAM Controller",
            (0x05, 0x01) => "Flash Controller",
            (0x06, 0x00) => "Host Bridge",
            (0x06, 0x01) => "ISA Bridge",
            (0x06, 0x02) => "EISA Bridge",
            (0x06, 0x04) => "PCI-PCI Bridge",
            (0x06, 0x80) => "Other Bridge",
            (0x07, 0x00) => "Serial Controller",
            (0x07, 0x01) => "Parallel Controller",
            (0x08, 0x00) => "PIC",
            (0x08, 0x01) => "DMA Controller",
            (0x08, 0x02) => "Timer",
            (0x08, 0x03) => "RTC Controller",
            (0x0C, 0x00) => "FireWire Controller",
            (0x0C, 0x03) => "USB Controller",
            (0x0C, 0x05) => "SMBus Controller",
            (0x0D, 0x00) => "IRDA Controller",
            (0x0D, 0x11) => "Bluetooth Controller",
            (0x0D, 0x20) => "WiFi Controller",
            _ => "Unknown",
        }
    }

    /// Check if device has a specific capability
    pub fn has_capability(&self, cap_id: u8) -> bool {
        self.capabilities.iter().any(|c| c.id == cap_id)
    }

    /// Find capability offset by ID
    pub fn find_capability(&self, cap_id: u8) -> Option<u8> {
        self.capabilities
            .iter()
            .find(|c| c.id == cap_id)
            .map(|c| c.offset)
    }

    /// Get total memory required by all BARs
    pub fn total_bar_size(&self) -> u64 {
        let mut total = 0u64;
        for bar in &self.bars {
            total += bar.size;
        }
        total
    }

    /// Check if this is a multifunction device
    pub fn is_multifunction(&self) -> bool {
        self.header_type & 0x80 != 0
    }

    /// Check if this is a PCI-to-PCI bridge (header type 1)
    pub fn is_bridge(&self) -> bool {
        (self.header_type & 0x7F) == 1
    }
}

/// Known PCI vendor IDs
pub mod vendors {
    pub const INTEL: u16 = 0x8086;
    pub const AMD: u16 = 0x1022;
    pub const NVIDIA: u16 = 0x10DE;
    pub const QEMU_VIRTIO: u16 = 0x1AF4;
    pub const REDHAT: u16 = 0x1B36;
    pub const REALTEK: u16 = 0x10EC;
    pub const BROADCOM: u16 = 0x14E4;
    pub const QUALCOMM: u16 = 0x168C;
    pub const VMWARE: u16 = 0x15AD;
    pub const VIRTIO_MODERN: u16 = 0x1AF4;
}

/// Global list of discovered PCI devices
static PCI_DEVICES: Mutex<Vec<PciDevice>> = Mutex::new(Vec::new());

/// Read a 32-bit value from PCI configuration space
fn pci_read_config(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address: u32 = (1 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    outl(PCI_CONFIG_ADDR, address);
    inl(PCI_CONFIG_DATA)
}

/// Write a 32-bit value to PCI configuration space
fn pci_write_config(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address: u32 = (1 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    outl(PCI_CONFIG_ADDR, address);
    outl(PCI_CONFIG_DATA, value);
}

/// Read a 16-bit value from PCI configuration space
fn pci_read_config_u16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let val = pci_read_config(bus, device, function, offset & 0xFC);
    ((val >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

/// Write a 16-bit value to PCI configuration space
fn pci_write_config_u16(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
    let aligned = offset & 0xFC;
    let shift = (offset & 2) * 8;
    let old = pci_read_config(bus, device, function, aligned);
    let mask = !(0xFFFF << shift);
    let new_val = (old & mask) | ((value as u32) << shift);
    pci_write_config(bus, device, function, aligned, new_val);
}

/// Read an 8-bit value from PCI configuration space
fn pci_read_config_u8(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
    let val = pci_read_config(bus, device, function, offset & 0xFC);
    ((val >> ((offset & 3) * 8)) & 0xFF) as u8
}

/// Walk the PCI capability list for a device
fn walk_capabilities(bus: u8, device: u8, function: u8) -> Vec<PciCapability> {
    let mut caps = Vec::new();

    // Check if capabilities list is supported (bit 4 of status register)
    let status = pci_read_config_u16(bus, device, function, PCI_STATUS);
    if status & (1 << 4) == 0 {
        return caps;
    }

    // Read the capabilities pointer (offset 0x34)
    let mut cap_ptr = pci_read_config_u8(bus, device, function, PCI_CAPABILITIES_PTR);
    cap_ptr &= 0xFC; // must be dword-aligned

    let mut visited = 0u32;
    while cap_ptr != 0 && visited < 48 {
        let cap_id = pci_read_config_u8(bus, device, function, cap_ptr);
        let next_ptr = pci_read_config_u8(bus, device, function, cap_ptr.saturating_add(1));

        caps.push(PciCapability {
            id: cap_id,
            offset: cap_ptr,
        });

        cap_ptr = next_ptr & 0xFC;
        visited = visited.saturating_add(1);
    }

    caps
}

/// Decode a BAR register, returning the bar info and whether it consumed two slots (64-bit)
fn decode_bar(bus: u8, device: u8, function: u8, bar_index: u8) -> (Bar, bool) {
    let offset = PCI_BAR0.saturating_add(bar_index.saturating_mul(4));
    let original = pci_read_config(bus, device, function, offset);

    if original == 0 {
        return (Bar::empty(), false);
    }

    // Determine BAR type
    if original & 1 != 0 {
        // I/O space BAR
        let addr = (original & 0xFFFFFFFC) as u64;

        // Detect size: write all 1s, read back, restore
        pci_write_config(bus, device, function, offset, 0xFFFFFFFF);
        let size_mask = pci_read_config(bus, device, function, offset);
        pci_write_config(bus, device, function, offset, original);

        let size = if size_mask == 0 || size_mask == 0xFFFFFFFF {
            0
        } else {
            let masked = size_mask & 0xFFFFFFFC;
            ((!masked).wrapping_add(1) & 0xFFFF) as u64
        };

        (
            Bar {
                bar_type: BarType::IoPort,
                address: addr,
                size,
                prefetchable: false,
            },
            false,
        )
    } else {
        // Memory space BAR
        let bar_type_bits = (original >> 1) & 3;
        let prefetchable = (original & (1 << 3)) != 0;

        if bar_type_bits == 2 {
            // 64-bit BAR
            let upper = pci_read_config(bus, device, function, offset + 4);
            let addr = ((upper as u64) << 32) | ((original & 0xFFFFFFF0) as u64);

            // Detect size: write all 1s to both BARs
            pci_write_config(bus, device, function, offset, 0xFFFFFFFF);
            pci_write_config(bus, device, function, offset + 4, 0xFFFFFFFF);
            let low_mask = pci_read_config(bus, device, function, offset);
            let high_mask = pci_read_config(bus, device, function, offset + 4);
            pci_write_config(bus, device, function, offset, original);
            pci_write_config(bus, device, function, offset + 4, upper);

            let combined = ((high_mask as u64) << 32) | ((low_mask & 0xFFFFFFF0) as u64);
            let size = if combined == 0 {
                0
            } else {
                (!combined).wrapping_add(1)
            };

            (
                Bar {
                    bar_type: BarType::Memory64,
                    address: addr,
                    size,
                    prefetchable,
                },
                true,
            ) // consumed two BAR slots
        } else {
            // 32-bit BAR
            let addr = (original & 0xFFFFFFF0) as u64;

            pci_write_config(bus, device, function, offset, 0xFFFFFFFF);
            let size_mask = pci_read_config(bus, device, function, offset);
            pci_write_config(bus, device, function, offset, original);

            let masked = size_mask & 0xFFFFFFF0;
            let size = if masked == 0 {
                0
            } else {
                ((!masked).wrapping_add(1)) as u64
            };

            (
                Bar {
                    bar_type: BarType::Memory32,
                    address: addr,
                    size,
                    prefetchable,
                },
                false,
            )
        }
    }
}

/// Check if a PCI device exists at bus:device.function and fully probe it
fn probe_device(bus: u8, device: u8, function: u8) -> Option<PciDevice> {
    let vendor_device = pci_read_config(bus, device, function, 0);
    let vendor_id = (vendor_device & 0xFFFF) as u16;

    if vendor_id == 0xFFFF {
        return None; // no device
    }

    let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;
    let class_info = pci_read_config(bus, device, function, 0x08);
    let class = ((class_info >> 24) & 0xFF) as u8;
    let subclass = ((class_info >> 16) & 0xFF) as u8;
    let prog_if = ((class_info >> 8) & 0xFF) as u8;
    let revision_id = (class_info & 0xFF) as u8;

    let header_info = pci_read_config(bus, device, function, 0x0C);
    let header_type = ((header_info >> 16) & 0xFF) as u8;

    let int_info = pci_read_config(bus, device, function, 0x3C);
    let interrupt_line = (int_info & 0xFF) as u8;
    let interrupt_pin = ((int_info >> 8) & 0xFF) as u8;

    // Walk capabilities list
    let capabilities = walk_capabilities(bus, device, function);

    let mut msi_offset = None;
    let mut msix_offset = None;
    let mut pcie_offset = None;
    let mut pm_offset = None;

    for cap in &capabilities {
        match cap.id {
            PCI_CAP_ID_MSI => msi_offset = Some(cap.offset),
            PCI_CAP_ID_MSIX => msix_offset = Some(cap.offset),
            PCI_CAP_ID_PCIE => pcie_offset = Some(cap.offset),
            PCI_CAP_ID_PM => pm_offset = Some(cap.offset),
            _ => {}
        }
    }

    // Decode BARs (only for type 0 headers; bridges have fewer BARs)
    let max_bars: u8 = if (header_type & 0x7F) == 0 { 6 } else { 2 };
    let mut bars = [Bar::empty(); 6];
    let mut bar_idx: u8 = 0;
    while bar_idx < max_bars {
        let (bar, is_64bit) = decode_bar(bus, device, function, bar_idx);
        bars[bar_idx as usize] = bar;
        if is_64bit {
            bar_idx += 2; // 64-bit BAR consumes two slots
        } else {
            bar_idx += 1;
        }
    }

    Some(PciDevice {
        bus,
        device,
        function,
        vendor_id,
        device_id,
        class,
        subclass,
        prog_if,
        header_type,
        revision_id,
        interrupt_line,
        interrupt_pin,
        bars,
        capabilities,
        msi_offset,
        msix_offset,
        pcie_offset,
        pm_offset,
    })
}

/// Scan a single PCI bus, recursively scanning behind bridges
fn scan_bus(bus: u8, devices: &mut Vec<PciDevice>) {
    for device in 0..32u8 {
        if let Some(dev) = probe_device(bus, device, 0) {
            let multi_function = dev.header_type & 0x80 != 0;
            let is_bridge = dev.is_bridge();
            devices.push(dev);

            if multi_function {
                for function in 1..8u8 {
                    if let Some(dev) = probe_device(bus, device, function) {
                        devices.push(dev);
                    }
                }
            }

            // If this is a PCI-PCI bridge, scan the secondary bus
            if is_bridge {
                let bridge_reg = pci_read_config(bus, device, 0, 0x18);
                let secondary_bus = ((bridge_reg >> 8) & 0xFF) as u8;
                if secondary_bus != 0 && secondary_bus != bus {
                    scan_bus(secondary_bus, devices);
                }
            }
        }
    }
}

/// Scan the entire PCI bus hierarchy
pub fn scan() -> Vec<PciDevice> {
    let mut devices = Vec::new();

    // Check if host bridge is multifunction (multiple PCI domains)
    let header0 = pci_read_config(0, 0, 0, 0x0C);
    let host_header_type = ((header0 >> 16) & 0xFF) as u8;

    if host_header_type & 0x80 != 0 {
        // Multiple PCI host bridges — scan each function as a separate bus domain
        for function in 0..8u8 {
            let vendor = pci_read_config(0, 0, function, 0) & 0xFFFF;
            if vendor as u16 != 0xFFFF {
                scan_bus(function, &mut devices);
            }
        }
    } else {
        // Single host bridge — scan all 256 buses
        for bus in 0..=255u8 {
            scan_bus(bus, &mut devices);
            // Optimization: stop if we haven't found anything on higher buses
            // (most machines only have bus 0-3)
        }
    }

    devices
}

/// Initialize PCI subsystem — scan bus and register devices
pub fn init() {
    let devices = scan();
    let count = devices.len();

    for dev in &devices {
        serial_println!(
            "  PCI: {} {:04x}:{:04x} {} (class {:02x}:{:02x}) IRQ {} BARs: {}",
            dev.bdf_string(),
            dev.vendor_id,
            dev.device_id,
            dev.class_name(),
            dev.class,
            dev.subclass,
            dev.interrupt_line,
            dev.bars
                .iter()
                .filter(|b| b.bar_type != BarType::None)
                .count()
        );

        // Log capability info
        if !dev.capabilities.is_empty() {
            let mut cap_str = alloc::string::String::new();
            for cap in &dev.capabilities {
                if !cap_str.is_empty() {
                    cap_str.push_str(", ");
                }
                match cap.id {
                    PCI_CAP_ID_PM => cap_str.push_str("PM"),
                    PCI_CAP_ID_MSI => cap_str.push_str("MSI"),
                    PCI_CAP_ID_MSIX => cap_str.push_str("MSI-X"),
                    PCI_CAP_ID_PCIE => cap_str.push_str("PCIe"),
                    PCI_CAP_ID_AGP => cap_str.push_str("AGP"),
                    PCI_CAP_ID_VENDOR => cap_str.push_str("VendorSpec"),
                    _ => {
                        cap_str.push_str("0x");
                        // simple hex format for unknown cap IDs
                        let hi = cap.id >> 4;
                        let lo = cap.id & 0xF;
                        cap_str.push(core::char::from_digit(hi as u32, 16).unwrap_or('?'));
                        cap_str.push(core::char::from_digit(lo as u32, 16).unwrap_or('?'));
                    }
                }
            }
            serial_println!("    Caps: [{}]", cap_str);
        }
    }

    *PCI_DEVICES.lock() = devices;
    super::register("pci-bus", super::DeviceType::Other);
    serial_println!("  PCI: {} devices found", count);

    // Populate the flat device tree used by pci_sysfs_read()
    scan_pci_bus();

    // Register all discovered devices in /sys/bus/pci/devices/
    register_pci_sysfs();
}

/// Find PCI devices by class/subclass
pub fn find_by_class(class: u8, subclass: u8) -> Vec<PciDevice> {
    PCI_DEVICES
        .lock()
        .iter()
        .filter(|d| d.class == class && d.subclass == subclass)
        .cloned()
        .collect()
}

/// Find PCI devices by class/subclass/prog_if
pub fn find_by_class_full(class: u8, subclass: u8, prog_if: u8) -> Vec<PciDevice> {
    PCI_DEVICES
        .lock()
        .iter()
        .filter(|d| d.class == class && d.subclass == subclass && d.prog_if == prog_if)
        .cloned()
        .collect()
}

/// Read a PCI BAR (Base Address Register)
/// Returns (address, is_mmio)
pub fn read_bar(bus: u8, device: u8, function: u8, bar_index: u8) -> (u64, bool) {
    let offset = 0x10 + (bar_index as u8) * 4;
    let raw = pci_read_config(bus, device, function, offset);

    if raw & 1 != 0 {
        // I/O space BAR
        let addr = (raw & 0xFFFFFFFC) as u64;
        (addr, false)
    } else {
        // Memory space BAR
        let bar_type = (raw >> 1) & 3;
        if bar_type == 2 {
            // 64-bit BAR — read next BAR for upper 32 bits
            let upper = pci_read_config(bus, device, function, offset + 4);
            let addr = ((upper as u64) << 32) | ((raw & 0xFFFFFFF0) as u64);
            (addr, true)
        } else {
            let addr = (raw & 0xFFFFFFF0) as u64;
            (addr, true)
        }
    }
}

/// Detect the size of a BAR by writing all 1s and reading back
pub fn detect_bar_size(bus: u8, device: u8, function: u8, bar_index: u8) -> u64 {
    let devices = PCI_DEVICES.lock();
    for dev in devices.iter() {
        if dev.bus == bus && dev.device == device && dev.function == function {
            if (bar_index as usize) < 6 {
                return dev.bars[bar_index as usize].size;
            }
        }
    }
    0
}

/// Enable PCI bus mastering for a device (needed for DMA)
pub fn enable_bus_master(bus: u8, device: u8, function: u8) {
    let cmd = pci_read_config(bus, device, function, 0x04) as u16;
    let new_cmd = cmd | PCI_CMD_BUS_MASTER;
    pci_write_config_u16(bus, device, function, PCI_COMMAND, new_cmd);
}

/// Enable memory space access for a device
pub fn enable_memory_space(bus: u8, device: u8, function: u8) {
    let cmd = pci_read_config_u16(bus, device, function, PCI_COMMAND);
    let new_cmd = cmd | PCI_CMD_MEMORY_SPACE;
    pci_write_config_u16(bus, device, function, PCI_COMMAND, new_cmd);
}

/// Enable I/O space access for a device
pub fn enable_io_space(bus: u8, device: u8, function: u8) {
    let cmd = pci_read_config_u16(bus, device, function, PCI_COMMAND);
    let new_cmd = cmd | PCI_CMD_IO_SPACE;
    pci_write_config_u16(bus, device, function, PCI_COMMAND, new_cmd);
}

/// Enable all access types (bus master + memory + I/O) for a device
pub fn enable_device(bus: u8, device: u8, function: u8) {
    let cmd = pci_read_config_u16(bus, device, function, PCI_COMMAND);
    let new_cmd = cmd | PCI_CMD_BUS_MASTER | PCI_CMD_MEMORY_SPACE | PCI_CMD_IO_SPACE;
    pci_write_config_u16(bus, device, function, PCI_COMMAND, new_cmd);
}

/// Disable legacy INTx interrupts for a device (used when switching to MSI)
pub fn disable_intx(bus: u8, device: u8, function: u8) {
    let cmd = pci_read_config_u16(bus, device, function, PCI_COMMAND);
    let new_cmd = cmd | PCI_CMD_INTERRUPT_DISABLE;
    pci_write_config_u16(bus, device, function, PCI_COMMAND, new_cmd);
}

/// Set up MSI (Message Signaled Interrupts) for a device
///
/// `vector` is the interrupt vector number.
/// `address` is the MSI message address (typically 0xFEE00000 for local APIC).
/// Returns true if MSI was successfully configured.
pub fn setup_msi(bus: u8, device: u8, function: u8, vector: u8) -> bool {
    // Find MSI capability
    let caps = walk_capabilities(bus, device, function);
    let msi_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_MSI) {
        Some(c) => c.offset,
        None => return false,
    };

    // Read MSI message control
    let msg_ctrl = pci_read_config_u16(bus, device, function, msi_cap + 2);
    let is_64bit = (msg_ctrl & (1 << 7)) != 0;
    let _multi_msg_capable = (msg_ctrl >> 1) & 0x7; // log2 of number of vectors

    // MSI message address: target APIC ID 0, physical mode, fixed delivery
    // Address format: 0xFEE[DestID]00[RH][DM]0
    let msi_addr: u32 = 0xFEE00000; // Local APIC base, dest ID 0

    // MSI message data: vector number, edge trigger, fixed delivery
    let msi_data: u16 = vector as u16;

    // Write message address
    pci_write_config(bus, device, function, msi_cap + 4, msi_addr);

    if is_64bit {
        // 64-bit: upper address at +8, data at +12
        pci_write_config(bus, device, function, msi_cap + 8, 0); // upper addr
        pci_write_config_u16(bus, device, function, msi_cap + 12, msi_data);
    } else {
        // 32-bit: data at +8
        pci_write_config_u16(bus, device, function, msi_cap + 8, msi_data);
    }

    // Enable MSI (bit 0 of message control), request 1 vector
    let new_ctrl = (msg_ctrl & !0x70) | 1; // clear MME bits, set enable
    pci_write_config_u16(bus, device, function, msi_cap + 2, new_ctrl);

    // Disable legacy INTx
    disable_intx(bus, device, function);

    serial_println!(
        "  PCI: MSI configured for {:02x}:{:02x}.{} vector {}",
        bus,
        device,
        function,
        vector
    );
    true
}

/// Set up MSI-X for a device
///
/// `vector` is the interrupt vector number.
/// `entry_index` is the MSI-X table entry to configure (usually 0).
/// Returns true if MSI-X was successfully configured.
pub fn setup_msix(bus: u8, device: u8, function: u8, entry_index: u16, vector: u8) -> bool {
    let caps = walk_capabilities(bus, device, function);
    let msix_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_MSIX) {
        Some(c) => c.offset,
        None => return false,
    };

    // Read MSI-X message control
    let msg_ctrl = pci_read_config_u16(bus, device, function, msix_cap + 2);
    let table_size = (msg_ctrl & 0x7FF) + 1;

    if entry_index >= table_size {
        return false;
    }

    // Read table BIR and offset
    let table_offset_bir = pci_read_config(bus, device, function, msix_cap + 4);
    let table_bir = (table_offset_bir & 0x7) as u8; // BAR index
    let table_offset = table_offset_bir & !0x7;

    // Get the BAR address for the MSI-X table
    let (bar_addr, _is_mmio) = read_bar(bus, device, function, table_bir);
    if bar_addr == 0 {
        return false;
    }

    let table_base = (bar_addr as usize).saturating_add(table_offset as usize);
    let entry_addr = table_base.saturating_add((entry_index as usize).saturating_mul(16));

    // Write MSI-X table entry
    // Offset 0: Message Address (lower 32 bits)
    // Offset 4: Message Address (upper 32 bits)
    // Offset 8: Message Data
    // Offset 12: Vector Control (bit 0 = masked)
    unsafe {
        core::ptr::write_volatile(entry_addr as *mut u32, 0xFEE00000); // addr low
        core::ptr::write_volatile(entry_addr.saturating_add(4) as *mut u32, 0); // addr high
        core::ptr::write_volatile(entry_addr.saturating_add(8) as *mut u32, vector as u32); // data
        core::ptr::write_volatile(entry_addr.saturating_add(12) as *mut u32, 0);
        // unmask
    }

    // Enable MSI-X (bit 15 of message control), clear function mask (bit 14)
    let new_ctrl = (msg_ctrl | (1 << 15)) & !(1 << 14);
    pci_write_config_u16(bus, device, function, msix_cap + 2, new_ctrl);

    // Disable legacy INTx
    disable_intx(bus, device, function);

    serial_println!(
        "  PCI: MSI-X configured for {:02x}:{:02x}.{} entry {} vector {}",
        bus,
        device,
        function,
        entry_index,
        vector
    );
    true
}

/// Set PCI device power state (D0 = full power, D3 = off)
/// Uses the Power Management capability
pub fn set_power_state(bus: u8, device: u8, function: u8, state: u8) -> bool {
    let caps = walk_capabilities(bus, device, function);
    let pm_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_PM) {
        Some(c) => c.offset,
        None => return false,
    };

    // Read PMCSR (Power Management Control/Status Register) at cap+4
    let pmcsr = pci_read_config_u16(bus, device, function, pm_cap + 4);
    let new_pmcsr = (pmcsr & !0x3) | (state as u16 & 0x3);
    pci_write_config_u16(bus, device, function, pm_cap + 4, new_pmcsr);

    // If transitioning to D0 from D3, need to wait at least 10ms
    if state == 0 && (pmcsr & 0x3) == 3 {
        crate::time::clock::sleep_ms(10);
    }

    true
}

/// Get the PCIe link speed and width (if device is PCIe)
/// Returns (speed_encoding, width) or None if not PCIe
pub fn get_pcie_link_info(bus: u8, device: u8, function: u8) -> Option<(u8, u8)> {
    let caps = walk_capabilities(bus, device, function);
    let pcie_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_PCIE) {
        Some(c) => c.offset,
        None => return None,
    };

    // Link Status Register is at PCIe cap + 0x12
    let link_status = pci_read_config_u16(bus, device, function, pcie_cap + 0x12);
    let speed = (link_status & 0xF) as u8; // bits 3:0
    let width = ((link_status >> 4) & 0x3F) as u8; // bits 9:4

    Some((speed, width))
}

/// Find PCI devices by vendor/device ID
pub fn find_by_id(vendor_id: u16, device_id: u16) -> Vec<PciDevice> {
    PCI_DEVICES
        .lock()
        .iter()
        .filter(|d| d.vendor_id == vendor_id && d.device_id == device_id)
        .cloned()
        .collect()
}

/// Find first PCI device matching vendor/device ID
pub fn find_first_by_id(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    PCI_DEVICES
        .lock()
        .iter()
        .find(|d| d.vendor_id == vendor_id && d.device_id == device_id)
        .cloned()
}

/// Find all devices from a specific vendor
pub fn find_by_vendor(vendor_id: u16) -> Vec<PciDevice> {
    PCI_DEVICES
        .lock()
        .iter()
        .filter(|d| d.vendor_id == vendor_id)
        .cloned()
        .collect()
}

/// Read a 16-bit value from PCI configuration space (public)
pub fn config_read_u16(bus: u8, device: u8, function: u8, offset: u16) -> u16 {
    let val = pci_read_config(bus, device, function, (offset & 0xFC) as u8);
    ((val >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

/// Write a 16-bit value to PCI configuration space (public)
pub fn config_write_u16(bus: u8, device: u8, function: u8, offset: u16, value: u16) {
    pci_write_config_u16(bus, device, function, (offset & 0xFF) as u8, value);
}

/// Read a 32-bit value from PCI configuration space (public)
pub fn config_read(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    pci_read_config(bus, device, function, offset)
}

/// Write a 32-bit value to PCI configuration space (public)
pub fn config_write(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    pci_write_config(bus, device, function, offset, value);
}

/// Find a device by class/subclass and return a specific BAR value
pub fn find_device_bar(class: u8, subclass: u8, bar_idx: u8) -> Option<u64> {
    let devices = PCI_DEVICES.lock();
    for dev in devices.iter() {
        if dev.class == class && dev.subclass == subclass {
            let (addr, _is_mmio) = read_bar(dev.bus, dev.device, dev.function, bar_idx);
            return Some(addr);
        }
    }
    None
}

/// Get all discovered PCI devices
pub fn all_devices() -> Vec<PciDevice> {
    PCI_DEVICES.lock().clone()
}

/// Get count of discovered PCI devices
pub fn device_count() -> usize {
    PCI_DEVICES.lock().len()
}

/// Dump full PCI configuration space for a device (256 bytes for conventional PCI)
pub fn dump_config_space(bus: u8, device: u8, function: u8) -> [u32; 64] {
    let mut space = [0u32; 64];
    for i in 0..64 {
        space[i] = pci_read_config(bus, device, function, (i * 4) as u8);
    }
    space
}

/// MSI-X table entry (in-memory representation for managing multiple vectors)
#[derive(Debug, Clone, Copy)]
pub struct MsixTableEntry {
    pub vector: u8,
    pub address_lo: u32,
    pub address_hi: u32,
    pub data: u32,
    pub masked: bool,
}

/// Get the number of MSI-X table entries for a device
pub fn msix_table_size(bus: u8, device: u8, function: u8) -> Option<u16> {
    let caps = walk_capabilities(bus, device, function);
    let msix_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_MSIX) {
        Some(c) => c.offset,
        None => return None,
    };
    let msg_ctrl = pci_read_config_u16(bus, device, function, msix_cap + 2);
    Some((msg_ctrl & 0x7FF) + 1)
}

/// Mask a specific MSI-X table entry
pub fn msix_mask_entry(bus: u8, device: u8, function: u8, entry_index: u16) -> bool {
    let caps = walk_capabilities(bus, device, function);
    let msix_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_MSIX) {
        Some(c) => c.offset,
        None => return false,
    };

    let table_offset_bir = pci_read_config(bus, device, function, msix_cap + 4);
    let table_bir = (table_offset_bir & 0x7) as u8;
    let table_offset = table_offset_bir & !0x7;

    let (bar_addr, _) = read_bar(bus, device, function, table_bir);
    if bar_addr == 0 {
        return false;
    }

    let entry_addr = (bar_addr as usize)
        .saturating_add(table_offset as usize)
        .saturating_add((entry_index as usize).saturating_mul(16));

    // Set the mask bit (bit 0 of vector control, at offset 12 in the entry)
    unsafe {
        let ctrl = core::ptr::read_volatile(entry_addr.saturating_add(12) as *const u32);
        core::ptr::write_volatile(entry_addr.saturating_add(12) as *mut u32, ctrl | 1);
    }
    true
}

/// Unmask a specific MSI-X table entry
pub fn msix_unmask_entry(bus: u8, device: u8, function: u8, entry_index: u16) -> bool {
    let caps = walk_capabilities(bus, device, function);
    let msix_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_MSIX) {
        Some(c) => c.offset,
        None => return false,
    };

    let table_offset_bir = pci_read_config(bus, device, function, msix_cap.saturating_add(4));
    let table_bir = (table_offset_bir & 0x7) as u8;
    let table_offset = table_offset_bir & !0x7;

    let (bar_addr, _) = read_bar(bus, device, function, table_bir);
    if bar_addr == 0 {
        return false;
    }

    let entry_addr = (bar_addr as usize)
        .saturating_add(table_offset as usize)
        .saturating_add((entry_index as usize).saturating_mul(16));

    unsafe {
        let ctrl = core::ptr::read_volatile(entry_addr.saturating_add(12) as *const u32);
        core::ptr::write_volatile(entry_addr.saturating_add(12) as *mut u32, ctrl & !1);
    }
    true
}

/// Set the function-level mask for all MSI-X entries at once
pub fn msix_set_function_mask(bus: u8, device: u8, function: u8, masked: bool) -> bool {
    let caps = walk_capabilities(bus, device, function);
    let msix_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_MSIX) {
        Some(c) => c.offset,
        None => return false,
    };

    let mut msg_ctrl = pci_read_config_u16(bus, device, function, msix_cap + 2);
    if masked {
        msg_ctrl |= 1 << 14; // set function mask
    } else {
        msg_ctrl &= !(1 << 14); // clear function mask
    }
    pci_write_config_u16(bus, device, function, msix_cap + 2, msg_ctrl);
    true
}

/// Read MSI message control register to get capability details
/// Returns (is_64bit, multi_message_capable_log2, multi_message_enable_log2, per_vector_masking)
pub fn msi_capabilities(bus: u8, device: u8, function: u8) -> Option<(bool, u8, u8, bool)> {
    let caps = walk_capabilities(bus, device, function);
    let msi_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_MSI) {
        Some(c) => c.offset,
        None => return None,
    };

    let msg_ctrl = pci_read_config_u16(bus, device, function, msi_cap + 2);
    let is_64bit = (msg_ctrl & (1 << 7)) != 0;
    let mmc = ((msg_ctrl >> 1) & 0x7) as u8; // multi-message capable
    let mme = ((msg_ctrl >> 4) & 0x7) as u8; // multi-message enable
    let pvm = (msg_ctrl & (1 << 8)) != 0; // per-vector masking

    Some((is_64bit, mmc, mme, pvm))
}

/// Configure MSI for multiple vectors (if device supports it)
/// `num_vectors_log2` is log2 of the number of vectors (0=1, 1=2, 2=4, etc.)
/// `base_vector` is the first interrupt vector
pub fn setup_msi_multi(
    bus: u8,
    device: u8,
    function: u8,
    base_vector: u8,
    num_vectors_log2: u8,
) -> bool {
    let caps = walk_capabilities(bus, device, function);
    let msi_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_MSI) {
        Some(c) => c.offset,
        None => return false,
    };

    let msg_ctrl = pci_read_config_u16(bus, device, function, msi_cap + 2);
    let is_64bit = (msg_ctrl & (1 << 7)) != 0;
    let mmc = ((msg_ctrl >> 1) & 0x7) as u8;

    // Cannot request more vectors than device supports
    let actual_log2 = num_vectors_log2.min(mmc);

    // MSI message address
    let msi_addr: u32 = 0xFEE00000;
    let msi_data: u16 = base_vector as u16;

    pci_write_config(bus, device, function, msi_cap + 4, msi_addr);

    if is_64bit {
        pci_write_config(bus, device, function, msi_cap + 8, 0);
        pci_write_config_u16(bus, device, function, msi_cap + 12, msi_data);
    } else {
        pci_write_config_u16(bus, device, function, msi_cap + 8, msi_data);
    }

    // Enable MSI with requested vector count
    let new_ctrl = (msg_ctrl & !0x70) | ((actual_log2 as u16 & 0x7) << 4) | 1;
    pci_write_config_u16(bus, device, function, msi_cap + 2, new_ctrl);

    disable_intx(bus, device, function);

    serial_println!(
        "  PCI: MSI multi-vector configured for {:02x}:{:02x}.{} base_vec={} count={}",
        bus,
        device,
        function,
        base_vector,
        1u16 << actual_log2
    );
    true
}

/// Get the PCIe device/port type from the PCIe capability
/// Returns: 0=Endpoint, 1=Legacy EP, 4=Root Port, 5=Upstream Switch, 6=Downstream Switch, etc.
pub fn get_pcie_device_type(bus: u8, device: u8, function: u8) -> Option<u8> {
    let caps = walk_capabilities(bus, device, function);
    let pcie_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_PCIE) {
        Some(c) => c.offset,
        None => return None,
    };

    // PCIe Capabilities Register is at cap + 0x02
    let pcie_caps = pci_read_config_u16(bus, device, function, pcie_cap + 2);
    let device_type = ((pcie_caps >> 4) & 0xF) as u8;
    Some(device_type)
}

/// Get PCIe maximum payload size (in bytes)
pub fn get_pcie_max_payload(bus: u8, device: u8, function: u8) -> Option<u32> {
    let caps = walk_capabilities(bus, device, function);
    let pcie_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_PCIE) {
        Some(c) => c.offset,
        None => return None,
    };

    // Device Capabilities Register at cap + 0x04
    let dev_caps = pci_read_config(bus, device, function, pcie_cap + 4);
    let mps = dev_caps & 0x7; // bits 2:0
    Some(128 << mps) // 128, 256, 512, 1024, 2048, 4096
}

/// Set PCIe maximum payload size
pub fn set_pcie_max_payload(bus: u8, device: u8, function: u8, size_log2_minus7: u8) -> bool {
    let caps = walk_capabilities(bus, device, function);
    let pcie_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_PCIE) {
        Some(c) => c.offset,
        None => return false,
    };

    // Device Control Register at cap + 0x08
    let dev_ctrl = pci_read_config_u16(bus, device, function, pcie_cap + 8);
    let new_ctrl = (dev_ctrl & !0xE0) | ((size_log2_minus7 as u16 & 0x7) << 5);
    pci_write_config_u16(bus, device, function, pcie_cap + 8, new_ctrl);
    true
}

/// Enable/disable PCIe relaxed ordering
pub fn set_pcie_relaxed_ordering(bus: u8, device: u8, function: u8, enable: bool) -> bool {
    let caps = walk_capabilities(bus, device, function);
    let pcie_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_PCIE) {
        Some(c) => c.offset,
        None => return false,
    };

    let dev_ctrl = pci_read_config_u16(bus, device, function, pcie_cap + 8);
    let new_ctrl = if enable {
        dev_ctrl | (1 << 4) // enable relaxed ordering
    } else {
        dev_ctrl & !(1 << 4)
    };
    pci_write_config_u16(bus, device, function, pcie_cap + 8, new_ctrl);
    true
}

/// Enable/disable PCIe no-snoop
pub fn set_pcie_no_snoop(bus: u8, device: u8, function: u8, enable: bool) -> bool {
    let caps = walk_capabilities(bus, device, function);
    let pcie_cap = match caps.iter().find(|c| c.id == PCI_CAP_ID_PCIE) {
        Some(c) => c.offset,
        None => return false,
    };

    let dev_ctrl = pci_read_config_u16(bus, device, function, pcie_cap + 8);
    let new_ctrl = if enable {
        dev_ctrl | (1 << 11)
    } else {
        dev_ctrl & !(1 << 11)
    };
    pci_write_config_u16(bus, device, function, pcie_cap + 8, new_ctrl);
    true
}

/// Get vendor name string for known vendors
pub fn vendor_name(vendor_id: u16) -> &'static str {
    match vendor_id {
        vendors::INTEL => "Intel",
        vendors::AMD => "AMD",
        vendors::NVIDIA => "NVIDIA",
        vendors::QEMU_VIRTIO => "Red Hat/Virtio",
        vendors::REALTEK => "Realtek",
        vendors::BROADCOM => "Broadcom",
        vendors::QUALCOMM => "Qualcomm/Atheros",
        vendors::VMWARE => "VMware",
        0x1234 => "Bochs/QEMU",
        0x1002 => "AMD/ATI",
        0x1D6B => "Linux Foundation",
        0x15B7 => "Sandisk/WD",
        0x144D => "Samsung",
        0x1179 => "Toshiba",
        0x1C5C => "SK Hynix",
        0x126F => "Silicon Motion",
        0x1987 => "Phison",
        _ => "Unknown",
    }
}

/// Find devices matching a predicate function
pub fn find_devices<F>(predicate: F) -> Vec<PciDevice>
where
    F: Fn(&PciDevice) -> bool,
{
    PCI_DEVICES
        .lock()
        .iter()
        .filter(|d| predicate(d))
        .cloned()
        .collect()
}

/// Write an 8-bit value to PCI configuration space
fn pci_write_config_u8(bus: u8, device: u8, function: u8, offset: u8, value: u8) {
    let aligned = offset & 0xFC;
    let shift = (offset & 3) * 8;
    let old = pci_read_config(bus, device, function, aligned);
    let mask = !(0xFFu32 << shift);
    let new_val = (old & mask) | ((value as u32) << shift);
    pci_write_config(bus, device, function, aligned, new_val);
}

/// Read the PCI interrupt pin (INTA=1, INTB=2, INTC=3, INTD=4, 0=none)
pub fn get_interrupt_pin(bus: u8, device: u8, function: u8) -> u8 {
    pci_read_config_u8(bus, device, function, PCI_INTERRUPT_PIN)
}

/// Set the PCI interrupt line (for routing)
pub fn set_interrupt_line(bus: u8, device: u8, function: u8, irq: u8) {
    pci_write_config_u8(bus, device, function, PCI_INTERRUPT_LINE, irq);
}

/// Get the current PCI command register value
pub fn get_command(bus: u8, device: u8, function: u8) -> u16 {
    pci_read_config_u16(bus, device, function, PCI_COMMAND)
}

/// Set the PCI cache line size (typically 64 bytes / 16 DWORDs on x86)
pub fn set_cache_line_size(bus: u8, device: u8, function: u8, cls: u8) {
    pci_write_config_u8(bus, device, function, PCI_CACHE_LINE_SIZE, cls);
}

/// Get the subsystem vendor and device IDs (for type 0 headers)
pub fn get_subsystem_ids(bus: u8, device: u8, function: u8) -> (u16, u16) {
    let subsys = pci_read_config(bus, device, function, 0x2C);
    let subsys_vendor = (subsys & 0xFFFF) as u16;
    let subsys_device = ((subsys >> 16) & 0xFFFF) as u16;
    (subsys_vendor, subsys_device)
}

// ---------------------------------------------------------------------------
// TASK 4 — PCI device tree and sysfs export
// ---------------------------------------------------------------------------
//
// `PCI_DEVICE_TREE` is a flat array of all discovered PCI devices (up to 256).
// `scan_pci_bus()` populates it by walking buses 0-255 / devices 0-31 / functions 0-7.
// `pci_sysfs_read()` implements `/sys/bus/pci/devices/{bus:dev.fn}/{attr}` paths.
//
// Path format: `/sys/bus/pci/devices/0000:{bus:02x}:{dev:02x}.{fn}/vendor`
// where the segment is always "0000" for conventional PCI.

/// Maximum devices stored in the flat device tree
const PCI_TREE_MAX: usize = 256;

/// Flat device tree (distinct from the Vec-based PCI_DEVICES so it can be
/// addressed by fixed index without a heap allocation per entry).
static PCI_DEVICE_TREE: Mutex<[Option<PciDevice>; PCI_TREE_MAX]> = Mutex::new({
    const NONE_DEV: Option<PciDevice> = None;
    [NONE_DEV; PCI_TREE_MAX]
});

/// Scan PCI buses 0-255, devices 0-31, functions 0-7.
/// Adds each valid device (vendor_id != 0xFFFF) to `PCI_DEVICE_TREE`.
/// This provides a secondary flat view used by the sysfs interface.
///
/// Must be called after `init()` so that `PCI_DEVICES` is already populated;
/// we simply copy entries from there to avoid a duplicate hardware scan.
pub fn scan_pci_bus() {
    let src = PCI_DEVICES.lock();
    let mut tree = PCI_DEVICE_TREE.lock();
    let mut idx = 0usize;

    for dev in src.iter() {
        if idx >= PCI_TREE_MAX {
            break;
        }
        tree[idx] = Some(dev.clone());
        idx += 1;
    }

    // If PCI_DEVICES was empty (e.g., called before init()), fall back to a
    // fresh hardware scan so scan_pci_bus() can stand alone.
    if idx == 0 {
        drop(src);
        drop(tree);
        // Walk all buses/devices/functions
        let mut tree2 = PCI_DEVICE_TREE.lock();
        let mut count = 0usize;
        'outer: for bus in 0u8..=255 {
            for device in 0u8..32 {
                if count >= PCI_TREE_MAX {
                    break 'outer;
                }
                // Function 0 always present for device to exist
                if let Some(d) = probe_device(bus, device, 0) {
                    let multi_fn = d.header_type & 0x80 != 0;
                    tree2[count] = Some(d);
                    count += 1;
                    if multi_fn {
                        for func in 1u8..8 {
                            if count >= PCI_TREE_MAX {
                                break;
                            }
                            if let Some(d) = probe_device(bus, device, func) {
                                tree2[count] = Some(d);
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
        serial_println!("  PCI: scan_pci_bus() found {} devices", count);
    } else {
        serial_println!(
            "  PCI: scan_pci_bus() populated {} entries from PCI_DEVICES",
            idx
        );
    }
}

/// Return the number of entries in the flat device tree.
pub fn pci_tree_count() -> usize {
    PCI_DEVICE_TREE
        .lock()
        .iter()
        .filter(|s| s.is_some())
        .count()
}

/// Iterate flat device tree entries (returns cloned Vec for simplicity).
pub fn pci_tree_devices() -> Vec<PciDevice> {
    PCI_DEVICE_TREE
        .lock()
        .iter()
        .filter_map(|s| s.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// sysfs read helpers
// ---------------------------------------------------------------------------

/// Format a u16 as a lowercase 4-digit hex string with "0x" prefix, e.g. "0x8086\n"
fn fmt_hex4(val: u16) -> alloc::string::String {
    let nibbles = [
        (val >> 12) as u8 & 0xF,
        (val >> 8) as u8 & 0xF,
        (val >> 4) as u8 & 0xF,
        (val) as u8 & 0xF,
    ];
    let mut s = alloc::string::String::from("0x");
    for n in nibbles.iter() {
        let c = if *n < 10 { b'0' + n } else { b'a' + n - 10 };
        s.push(c as char);
    }
    s.push('\n');
    s
}

/// Format a 24-bit class code as a 6-hex-digit string, e.g. "0x020000\n"
fn fmt_hex6(val: u32) -> alloc::string::String {
    let val = val & 0xFF_FFFF;
    let nibbles = [
        (val >> 20) as u8 & 0xF,
        (val >> 16) as u8 & 0xF,
        (val >> 12) as u8 & 0xF,
        (val >> 8) as u8 & 0xF,
        (val >> 4) as u8 & 0xF,
        (val) as u8 & 0xF,
    ];
    let mut s = alloc::string::String::from("0x");
    for n in nibbles.iter() {
        let c = if *n < 10 { b'0' + n } else { b'a' + n - 10 };
        s.push(c as char);
    }
    s.push('\n');
    s
}

/// Format a decimal u8 (IRQ) as a string, e.g. "11\n"
fn fmt_decimal(val: u8) -> alloc::string::String {
    let mut s = alloc::string::String::new();
    if val >= 100 {
        s.push(('0' as u8 + val / 100) as char);
    }
    if val >= 10 {
        s.push(('0' as u8 + (val / 10) % 10) as char);
    }
    s.push(('0' as u8 + val % 10) as char);
    s.push('\n');
    s
}

/// Parse a BDF string of the form "SSSS:BB:DD.F" or "BB:DD.F".
/// Returns (bus, device, function) on success, None otherwise.
fn parse_bdf(bdf: &str) -> Option<(u8, u8, u8)> {
    // Try "SSSS:BB:DD.F" first (with segment prefix)
    let rest = if bdf.len() >= 5 && bdf.as_bytes()[4] == b':' {
        &bdf[5..] // skip "SSSS:"
    } else {
        bdf
    };

    // rest should now be "BB:DD.F"
    let colon = rest.find(':')?;
    let dot = rest.find('.')?;
    if dot <= colon {
        return None;
    }

    let bus_str = &rest[..colon];
    let dev_str = &rest[colon + 1..dot];
    let func_str = &rest[dot + 1..];

    fn parse_hex(s: &str) -> Option<u8> {
        let mut val = 0u16;
        for c in s.bytes() {
            let digit = match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                b'A'..=b'F' => c - b'A' + 10,
                _ => return None,
            };
            val = val * 16 + digit as u16;
        }
        if val > 255 {
            return None;
        }
        Some(val as u8)
    }

    Some((
        parse_hex(bus_str)?,
        parse_hex(dev_str)?,
        parse_hex(func_str)?,
    ))
}

/// Read a sysfs attribute for a PCI device.
///
/// `path` format: `/sys/bus/pci/devices/0000:{bus:02x}:{dev:02x}.{fn}/{attr}`
///
/// Supported attributes:
/// - `vendor`  — 4-hex vendor ID (e.g. "0x8086\n")
/// - `device`  — 4-hex device ID
/// - `class`   — 6-hex class code (class[23:16] | subclass[15:8] | prog_if[7:0])
/// - `irq`     — decimal IRQ number
///
/// Returns the number of bytes written into `buf`.
/// Returns 0 if the path is not recognised or the device does not exist.
pub fn pci_sysfs_read(path: &str, buf: &mut [u8]) -> usize {
    // Expected prefix: /sys/bus/pci/devices/
    const PREFIX: &str = "/sys/bus/pci/devices/";
    if !path.starts_with(PREFIX) {
        return 0;
    }
    let rest = &path[PREFIX.len()..];

    // Split at the last '/' to get BDF and attribute name
    let slash = match rest.rfind('/') {
        Some(i) => i,
        None => return 0,
    };
    let bdf_str = &rest[..slash];
    let attr = &rest[slash + 1..];

    let (bus, dev, func) = match parse_bdf(bdf_str) {
        Some(t) => t,
        None => return 0,
    };

    // Look up the device in the tree
    let tree = PCI_DEVICE_TREE.lock();
    let pdev = tree
        .iter()
        .filter_map(|s| s.as_ref())
        .find(|d| d.bus == bus && d.device == dev && d.function == func);

    let pdev = match pdev {
        Some(d) => d,
        None => {
            // Device not in tree yet; try a live config-space read
            drop(tree);
            return pci_sysfs_read_live(bus, dev, func, attr, buf);
        }
    };

    let content = match attr {
        "vendor" => fmt_hex4(pdev.vendor_id),
        "device" => fmt_hex4(pdev.device_id),
        "class" => {
            let code: u32 =
                ((pdev.class as u32) << 16) | ((pdev.subclass as u32) << 8) | (pdev.prog_if as u32);
            fmt_hex6(code)
        }
        "irq" => fmt_decimal(pdev.interrupt_line),
        "subsystem_vendor" => {
            let (sv, _) = get_subsystem_ids(bus, dev, func);
            fmt_hex4(sv)
        }
        "subsystem_device" => {
            let (_, sd) = get_subsystem_ids(bus, dev, func);
            fmt_hex4(sd)
        }
        "revision" => fmt_hex4(pdev.revision_id as u16),
        _ => return 0,
    };

    let bytes = content.as_bytes();
    let n = bytes.len().min(buf.len());
    buf[..n].copy_from_slice(&bytes[..n]);
    n
}

/// Live sysfs read for a device not yet in the device tree.
/// Used as a fallback when the tree has not been populated.
fn pci_sysfs_read_live(bus: u8, dev: u8, func: u8, attr: &str, buf: &mut [u8]) -> usize {
    let vendor_device = pci_read_config(bus, dev, func, 0);
    let vendor_id = (vendor_device & 0xFFFF) as u16;
    if vendor_id == 0xFFFF {
        return 0;
    } // no device

    let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;
    let class_info = pci_read_config(bus, dev, func, 0x08);
    let class = ((class_info >> 24) & 0xFF) as u8;
    let subclass = ((class_info >> 16) & 0xFF) as u8;
    let prog_if = ((class_info >> 8) & 0xFF) as u8;
    let irq = (pci_read_config(bus, dev, func, 0x3C) & 0xFF) as u8;

    let content = match attr {
        "vendor" => fmt_hex4(vendor_id),
        "device" => fmt_hex4(device_id),
        "class" => {
            let code: u32 = ((class as u32) << 16) | ((subclass as u32) << 8) | (prog_if as u32);
            fmt_hex6(code)
        }
        "irq" => fmt_decimal(irq),
        _ => return 0,
    };

    let bytes = content.as_bytes();
    let n = bytes.len().min(buf.len());
    buf[..n].copy_from_slice(&bytes[..n]);
    n
}

/// List all sysfs entries under `/sys/bus/pci/devices/`.
///
/// Returns a Vec of BDF strings in the form "0000:BB:DD.F" for each device
/// that has been discovered.  These are the directory names under the prefix.
pub fn pci_sysfs_list_devices() -> Vec<alloc::string::String> {
    PCI_DEVICE_TREE
        .lock()
        .iter()
        .filter_map(|s| s.as_ref())
        .map(|d| alloc::format!("0000:{:02x}:{:02x}.{}", d.bus, d.device, d.function))
        .collect()
}

/// Register all discovered PCI devices in the sysfs tree.
///
/// This function should be called after `scan_pci_bus()` to expose each
/// device under `/sys/bus/pci/devices/{bdf}/`.
/// It creates sysfs directories and wires up read functions for the standard
/// attributes (vendor, device, class, irq).
///
/// In the Genesis sysfs implementation each attribute is backed by a
/// `fn() -> String` read function.  Since we cannot easily close over the
/// BDF per-attribute with fn pointers, we add a directory entry for each
/// device and rely on `pci_sysfs_read()` to serve the actual content.
pub fn register_pci_sysfs() {
    let devices = pci_sysfs_list_devices();
    let count = devices.len();

    for bdf in devices {
        let path = alloc::format!("/sys/bus/pci/devices/{}", bdf);
        crate::fs::sysfs::add_pci_device_dir(&path);
    }

    serial_println!("  PCI: registered {} device(s) in sysfs", count);
}
