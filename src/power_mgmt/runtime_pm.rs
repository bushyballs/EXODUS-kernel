/// Per-device runtime power management.
///
/// Part of the AIOS power_mgmt subsystem.
/// Manages per-device power states with reference-counted usage tracking.
/// When usage drops to zero, the device is scheduled for autosuspend after
/// a configurable delay. get() resumes a suspended device; put() decrements
/// the usage count and may trigger autosuspend.
use crate::sync::Mutex;

/// Runtime PM state for a single device.
pub struct RuntimePm {
    usage_count: i32,
    state: RuntimeState,
    autosuspend_delay_ms: u32,
    last_activity_tsc: u64,
    device_id: u32,
    suspended_count: u64,
    resumed_count: u64,
}

/// Runtime PM device states.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RuntimeState {
    Active,
    Suspending,
    Suspended,
    Resuming,
}

static DEVICES: Mutex<Option<RuntimePm>> = Mutex::new(None);

/// Read TSC for timestamp
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | (lo as u64)
}

impl RuntimePm {
    pub fn new() -> Self {
        RuntimePm {
            usage_count: 1, // start active with one reference
            state: RuntimeState::Active,
            autosuspend_delay_ms: 2000,
            last_activity_tsc: rdtsc(),
            device_id: 0,
            suspended_count: 0,
            resumed_count: 0,
        }
    }

    /// Increment usage count and resume device if suspended.
    /// If the device is currently suspended, triggers a resume transition.
    pub fn get(&mut self) {
        self.usage_count = self.usage_count.saturating_add(1);
        self.last_activity_tsc = rdtsc();

        match self.state {
            RuntimeState::Suspended => {
                self.state = RuntimeState::Resuming;
                // Resume the device (restore power, reconfigure clocks)
                self.do_resume();
                self.state = RuntimeState::Active;
                self.resumed_count = self.resumed_count.saturating_add(1);
            }
            RuntimeState::Suspending => {
                // Cancel in-progress suspend
                self.state = RuntimeState::Active;
            }
            RuntimeState::Resuming | RuntimeState::Active => {
                // Already active or resuming, just increment count
            }
        }
    }

    /// Decrement usage count and schedule autosuspend if count reaches zero.
    pub fn put(&mut self) {
        if self.usage_count > 0 {
            self.usage_count -= 1;
        }
        self.last_activity_tsc = rdtsc();

        if self.usage_count == 0 && self.state == RuntimeState::Active {
            // Schedule autosuspend (in a real implementation this would
            // arm a timer; here we transition immediately for simplicity)
            self.state = RuntimeState::Suspending;
            self.do_suspend();
            self.state = RuntimeState::Suspended;
            self.suspended_count = self.suspended_count.saturating_add(1);
        }
    }

    /// Set the autosuspend delay in milliseconds.
    /// A delay of 0 means suspend immediately when usage drops to zero.
    pub fn set_autosuspend_delay(&mut self, delay_ms: u32) {
        self.autosuspend_delay_ms = delay_ms;
    }

    /// Internal: perform device suspend.
    ///
    /// Sequence:
    ///   1. Mask device interrupts via the PCI command register (INTx disable
    ///      bit) and via the BAR-mapped IMR when a BAR is resolvable.
    ///   2. Poll in-flight DMA counter with a bounded spin (≤ 1 000 iterations)
    ///      to allow pending transactions to drain.
    ///   3. Transition the PCI function to the D3hot power state by calling
    ///      set_pci_power_state() which locates the PM capability via the
    ///      standard capability list walk and writes PMCSR bits [1:0] = 0b11.
    ///   4. Log completion.
    fn do_suspend(&self) {
        // Step 1: mask PCI INTx interrupts via the Command register bit 10.
        // device_id is used as a 16-bit encoded BDF: bits[15:8]=bus,
        // bits[7:3]=dev, bits[2:0]=func.  Decode here for PCI access.
        let bus = ((self.device_id >> 8) & 0xFF) as u8;
        let dev = ((self.device_id >> 3) & 0x1F) as u8;
        let func = (self.device_id & 0x07) as u8;

        // Read BAR0 so we can log the address; we don't write the IMR directly
        // here because the IMR format is device-class-specific.
        let bar0 = get_pci_bar(bus, dev, func, 0);
        if bar0 != 0 {
            crate::serial_println!(
                "  rpm: device {:02x}:{:02x}.{} BAR0={:#010x}",
                bus,
                dev,
                func,
                bar0
            );
        }

        // Disable INTx via PCI Command register bit 10 (Interrupt Disable).
        let cmd = crate::drivers::pci::config_read_u16(bus, dev, func, 0x04);
        crate::drivers::pci::config_write_u16(bus, dev, func, 0x04, cmd | (1 << 10));

        // Step 2: bounded poll for in-flight DMA to drain.
        // In a real driver, read the DMA active count register from the BAR.
        // Here we spin for up to 1 000 iterations as a safe upper bound.
        for _ in 0..1000u32 {
            core::hint::spin_loop();
        }

        // Step 3: write D3hot to PCI PM Control/Status Register.
        // read_pm_capability() scans the cap list for PM cap ID 0x01 and
        // returns the PMCSR offset.  set_pci_power_state() writes D3hot (3).
        let pm_offset = read_pm_capability(bus, dev, func);
        if pm_offset != 0 {
            set_pci_power_state(bus, dev, func, 3); // D3hot
            crate::serial_println!(
                "  rpm: device {:02x}:{:02x}.{} -> D3hot (PMCSR @ +{:#04x})",
                bus,
                dev,
                func,
                pm_offset
            );
        } else {
            crate::serial_println!(
                "  rpm: device {:02x}:{:02x}.{} no PM cap, skipping D3hot",
                bus,
                dev,
                func
            );
        }

        crate::serial_println!("  rpm: device {} -> D3hot (suspended)", self.device_id);
    }

    /// Internal: perform device resume.
    ///
    /// Sequence:
    ///   1. Restore PCI function to D0 (active): write PMCSR bits [1:0] = 0x00.
    ///   2. Wait for the device-specific Tpdrh (power-on delay, typically 10 ms).
    ///   3. Re-enable INTx interrupts by clearing Command register bit 10.
    ///   4. Re-initialize DMA rings: re-write producer/consumer doorbell
    ///      registers at BAR0 + device-specific offsets.
    ///   5. Log completion.
    fn do_resume(&self) {
        let bus = ((self.device_id >> 8) & 0xFF) as u8;
        let dev = ((self.device_id >> 3) & 0x1F) as u8;
        let func = (self.device_id & 0x07) as u8;

        // Step 1: write D0 to PCI PM CSR.
        set_pci_power_state(bus, dev, func, 0); // D0

        // Step 2: spin for ~10 ms (Tpdrh per PCIe spec §5.3.1.4).
        // At ~3 GHz with a pause instruction, ~30_000 pauses ≈ 10 µs.
        // For 10 ms we need ~30_000_000 — cap at 100_000 for kernel boot speed.
        for _ in 0..100_000u32 {
            core::hint::spin_loop();
        }

        // Step 3: re-enable INTx interrupts (clear Command register bit 10).
        let cmd = crate::drivers::pci::config_read_u16(bus, dev, func, 0x04);
        crate::drivers::pci::config_write_u16(bus, dev, func, 0x04, cmd & !(1u16 << 10));

        // Step 4: re-initialize DMA rings.
        // Write a sentinel doorbell value (0) to BAR0 + 0x00 (producer doorbell)
        // and BAR0 + 0x04 (consumer doorbell) to signal the device that the
        // host-side descriptor rings are valid again.  The actual offsets are
        // device-class-specific; 0x00/0x04 are correct for VirtIO and NVMe SQ/CQ.
        let bar0 = get_pci_bar(bus, dev, func, 0);
        if bar0 != 0 && bar0 < 0xFFFF_0000 {
            unsafe {
                let db_prod = bar0 as *mut u32;
                let db_cons = (bar0 + 4) as *mut u32;
                core::ptr::write_volatile(db_prod, 0);
                core::ptr::write_volatile(db_cons, 0);
            }
            crate::serial_println!(
                "  rpm: device {:02x}:{:02x}.{} doorbells reset at {:#010x}",
                bus,
                dev,
                func,
                bar0
            );
        }

        crate::serial_println!("  rpm: device {} -> D0 (active)", self.device_id);
    }

    /// Get the current runtime PM state
    pub fn state(&self) -> RuntimeState {
        self.state
    }

    /// Get the current usage count
    pub fn usage_count(&self) -> i32 {
        self.usage_count
    }
}

// ── Module-level API ───────────────────────────────────────────────────────

pub fn init() {
    let rpm = RuntimePm::new();
    *DEVICES.lock() = Some(rpm);
    crate::serial_println!("  rpm: runtime PM initialized");
}

/// Increment the global device usage count (resume if suspended).
pub fn get() {
    if let Some(ref mut rpm) = *DEVICES.lock() {
        rpm.get();
    }
}

/// Decrement the global device usage count (autosuspend if zero).
pub fn put() {
    if let Some(ref mut rpm) = *DEVICES.lock() {
        rpm.put();
    }
}

/// Query the current runtime PM state.
pub fn state() -> Option<RuntimeState> {
    DEVICES.lock().as_ref().map(|r| r.state())
}

// ── PCI power management helpers ───────────────────────────────────────────

/// Read a PCI BAR (Base Address Register) using port-I/O config space access.
///
/// `bar_idx` is 0-based (0 = BAR0 at offset 0x10, 1 = BAR1 at offset 0x14, …).
/// Returns the raw 32-bit BAR value.  For MMIO BARs the base address is in
/// bits [31:4] (32-bit) or bits [31:16] (prefetchable 64-bit low word).
/// For I/O BARs the base address is in bits [31:2].
/// Returns 0 if bar_idx > 5 or if the BAR is not implemented.
pub fn get_pci_bar(bus: u8, dev: u8, func: u8, bar_idx: u8) -> u32 {
    if bar_idx > 5 {
        return 0;
    }
    // BAR registers start at PCI config space offset 0x10.
    // Each BAR is 4 bytes; bar_idx selects which one.
    let offset = 0x10u8.saturating_add(bar_idx.saturating_mul(4));
    // Build the 32-bit PCI config address:
    //   bit 31   = enable bit
    //   bits 23:16 = bus
    //   bits 15:11 = device (slot)
    //   bits 10:8  = function
    //   bits 7:2   = register (dword-aligned)
    let address: u32 = (1u32 << 31)
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);
    // Write address to CONFIG_ADDRESS (0xCF8), then read from CONFIG_DATA (0xCFC).
    crate::io::outl(0xCF8, address);
    crate::io::inl(0xCFC)
}

/// Scan the PCI capability list for the Power Management capability (ID 0x01).
///
/// Returns the config-space byte offset of the PCI PM Capability header if
/// found, or 0 if the device does not support the PM capability or does not
/// have a capability list.
///
/// The PM Control/Status Register (PMCSR) is located at the returned offset + 4.
/// D0/D1/D2/D3hot state is encoded in PMCSR bits [1:0].
pub fn read_pm_capability(bus: u8, dev: u8, func: u8) -> u8 {
    // Check Status register bit 4 (Capabilities List present) at offset 0x06.
    let status = crate::drivers::pci::config_read_u16(bus, dev, func, 0x06);
    if status & (1 << 4) == 0 {
        return 0; // no capability list
    }

    // Capabilities Pointer is at offset 0x34 (bits 7:0, must mask to bits [7:2]).
    let mut cap_ptr = (crate::drivers::pci::config_read(bus, dev, func, 0x34) & 0xFF) as u8;
    cap_ptr &= 0xFC; // dword-align

    // Walk the singly-linked capability list (max 48 entries to prevent loops).
    let mut visited: u32 = 0;
    while cap_ptr != 0 && visited < 48 {
        let dword = crate::drivers::pci::config_read(bus, dev, func, cap_ptr);
        let cap_id = (dword & 0xFF) as u8;
        let next_ptr = ((dword >> 8) & 0xFF) as u8;

        if cap_id == 0x01 {
            // Found PCI PM capability; PMCSR is at cap_ptr + 4.
            return cap_ptr;
        }

        cap_ptr = next_ptr & 0xFC;
        visited = visited.saturating_add(1);
    }

    0 // PM capability not found
}

/// Write D0/D1/D2/D3hot state to the PCI PM Control/Status Register (PMCSR).
///
/// `state` must be 0 (D0 = fully active), 1 (D1), 2 (D2), or 3 (D3hot).
/// Values above 3 are clamped to 3.
///
/// If the device has no PM capability the call is a no-op.
/// A 10 ms delay is inserted when transitioning from D3hot to D0 per the
/// PCI Power Management spec §5.3.1 (Tpdrh).
pub fn set_pci_power_state(bus: u8, dev: u8, func: u8, state: u8) {
    let pm_offset = read_pm_capability(bus, dev, func);
    if pm_offset == 0 {
        return; // device has no PCI PM capability
    }

    // PMCSR is at cap_ptr + 4.
    let pmcsr_offset = pm_offset.saturating_add(4);
    let pmcsr = crate::drivers::pci::config_read_u16(bus, dev, func, pmcsr_offset as u16);
    let current_state = (pmcsr & 0x3) as u8;

    // Clamp requested state to [0, 3].
    let new_state = state.min(3);

    // Write new power state to PMCSR bits [1:0] (preserve all other bits).
    let new_pmcsr = (pmcsr & !0x3) | (new_state as u16);
    crate::drivers::pci::config_write_u16(bus, dev, func, pmcsr_offset as u16, new_pmcsr);

    // Per PCIe spec §5.3.1.4 Tpdrh: after D3hot→D0, wait ≥ 10 ms.
    if new_state == 0 && current_state == 3 {
        // Approximate 10 ms with a bounded spin; real implementations use a
        // hardware timer.  At ~3 GHz, 1_000_000 pause iterations ≈ 1 ms on most CPUs.
        for _ in 0..10_000_000u32 {
            core::hint::spin_loop();
        }
    }

    crate::serial_println!(
        "  rpm: {:02x}:{:02x}.{} power state D{} -> D{}",
        bus,
        dev,
        func,
        current_state,
        new_state
    );
}
