use crate::drivers::pci;
/// PCI MSI / MSI-X interrupt support for Genesis
///
/// Implements Message Signaled Interrupts (MSI) and MSI-X for PCI/PCIe devices.
/// Both mechanisms allow a device to signal an interrupt by writing a small
/// message to a CPU-visible MMIO address rather than asserting a physical wire.
///
/// MSI:   single capability block in PCI config space, up to 32 vectors,
///        single message address + data pair.
/// MSI-X: separate in-memory table (pointed to by a BAR), up to 2048 vectors,
///        each entry has its own address/data/mask triple.
///
/// All MMIO writes to the MSI-X table MUST use `write_volatile` because the
/// hardware observes each individual store — the compiler must not reorder
/// or elide them.
///
/// Reference: PCI Local Bus Specification rev 3.0, Section 6.8 (MSI),
///            PCI Express Base Specification rev 5.0, Section 7.7.2 (MSI-X).
use crate::serial_println;

// ---------------------------------------------------------------------------
// PCI capability IDs (duplicated here so this module is self-contained)
// ---------------------------------------------------------------------------

/// MSI capability ID in PCI capability list
pub const PCI_CAP_MSI: u8 = 0x05;
/// MSI-X capability ID in PCI capability list
pub const PCI_CAP_MSIX: u8 = 0x11;

// ---------------------------------------------------------------------------
// Public data structures
// ---------------------------------------------------------------------------

/// Decoded MSI capability for a PCI device.
///
/// `cap_offset` points into PCI configuration space where the MSI capability
/// header lives.  The layout at that offset is:
///   +0  Cap ID (0x05)
///   +1  Next Pointer
///   +2  Message Control
///   +4  Message Address (lower 32 bits)
///   +8  Message Address upper 32 bits  (only when `is_64bit`)
///   +8/+12  Message Data
///   +12/+16 Mask Bits  (only when per-vector masking is supported)
#[derive(Debug, Clone, Copy)]
pub struct MsiConfig {
    /// Byte offset within PCI config space where the MSI capability starts.
    pub cap_offset: u8,
    /// True when the device advertises a 64-bit message address field.
    pub is_64bit: bool,
    /// Number of interrupt vectors requested (must be a power of two, 1–32).
    pub num_vectors: u8,
}

/// Decoded MSI-X capability for a PCI device.
///
/// The MSI-X table is an in-MMIO array of 16-byte entries:
///   Entry offset 0:  Message Address lower 32 bits
///   Entry offset 4:  Message Address upper 32 bits
///   Entry offset 8:  Message Data
///   Entry offset 12: Vector Control (bit 0 = masked)
///
/// The Pending Bit Array (PBA) is a separate MMIO bitmap, one bit per entry,
/// indicating that an interrupt is pending while the entry is masked.
#[derive(Debug, Clone, Copy)]
pub struct MsixConfig {
    /// Byte offset within PCI config space where the MSI-X capability starts.
    pub cap_offset: u8,
    /// Index of the BAR that contains the MSI-X table.
    pub table_bir: u8,
    /// Byte offset within that BAR where the MSI-X table begins.
    pub table_offset: u32,
    /// Index of the BAR that contains the Pending Bit Array.
    pub pba_bir: u8,
    /// Byte offset within the PBA BAR where the PBA begins.
    pub pba_offset: u32,
    /// Number of MSI-X table entries minus one (as stored in hardware).
    /// Actual entry count = `table_size + 1`.
    pub table_size: u16,
}

// ---------------------------------------------------------------------------
// Capability list walker
// ---------------------------------------------------------------------------

/// Walk the PCI capability list and return the config-space offset of the
/// first capability whose ID matches `cap_id`.
///
/// Returns `None` if:
///   - the device does not have the Capabilities List bit set in Status,
///   - no capability with `cap_id` is found before the end of the list,
///   - the list appears corrupted (cycle guard fires at 48 hops).
///
/// # Arguments
/// * `bus`, `dev`, `func` — PCI Bus:Device.Function address
/// * `cap_id` — the capability ID byte to search for (e.g. `PCI_CAP_MSI`)
pub fn pci_find_capability(bus: u8, dev: u8, func: u8, cap_id: u8) -> Option<u8> {
    // Status register is at PCI config offset 0x06.
    // Bit 4 (Capabilities List) must be set before offset 0x34 is valid.
    let status = pci::config_read_u16(bus, dev, func, 0x06);
    if status & (1 << 4) == 0 {
        return None;
    }

    // Capabilities pointer is at config offset 0x34.  Bits 1:0 are reserved.
    let mut ptr = (pci::config_read(bus, dev, func, 0x34) & 0xFC) as u8;

    // Guard against malformed/cyclical lists.
    let mut hops: u8 = 0;
    while ptr != 0 && hops < 48 {
        // Each capability entry begins with [ID, Next Pointer] at `ptr`.
        let id = (pci::config_read(bus, dev, func, ptr) & 0xFF) as u8;
        if id == cap_id {
            return Some(ptr);
        }
        let next = ((pci::config_read(bus, dev, func, ptr) >> 8) & 0xFC) as u8;
        ptr = next;
        hops = hops.saturating_add(1);
    }
    None
}

// ---------------------------------------------------------------------------
// MSI enable / disable
// ---------------------------------------------------------------------------

/// Enable MSI interrupts on a PCI device.
///
/// Programs the MSI Message Address to target the local APIC of the CPU
/// identified by `apic_id`, and writes `vector` into the Message Data register.
/// Enables the MSI capability (bit 0 of Message Control) and simultaneously
/// disables legacy INTx signalling via the Command register.
///
/// The message address format used here is the Intel x86 local-APIC delivery
/// format:  `0xFEE[DestID]00[RH=0][DM=0]0`, physical destination, Fixed
/// delivery mode.  Other CPU architectures would differ.
///
/// Returns `Ok(())` on success, or an error string when the device does not
/// expose an MSI capability.
///
/// # Arguments
/// * `bus`, `dev`, `func` — PCI Bus:Device.Function
/// * `apic_id` — local APIC destination ID (8-bit flat model, bits 19:12)
/// * `vector` — IDT vector number to deliver (0x20–0xFF)
pub fn pci_enable_msi(
    bus: u8,
    dev: u8,
    func: u8,
    apic_id: u8,
    vector: u8,
) -> Result<(), &'static str> {
    let cap = pci_find_capability(bus, dev, func, PCI_CAP_MSI).ok_or("MSI capability not found")?;

    // Message Control register sits at cap + 2.
    // Bit 7: 64-bit address capable.
    // Bits 3:1: Multi-Message Capable (log2 of supported vectors).
    // Bit 0: MSI Enable.
    let msg_ctrl = pci::config_read_u16(bus, dev, func, cap as u16 + 2);
    let is_64bit = (msg_ctrl & (1 << 7)) != 0;

    // Build the message address.
    // Bits 31:20 = 0xFEE (magic APIC prefix).
    // Bits 19:12 = Destination ID.
    // Bits 11:4  = 0 (reserved / RH / DM = 0).
    // Bits 3:2   = 0 (reserved).
    let msg_addr: u32 = 0xFEE0_0000u32 | ((apic_id as u32) << 12);

    // Message Data carries the vector in bits 7:0.
    // Delivery mode 000 = Fixed, trigger mode 0 = edge.
    let msg_data: u32 = vector as u32;

    // Write message address (lower 32 bits) at cap + 4.
    pci::config_write(bus, dev, func, cap.saturating_add(4), msg_addr);

    if is_64bit {
        // Upper 32-bit address at cap + 8 (set to 0 — local APIC is 32-bit).
        pci::config_write(bus, dev, func, cap.saturating_add(8), 0);
        // Message Data at cap + 12 for 64-bit capable devices.
        pci::config_write(bus, dev, func, cap.saturating_add(12), msg_data);
    } else {
        // Message Data at cap + 8 for 32-bit devices.
        pci::config_write(bus, dev, func, cap.saturating_add(8), msg_data);
    }

    // Enable MSI: clear MME (Multi-Message Enable) bits to request 1 vector,
    // then set bit 0 (MSI Enable).
    let new_ctrl = (msg_ctrl & !0x0070u16) | 0x0001u16;
    pci::config_write_u16(bus, dev, func, cap as u16 + 2, new_ctrl);

    // Disable legacy INTx (bit 10 of Command register).
    pci::disable_intx(bus, dev, func);

    serial_println!(
        "  pci_msi: MSI enabled on {:02x}:{:02x}.{} vector={:#x} apic={}",
        bus,
        dev,
        func,
        vector,
        apic_id
    );
    Ok(())
}

/// Disable MSI interrupts on a PCI device.
///
/// Clears the MSI Enable bit (bit 0 of Message Control).  Legacy INTx is
/// *not* automatically re-enabled; callers must do that separately if needed.
///
/// Does nothing (and succeeds silently) if the device has no MSI capability.
pub fn pci_disable_msi(bus: u8, dev: u8, func: u8) {
    if let Some(cap) = pci_find_capability(bus, dev, func, PCI_CAP_MSI) {
        let msg_ctrl = pci::config_read_u16(bus, dev, func, cap as u16 + 2);
        let new_ctrl = msg_ctrl & !0x0001u16; // clear Enable bit
        pci::config_write_u16(bus, dev, func, cap as u16 + 2, new_ctrl);
        serial_println!(
            "  pci_msi: MSI disabled on {:02x}:{:02x}.{}",
            bus,
            dev,
            func
        );
    }
}

// ---------------------------------------------------------------------------
// MSI-X enable
// ---------------------------------------------------------------------------

/// Enable MSI-X on a PCI device and return the number of available vectors.
///
/// This function:
///  1. Locates the MSI-X capability in the PCI config space.
///  2. Reads the table size from Message Control.
///  3. Sets the MSI-X Enable bit (bit 15) and clears the Function Mask bit
///     (bit 14) so that individual entries can be unmasked.
///  4. Disables legacy INTx.
///
/// The caller is responsible for programming individual table entries via
/// `pci_msix_set_vector()` after mapping the MSI-X table BAR.
///
/// Returns `Ok(num_vectors)` where `num_vectors` is the total entry count
/// (hardware value + 1), or an error string if the capability is missing.
///
/// # Arguments
/// * `bus`, `dev`, `func` — PCI Bus:Device.Function
pub fn pci_enable_msix(bus: u8, dev: u8, func: u8) -> Result<u16, &'static str> {
    let cap =
        pci_find_capability(bus, dev, func, PCI_CAP_MSIX).ok_or("MSI-X capability not found")?;

    // Message Control at cap + 2.
    // Bits 10:0  = Table Size (N-1, so actual size = field + 1).
    // Bit 14     = Function Mask (1 = all entries masked).
    // Bit 15     = MSI-X Enable.
    let msg_ctrl = pci::config_read_u16(bus, dev, func, cap as u16 + 2);
    let num_vectors: u16 = (msg_ctrl & 0x07FF).saturating_add(1);

    // Enable MSI-X and clear function-level mask.
    let new_ctrl = (msg_ctrl | (1u16 << 15)) & !(1u16 << 14);
    pci::config_write_u16(bus, dev, func, cap as u16 + 2, new_ctrl);

    // Disable legacy INTx.
    pci::disable_intx(bus, dev, func);

    serial_println!(
        "  pci_msi: MSI-X enabled on {:02x}:{:02x}.{} ({} vectors)",
        bus,
        dev,
        func,
        num_vectors
    );
    Ok(num_vectors)
}

/// Read the MSI-X capability into an `MsixConfig` struct.
///
/// Returns `None` when the device does not expose an MSI-X capability.
/// Does not change any hardware state.
pub fn pci_read_msix_config(bus: u8, dev: u8, func: u8) -> Option<MsixConfig> {
    let cap = pci_find_capability(bus, dev, func, PCI_CAP_MSIX)?;

    let msg_ctrl = pci::config_read_u16(bus, dev, func, cap as u16 + 2);
    let table_size = msg_ctrl & 0x07FF; // stored as N-1

    // Table BIR + Offset at cap + 4.
    let table_reg = pci::config_read(bus, dev, func, cap.saturating_add(4));
    let table_bir = (table_reg & 0x7) as u8;
    let table_offset = table_reg & !0x7u32;

    // PBA BIR + Offset at cap + 8.
    let pba_reg = pci::config_read(bus, dev, func, cap.saturating_add(8));
    let pba_bir = (pba_reg & 0x7) as u8;
    let pba_offset = pba_reg & !0x7u32;

    Some(MsixConfig {
        cap_offset: cap,
        table_bir,
        table_offset,
        pba_bir,
        pba_offset,
        table_size,
    })
}

// ---------------------------------------------------------------------------
// MSI-X table entry programming
// ---------------------------------------------------------------------------

/// Write a single MSI-X table entry to route an interrupt vector.
///
/// Each MSI-X table entry is 16 bytes at `table_base + entry * 16`:
///   +0  Message Address lower 32 bits  (0xFEE[DestID]00...)
///   +4  Message Address upper 32 bits  (0 for local APIC on x86)
///   +8  Message Data                   (vector number, edge, fixed)
///   +12 Vector Control                 (bit 0 = masked; 0 = unmasked)
///
/// All writes use `write_volatile` to prevent compiler reordering and to
/// ensure the hardware observes each write.
///
/// # Safety
/// `table_base` must be a valid MMIO virtual address for the MSI-X table
/// (obtained from the device's BAR after ioremap).  Writing to an incorrect
/// address will cause undefined hardware behaviour or a page fault.
///
/// # Arguments
/// * `table_base` — virtual address of the start of the MSI-X table
/// * `entry`      — zero-based entry index (must be < `MsixConfig::table_size + 1`)
/// * `vector`     — IDT vector to deliver (0x20–0xFF)
/// * `apic_id`    — local APIC destination ID (bits 19:12 of message address)
pub fn pci_msix_set_vector(table_base: u64, entry: u16, vector: u8, apic_id: u8) {
    // Byte offset of this entry within the table.
    let entry_offset = (entry as u64).saturating_mul(16);
    let base = table_base.saturating_add(entry_offset) as usize;

    // Message Address: 0xFEE[apic_id << 12]
    let msg_addr_lo: u32 = 0xFEE0_0000u32 | ((apic_id as u32) << 12);
    // Message Address upper: 0 (local APIC on x86 is sub-4 GiB)
    let msg_addr_hi: u32 = 0;
    // Message Data: vector, Fixed delivery, edge triggered
    let msg_data: u32 = vector as u32;
    // Vector Control: 0 = unmasked
    let vec_ctrl: u32 = 0;

    // MMIO writes must be volatile — hardware is the observer.
    unsafe {
        core::ptr::write_volatile(base as *mut u32, msg_addr_lo);
        core::ptr::write_volatile(base.saturating_add(4) as *mut u32, msg_addr_hi);
        core::ptr::write_volatile(base.saturating_add(8) as *mut u32, msg_data);
        core::ptr::write_volatile(base.saturating_add(12) as *mut u32, vec_ctrl);
    }
}

/// Mask a single MSI-X table entry (set bit 0 of Vector Control).
///
/// A masked entry will not generate any interrupt messages.  Use this before
/// reprogramming the vector to avoid spurious interrupts during the update.
///
/// # Safety
/// Same requirements as `pci_msix_set_vector`.
pub fn pci_msix_mask_entry(table_base: u64, entry: u16) {
    let base =
        (table_base.saturating_add((entry as u64).saturating_mul(16)) as usize).saturating_add(12);
    unsafe {
        let ctrl = core::ptr::read_volatile(base as *const u32);
        core::ptr::write_volatile(base as *mut u32, ctrl | 1);
    }
}

/// Unmask a single MSI-X table entry (clear bit 0 of Vector Control).
///
/// # Safety
/// Same requirements as `pci_msix_set_vector`.
pub fn pci_msix_unmask_entry(table_base: u64, entry: u16) {
    let base =
        (table_base.saturating_add((entry as u64).saturating_mul(16)) as usize).saturating_add(12);
    unsafe {
        let ctrl = core::ptr::read_volatile(base as *const u32);
        core::ptr::write_volatile(base as *mut u32, ctrl & !1u32);
    }
}

// ---------------------------------------------------------------------------
// MSI upgrade helper used by device drivers
// ---------------------------------------------------------------------------

/// Attempt to upgrade a device from legacy INTx to MSI.
///
/// Tries MSI-X first (preferred: per-vector control), then falls back to
/// plain MSI, then falls back to legacy pin-based interrupt.
///
/// Returns `true` if MSI or MSI-X was successfully configured, `false` if
/// the driver must continue using legacy INTx.
///
/// This is the recommended entry point for device drivers that previously
/// used only `dev.interrupt_line`.
///
/// # Arguments
/// * `bus`, `dev`, `func` — PCI Bus:Device.Function
/// * `apic_id` — local APIC destination ID
/// * `vector`  — IDT vector number
/// * `driver_name` — used in diagnostic messages only
pub fn try_upgrade_to_msi(
    bus: u8,
    dev: u8,
    func: u8,
    apic_id: u8,
    vector: u8,
    driver_name: &str,
) -> bool {
    // Try MSI first (simpler than MSI-X; sufficient for most drivers that
    // only need one interrupt vector).
    match pci_enable_msi(bus, dev, func, apic_id, vector) {
        Ok(()) => {
            serial_println!("  [{}] MSI enabled, vector={:#x}", driver_name, vector);
            return true;
        }
        Err(e) => {
            serial_println!(
                "  [{}] MSI not available ({}), using legacy IRQ",
                driver_name,
                e
            );
        }
    }
    false
}
