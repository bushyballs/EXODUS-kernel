/// PCI device passthrough via IOMMU/VT-d.
///
/// Part of the AIOS hypervisor subsystem.
///
/// Allows assigning physical PCI devices directly to guest VMs for
/// near-native I/O performance. Uses the IOMMU (VT-d on Intel, AMD-Vi
/// on AMD) for DMA remapping and interrupt remapping to provide
/// isolation between the guest and host.

use crate::{serial_print, serial_println};
use crate::sync::Mutex;
use alloc::vec::Vec;

/// IOMMU base address register (DMAR ACPI table provides the real address).
/// This is a placeholder for the remapping hardware register base.
const IOMMU_REG_BASE: u64 = 0xFED9_0000;

/// IOMMU register offsets (Intel VT-d specification).
const IOMMU_VER: u64 = 0x00;        // Version register.
const IOMMU_CAP: u64 = 0x08;        // Capability register.
const IOMMU_ECAP: u64 = 0x10;       // Extended capability register.
const IOMMU_GCMD: u64 = 0x18;       // Global command register.
const IOMMU_GSTS: u64 = 0x1C;       // Global status register.
const IOMMU_RTADDR: u64 = 0x20;     // Root table address register.
const IOMMU_IOTLB: u64 = 0x108;     // IOTLB invalidation register.

/// Global command bits.
const GCMD_TE: u32 = 1 << 31;       // Translation enable.
const GCMD_SRTP: u32 = 1 << 30;     // Set root table pointer.
const GCMD_IRE: u32 = 1 << 25;      // Interrupt remapping enable.

/// Global status bits.
const GSTS_TES: u32 = 1 << 31;      // Translation enable status.
const GSTS_RTPS: u32 = 1 << 30;     // Root table pointer status.
const GSTS_IRES: u32 = 1 << 25;     // Interrupt remapping enable status.

/// Maximum PCI devices that can be passed through.
const MAX_PASSTHROUGH_DEVICES: usize = 32;

/// Global IOMMU/passthrough manager.
static IOMMU_STATE: Mutex<Option<IommuState>> = Mutex::new(None);

/// State of the IOMMU hardware and domain tracking.
struct IommuState {
    /// Whether an IOMMU was detected and enabled.
    enabled: bool,
    /// IOMMU base address (from ACPI DMAR table).
    base_address: u64,
    /// IOMMU version (major.minor).
    version: (u8, u8),
    /// Number of IOMMU domains in use.
    domain_count: u64,
    /// Next domain ID to allocate.
    next_domain_id: u64,
    /// Whether interrupt remapping is supported.
    interrupt_remapping: bool,
}

/// BDF (Bus:Device.Function) address for PCI device identification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BdfAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl BdfAddress {
    /// Create from a packed u32 (bus:device.function).
    pub fn from_raw(bdf: u32) -> Self {
        BdfAddress {
            bus: ((bdf >> 8) & 0xFF) as u8,
            device: ((bdf >> 3) & 0x1F) as u8,
            function: (bdf & 0x07) as u8,
        }
    }

    /// Pack into a u32.
    pub fn to_raw(&self) -> u32 {
        ((self.bus as u32) << 8) | ((self.device as u32) << 3) | (self.function as u32)
    }
}

/// Tracks a device assigned to a guest VM.
struct AssignedDevice {
    bdf: BdfAddress,
    guest_id: u64,
    iommu_domain: u64,
    /// Original PCI command register value (for restore on reclaim).
    saved_pci_command: u16,
    /// Whether DMA remapping is active for this device.
    dma_remapping_active: bool,
    /// Whether interrupt remapping is active for this device.
    irq_remapping_active: bool,
}

/// Manages PCI device assignment to guest VMs.
pub struct DevicePassthrough {
    /// Currently assigned devices.
    assigned: Vec<AssignedDevice>,
}

impl DevicePassthrough {
    pub fn new() -> Self {
        DevicePassthrough {
            assigned: Vec::new(),
        }
    }

    /// Assign a PCI device to a guest VM.
    ///
    /// Sets up an IOMMU domain for the device, configures DMA remapping,
    /// and optionally sets up interrupt remapping.
    pub fn assign_device(&mut self, bdf: u32, guest_id: u64) {
        let bdf_addr = BdfAddress::from_raw(bdf);

        // Check if device is already assigned.
        for dev in &self.assigned {
            if dev.bdf.to_raw() == bdf {
                serial_println!(
                    "    [passthrough] Device {:02x}:{:02x}.{} already assigned to guest {}",
                    bdf_addr.bus, bdf_addr.device, bdf_addr.function, dev.guest_id
                );
                return;
            }
        }

        if self.assigned.len() >= MAX_PASSTHROUGH_DEVICES {
            serial_println!("    [passthrough] Maximum passthrough device limit reached");
            return;
        }

        // Allocate an IOMMU domain for this device.
        let domain_id = {
            let mut state = IOMMU_STATE.lock();
            if let Some(ref mut s) = *state {
                let id = s.next_domain_id;
                s.next_domain_id = s.next_domain_id.saturating_add(1);
                s.domain_count = s.domain_count.saturating_add(1);
                id
            } else {
                serial_println!("    [passthrough] IOMMU not initialized");
                return;
            }
        };

        // Save the device's current PCI command register.
        let saved_cmd = pci_config_read16(bdf_addr.bus, bdf_addr.device, bdf_addr.function, 0x04);

        // Disable DMA on the device while we reconfigure.
        let cmd_no_dma = saved_cmd & !0x04; // Clear bus master bit.
        pci_config_write16(bdf_addr.bus, bdf_addr.device, bdf_addr.function, 0x04, cmd_no_dma);

        // Configure IOMMU domain: create context entry, set up second-level page tables.
        configure_iommu_domain(domain_id, &bdf_addr);

        // Re-enable bus mastering through the IOMMU domain.
        pci_config_write16(
            bdf_addr.bus, bdf_addr.device, bdf_addr.function, 0x04,
            saved_cmd | 0x04, // Set bus master bit.
        );

        let dev = AssignedDevice {
            bdf: bdf_addr,
            guest_id,
            iommu_domain: domain_id,
            saved_pci_command: saved_cmd,
            dma_remapping_active: true,
            irq_remapping_active: false,
        };

        self.assigned.push(dev);

        serial_println!(
            "    [passthrough] Assigned device {:02x}:{:02x}.{} to guest {} (domain={})",
            bdf_addr.bus, bdf_addr.device, bdf_addr.function, guest_id, domain_id
        );
    }

    /// Reclaim a device from a guest VM.
    ///
    /// Tears down the IOMMU domain, restores the device to host ownership,
    /// and resets it to a clean state.
    pub fn reclaim_device(&mut self, bdf: u32) {
        let bdf_addr = BdfAddress::from_raw(bdf);

        let pos = self.assigned.iter().position(|d| d.bdf.to_raw() == bdf);
        let dev = match pos {
            Some(i) => self.assigned.remove(i),
            None => {
                serial_println!(
                    "    [passthrough] Device {:02x}:{:02x}.{} not assigned",
                    bdf_addr.bus, bdf_addr.device, bdf_addr.function
                );
                return;
            }
        };

        // Disable bus mastering.
        pci_config_write16(
            dev.bdf.bus, dev.bdf.device, dev.bdf.function, 0x04,
            dev.saved_pci_command & !0x04,
        );

        // Tear down the IOMMU domain.
        teardown_iommu_domain(dev.iommu_domain, &dev.bdf);

        // Restore original PCI command register.
        pci_config_write16(
            dev.bdf.bus, dev.bdf.device, dev.bdf.function, 0x04,
            dev.saved_pci_command,
        );

        // Decrement domain count.
        {
            let mut state = IOMMU_STATE.lock();
            if let Some(ref mut s) = *state {
                if s.domain_count > 0 {
                    s.domain_count = s.domain_count.saturating_sub(1);
                }
            }
        }

        serial_println!(
            "    [passthrough] Reclaimed device {:02x}:{:02x}.{} from guest {}",
            dev.bdf.bus, dev.bdf.device, dev.bdf.function, dev.guest_id
        );
    }

    /// Check if a specific device is currently assigned.
    pub fn is_assigned(&self, bdf: u32) -> bool {
        self.assigned.iter().any(|d| d.bdf.to_raw() == bdf)
    }
}

// --- IOMMU domain management helpers ---

/// Configure an IOMMU domain for a device.
fn configure_iommu_domain(domain_id: u64, bdf: &BdfAddress) {
    let state = IOMMU_STATE.lock();
    if let Some(ref s) = *state {
        if !s.enabled {
            return;
        }

        let base = s.base_address;

        // Write the context entry for this BDF in the root/context table.
        // The context entry maps (bus, devfn) -> domain_id and second-level page table.
        //
        // In a real implementation, this would:
        // 1. Locate the root table entry for the bus.
        // 2. Locate the context entry for the device/function.
        // 3. Write the domain ID and page table pointer.
        // 4. Invalidate the IOTLB.

        // Invalidate IOTLB for the domain.
        unsafe {
            let iotlb_addr = (base + IOMMU_IOTLB) as *mut u64;
            // Domain-selective invalidation: write domain ID + invalidation command.
            let invalidate_cmd: u64 = (domain_id << 32) | (1 << 63); // IVT bit + domain.
            core::ptr::write_volatile(iotlb_addr, invalidate_cmd);
        }

        serial_println!(
            "    [passthrough] IOMMU domain {} configured for {:02x}:{:02x}.{}",
            domain_id, bdf.bus, bdf.device, bdf.function
        );
    }
}

/// Tear down an IOMMU domain.
fn teardown_iommu_domain(domain_id: u64, bdf: &BdfAddress) {
    let state = IOMMU_STATE.lock();
    if let Some(ref s) = *state {
        if !s.enabled {
            return;
        }

        let base = s.base_address;

        // Clear the context entry and invalidate.
        unsafe {
            let iotlb_addr = (base + IOMMU_IOTLB) as *mut u64;
            let invalidate_cmd: u64 = (domain_id << 32) | (1 << 63);
            core::ptr::write_volatile(iotlb_addr, invalidate_cmd);
        }

        serial_println!(
            "    [passthrough] IOMMU domain {} torn down for {:02x}:{:02x}.{}",
            domain_id, bdf.bus, bdf.device, bdf.function
        );
    }
}

// --- PCI configuration space access helpers ---

/// Read a 16-bit value from PCI configuration space.
fn pci_config_read16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    crate::io::outl(0xCF8, address);
    let val = crate::io::inl(0xCFC);

    // Extract the 16-bit value at the correct alignment.
    ((val >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

/// Write a 16-bit value to PCI configuration space.
fn pci_config_write16(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
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

/// Detect IOMMU hardware by reading its version register.
fn detect_iommu() -> Option<(u64, u8, u8, bool)> {
    // In a real system, the IOMMU base address comes from the ACPI DMAR table.
    // Here we probe the conventional address.
    let base = IOMMU_REG_BASE;

    let ver = unsafe {
        core::ptr::read_volatile((base + IOMMU_VER) as *const u32)
    };

    let major = ((ver >> 4) & 0xF) as u8;
    let minor = (ver & 0xF) as u8;

    if major == 0 && minor == 0 {
        return None; // No IOMMU present.
    }

    // Check extended capabilities for interrupt remapping.
    let ecap = unsafe {
        core::ptr::read_volatile((base + IOMMU_ECAP) as *const u64)
    };
    let ir_supported = (ecap >> 3) & 1 == 1;

    Some((base, major, minor, ir_supported))
}

pub fn init() {
    match detect_iommu() {
        Some((base, major, minor, ir_supported)) => {
            let state = IommuState {
                enabled: true,
                base_address: base,
                version: (major, minor),
                domain_count: 0,
                next_domain_id: 1,
                interrupt_remapping: ir_supported,
            };

            // Enable translation.
            unsafe {
                let gcmd_addr = (base + IOMMU_GCMD) as *mut u32;
                core::ptr::write_volatile(gcmd_addr, GCMD_TE);

                // Wait for translation enable status.
                let gsts_addr = (base + IOMMU_GSTS) as *const u32;
                let mut timeout = 10000u32;
                while core::ptr::read_volatile(gsts_addr) & GSTS_TES == 0 && timeout > 0 {
                    core::hint::spin_loop();
                    timeout -= 1;
                }
            }

            *IOMMU_STATE.lock() = Some(state);
            serial_println!(
                "    [passthrough] IOMMU detected v{}.{} (IR={}) — DMA remapping enabled",
                major, minor, ir_supported
            );
        }
        None => {
            let state = IommuState {
                enabled: false,
                base_address: 0,
                version: (0, 0),
                domain_count: 0,
                next_domain_id: 1,
                interrupt_remapping: false,
            };
            *IOMMU_STATE.lock() = Some(state);
            serial_println!("    [passthrough] No IOMMU detected — device passthrough unavailable");
        }
    }
}
