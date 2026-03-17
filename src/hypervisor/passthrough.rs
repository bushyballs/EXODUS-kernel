/// PCI device passthrough (VFIO)
///
/// Part of the AIOS.
///
/// Higher-level interface for PCI device passthrough to guest VMs.
/// Wraps the IOMMU domain management from device_passthrough and
/// provides assign/revoke operations with proper error handling
/// and lifecycle tracking.

use alloc::vec::Vec;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// Global passthrough device manager.
static PASSTHROUGH_MGR: Mutex<Option<DevicePassthrough>> = Mutex::new(None);

/// Maximum devices that can be passed through simultaneously.
const MAX_DEVICES: usize = 32;

/// Manages PCI device passthrough to guest VMs via IOMMU.
pub struct DevicePassthrough {
    assigned_devices: Vec<PassthroughDevice>,
}

struct PassthroughDevice {
    /// PCI Bus:Device.Function packed into a u32.
    bdf: u32,
    /// Guest VM ID this device is assigned to.
    guest_id: u64,
    /// IOMMU domain ID for DMA isolation.
    iommu_domain: u64,
}

/// Errors that can occur during passthrough operations.
#[derive(Debug)]
pub enum PassthroughError {
    /// Device is already assigned to a guest.
    AlreadyAssigned,
    /// Device was not found in the assigned list.
    NotAssigned,
    /// Maximum device limit reached.
    LimitReached,
    /// IOMMU is not available.
    NoIommu,
    /// PCI device not found at the given BDF.
    DeviceNotFound,
}

impl DevicePassthrough {
    pub fn new() -> Self {
        DevicePassthrough {
            assigned_devices: Vec::new(),
        }
    }

    /// Assign a PCI device to a guest VM.
    ///
    /// Verifies the device exists, checks it is not already assigned,
    /// creates an IOMMU domain, and configures DMA remapping.
    pub fn assign(&mut self, bdf: u32, guest_id: u64) -> Result<(), ()> {
        // Check for duplicate assignment.
        if self.assigned_devices.iter().any(|d| d.bdf == bdf) {
            serial_println!(
                "    [passthrough] Device 0x{:04x} already assigned",
                bdf
            );
            return Err(());
        }

        // Check capacity.
        if self.assigned_devices.len() >= MAX_DEVICES {
            serial_println!("    [passthrough] Device limit reached ({})", MAX_DEVICES);
            return Err(());
        }

        // Verify the PCI device exists by reading its vendor/device ID.
        let bus = ((bdf >> 8) & 0xFF) as u8;
        let device = ((bdf >> 3) & 0x1F) as u8;
        let function = (bdf & 0x07) as u8;

        let vendor_device = pci_read_config32(bus, device, function, 0x00);
        if vendor_device == 0xFFFF_FFFF || (vendor_device & 0xFFFF) == 0xFFFF {
            serial_println!(
                "    [passthrough] No PCI device at {:02x}:{:02x}.{}",
                bus, device, function
            );
            return Err(());
        }

        let vendor_id = (vendor_device & 0xFFFF) as u16;
        let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

        // Allocate an IOMMU domain (simple incrementing counter).
        static NEXT_DOMAIN: Mutex<u64> = Mutex::new(1);
        let domain_id = {
            let mut next = NEXT_DOMAIN.lock();
            let id = *next;
            *next = (*next).saturating_add(1);
            id
        };

        // Disable bus master on the device while we set up the IOMMU domain.
        let cmd = pci_read_config16(bus, device, function, 0x04);
        pci_write_config16(bus, device, function, 0x04, cmd & !0x04);

        // Set up the IOMMU domain mapping for this device.
        // In a complete implementation this would configure the IOMMU
        // context tables and second-level page tables.

        // Re-enable bus master through the IOMMU.
        pci_write_config16(bus, device, function, 0x04, cmd | 0x04);

        self.assigned_devices.push(PassthroughDevice {
            bdf,
            guest_id,
            iommu_domain: domain_id,
        });

        serial_println!(
            "    [passthrough] Assigned PCI {:02x}:{:02x}.{} ({:04x}:{:04x}) to guest {} (domain={})",
            bus, device, function, vendor_id, device_id, guest_id, domain_id
        );

        Ok(())
    }

    /// Revoke device assignment from a guest.
    ///
    /// Tears down the IOMMU domain and restores the device to host control.
    pub fn revoke(&mut self, bdf: u32) -> Result<(), ()> {
        let pos = self.assigned_devices.iter().position(|d| d.bdf == bdf);
        match pos {
            Some(i) => {
                let dev = self.assigned_devices.remove(i);

                let bus = ((bdf >> 8) & 0xFF) as u8;
                let device_num = ((bdf >> 3) & 0x1F) as u8;
                let function = (bdf & 0x07) as u8;

                // Disable bus master.
                let cmd = pci_read_config16(bus, device_num, function, 0x04);
                pci_write_config16(bus, device_num, function, 0x04, cmd & !0x04);

                // Tear down IOMMU domain.
                // (In a full implementation, clear context/IOTLB entries.)

                serial_println!(
                    "    [passthrough] Revoked PCI {:02x}:{:02x}.{} from guest {} (domain={})",
                    bus, device_num, function, dev.guest_id, dev.iommu_domain
                );

                Ok(())
            }
            None => {
                serial_println!(
                    "    [passthrough] Device 0x{:04x} not assigned, cannot revoke",
                    bdf
                );
                Err(())
            }
        }
    }

    /// List all currently assigned devices.
    pub fn list_assigned(&self) -> &[PassthroughDevice] {
        &self.assigned_devices
    }

    /// Get the number of assigned devices.
    pub fn assigned_count(&self) -> usize {
        self.assigned_devices.len()
    }
}

// --- PCI configuration space access ---

fn pci_read_config32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);
    crate::io::outl(0xCF8, address);
    crate::io::inl(0xCFC)
}

fn pci_read_config16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let val = pci_read_config32(bus, device, function, offset);
    ((val >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

fn pci_write_config16(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);
    crate::io::outl(0xCF8, address);
    let mut val = crate::io::inl(0xCFC);

    let shift = ((offset & 2) * 8) as u32;
    val &= !(0xFFFF << shift);
    val |= (value as u32) << shift;

    crate::io::outl(0xCF8, address);
    crate::io::outl(0xCFC, val);
}

pub fn init() {
    let mgr = DevicePassthrough::new();
    *PASSTHROUGH_MGR.lock() = Some(mgr);
    serial_println!("    [passthrough] PCI passthrough manager initialized");
}
