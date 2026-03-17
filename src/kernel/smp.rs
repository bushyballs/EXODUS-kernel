use crate::sync::Mutex;
/// kernel/smp.rs — SMP core management for Genesis kernel
///
/// Provides CPU topology discovery from the ACPI MADT (Multiple APIC
/// Description Table), per-CPU state management, and AP (Application
/// Processor) startup coordination.
///
/// Design constraints (strictly enforced — violations crash the kernel):
///   - No float casts: no `as f64` / `as f32`
///   - No heap: no Vec, Box, String, alloc::* — fixed-size static arrays only
///   - No panics: no unwrap(), expect(), panic!() — return Option/bool
///   - Saturating arithmetic for all counters
///   - Wrapping arithmetic for sequence numbers
///   - MMIO via read_volatile / write_volatile only
///   - Statics in Mutex must be Copy + have const fn empty()
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum number of CPU cores this module tracks.
pub const MAX_CPUS: usize = 16;

/// MADT entry type: Processor Local APIC.
const MADT_TYPE_LOCAL_APIC: u8 = 0;
/// MADT entry type: I/O APIC.
const MADT_TYPE_IO_APIC: u8 = 1;
/// MADT entry type: Interrupt Source Override.
const MADT_TYPE_INT_SOURCE_OVERRIDE: u8 = 2;

/// MADT Local APIC flag: processor is enabled (bit 0).
const LAPIC_FLAG_ENABLED: u32 = 1;

/// MADT header: byte offset of the table length field.
const MADT_OFFSET_LENGTH: usize = 4;
/// MADT header: byte offset of the Local APIC physical address field.
const MADT_OFFSET_LAPIC_ADDR: usize = 36;
/// MADT header: byte offset where variable-length entries begin.
const MADT_OFFSET_ENTRIES: usize = 44;

/// SIPI vector page number for the AP trampoline at physical address 0x8000.
/// Page number = 0x8000 >> 12 = 0x8.
const AP_TRAMPOLINE_PAGE: u8 = 0x08;

/// AP trampoline physical address (must be in the first megabyte, page-aligned).
const AP_TRAMPOLINE_ADDR: u64 = 0x8000;

/// Spin cycles used to approximate a 10 ms delay for INIT IPI timing.
const DELAY_10MS_CYCLES: u64 = 10_000_000;

/// Maximum spin iterations to wait for an AP to signal it has started.
const AP_START_TIMEOUT_SPINS: u64 = 100_000_000;

// ── Per-CPU state ─────────────────────────────────────────────────────────────

/// Per-CPU descriptor maintained by this module.
#[derive(Copy, Clone)]
pub struct CpuInfo {
    /// Hardware APIC ID reported by the MADT.
    pub apic_id: u8,
    /// Sequential logical CPU index (0 = BSP).
    pub cpu_id: u32,
    /// Whether the CPU entry is present in the MADT and enabled.
    pub online: bool,
    /// Whether the AP has completed its startup sequence.
    pub started: bool,
    /// TSC calibration offset relative to the BSP (ticks).
    pub tsc_offset: u64,
    /// Ticks this CPU has spent in the idle loop.
    pub idle_ticks: u64,
    /// Ticks this CPU has spent executing non-idle work.
    pub active_ticks: u64,
}

impl CpuInfo {
    /// Return a zero-initialised `CpuInfo`.  Required for static initialisation.
    pub const fn empty() -> Self {
        CpuInfo {
            apic_id: 0,
            cpu_id: 0,
            online: false,
            started: false,
            tsc_offset: 0,
            idle_ticks: 0,
            active_ticks: 0,
        }
    }
}

// ── Global SMP state ──────────────────────────────────────────────────────────

/// Fixed-size CPU table protected by a Mutex.
///
/// Index 0 always holds the BSP.  Entries 1..NUM_CPUS hold APs discovered
/// from the MADT.
static CPU_TABLE: Mutex<[CpuInfo; MAX_CPUS]> = Mutex::new([CpuInfo::empty(); MAX_CPUS]);

/// Number of CPUs found in the MADT (includes BSP).  Starts at 1 (BSP only).
static NUM_CPUS: AtomicU32 = AtomicU32::new(1);

/// APIC ID of the Bootstrap Processor.  Read from the LAPIC after early init.
static BSP_APIC_ID: AtomicU32 = AtomicU32::new(0);

/// Per-AP ready flags indexed by logical cpu_id.
static AP_STARTED: [AtomicBool; MAX_CPUS] = {
    const INIT: AtomicBool = AtomicBool::new(false);
    [INIT; MAX_CPUS]
};

/// I/O APIC base address discovered from the MADT (informational).
static IO_APIC_BASE: AtomicU32 = AtomicU32::new(0);

// ── MADT parsing ──────────────────────────────────────────────────────────────

/// Parse an ACPI MADT table to populate the CPU table.
///
/// `madt_addr` — physical address of the MADT table header.
///
/// The MADT header layout:
///   [0..4]   signature "APIC" (4 bytes)
///   [4..8]   table length (u32, little-endian)
///   [36..40] Local APIC physical address (u32)
///   [40..44] flags (u32)
///   [44..]   variable-length entry records
///
/// Entry record header (2 bytes):
///   [0]  type  (u8)
///   [1]  length (u8)
///
/// Type 0 — Processor Local APIC (8 bytes total):
///   [2]  ACPI processor UID (u8)
///   [3]  APIC ID (u8)
///   [4..8] flags (u32): bit 0 = enabled
///
/// Type 1 — I/O APIC (12 bytes total):
///   [2]  I/O APIC ID (u8)
///   [3]  reserved (u8)
///   [4..8]  I/O APIC address (u32)
///   [8..12] global system interrupt base (u32)
///
/// Type 2 — Interrupt Source Override (10 bytes total):
///   [2]  bus (u8)
///   [3]  source IRQ (u8)
///   [4..8]  global system interrupt (u32)
///   [8..10] flags (u16)
pub fn parse_madt(madt_addr: u64) {
    if madt_addr == 0 {
        crate::serial_println!("  [kernel::smp] parse_madt: null MADT address, skipping");
        return;
    }

    // Read table length (u32 at offset 4).
    let table_length: u32 = unsafe {
        core::ptr::read_volatile(
            (madt_addr as usize).saturating_add(MADT_OFFSET_LENGTH) as *const u32
        )
    };

    if table_length < MADT_OFFSET_ENTRIES as u32 {
        crate::serial_println!(
            "  [kernel::smp] parse_madt: table too short ({})",
            table_length
        );
        return;
    }

    // Discover the Local APIC address (informational; we use the fixed base).
    let lapic_addr: u32 = unsafe {
        core::ptr::read_volatile(
            (madt_addr as usize).saturating_add(MADT_OFFSET_LAPIC_ADDR) as *const u32
        )
    };
    crate::serial_println!("  [kernel::smp] MADT: Local APIC addr = {:#x}", lapic_addr);

    // Walk variable-length entries.
    let mut offset = MADT_OFFSET_ENTRIES;
    let end = table_length as usize;
    let mut cpu_table = CPU_TABLE.lock();
    let mut cpu_count: u32 = 0;

    // BSP occupies slot 0; we will fill it once we find the matching APIC ID.
    let bsp_apic = BSP_APIC_ID.load(Ordering::Acquire) as u8;

    while offset.saturating_add(2) <= end {
        let base = (madt_addr as usize).saturating_add(offset);

        // Read entry type and length.
        let entry_type: u8 = unsafe { core::ptr::read_volatile(base as *const u8) };
        let entry_len: u8 =
            unsafe { core::ptr::read_volatile(base.saturating_add(1) as *const u8) };

        if entry_len < 2 {
            break; // malformed entry — stop walking
        }

        match entry_type {
            MADT_TYPE_LOCAL_APIC => {
                // Minimum length for a type-0 entry is 8 bytes.
                if entry_len >= 8 {
                    let apic_id: u8 =
                        unsafe { core::ptr::read_volatile(base.saturating_add(3) as *const u8) };
                    let flags: u32 =
                        unsafe { core::ptr::read_volatile(base.saturating_add(4) as *const u32) };

                    if flags & LAPIC_FLAG_ENABLED != 0 {
                        // Determine logical cpu_id: BSP gets 0, APs get 1+.
                        let cpu_id: u32 = if apic_id == bsp_apic {
                            0
                        } else {
                            let next = cpu_count.saturating_add(1);
                            next
                        };

                        if (cpu_id as usize) < MAX_CPUS {
                            cpu_table[cpu_id as usize] = CpuInfo {
                                apic_id,
                                cpu_id,
                                online: true,
                                started: cpu_id == 0, // BSP is already started
                                tsc_offset: 0,
                                idle_ticks: 0,
                                active_ticks: 0,
                            };

                            if apic_id == bsp_apic {
                                // BSP slot already counted as 1 in NUM_CPUS.
                            } else {
                                cpu_count = cpu_count.saturating_add(1);
                            }

                            crate::serial_println!(
                                "  [kernel::smp] MADT CPU: apic_id={} cpu_id={} ({})",
                                apic_id,
                                cpu_id,
                                if apic_id == bsp_apic { "BSP" } else { "AP" }
                            );
                        }
                    }
                }
            }

            MADT_TYPE_IO_APIC => {
                if entry_len >= 12 {
                    let io_apic_id: u8 =
                        unsafe { core::ptr::read_volatile(base.saturating_add(2) as *const u8) };
                    let io_apic_addr: u32 =
                        unsafe { core::ptr::read_volatile(base.saturating_add(4) as *const u32) };
                    let gsi_base: u32 =
                        unsafe { core::ptr::read_volatile(base.saturating_add(8) as *const u32) };
                    IO_APIC_BASE.store(io_apic_addr, Ordering::Relaxed);
                    crate::serial_println!(
                        "  [kernel::smp] MADT I/O APIC: id={} addr={:#x} gsi_base={}",
                        io_apic_id,
                        io_apic_addr,
                        gsi_base
                    );
                }
            }

            MADT_TYPE_INT_SOURCE_OVERRIDE => {
                if entry_len >= 10 {
                    let bus: u8 =
                        unsafe { core::ptr::read_volatile(base.saturating_add(2) as *const u8) };
                    let src_irq: u8 =
                        unsafe { core::ptr::read_volatile(base.saturating_add(3) as *const u8) };
                    let gsi: u32 =
                        unsafe { core::ptr::read_volatile(base.saturating_add(4) as *const u32) };
                    crate::serial_println!(
                        "  [kernel::smp] MADT Int Override: bus={} irq={} gsi={}",
                        bus,
                        src_irq,
                        gsi
                    );
                }
            }

            _ => {
                // Unknown or reserved entry type — skip silently.
            }
        }

        // Advance to the next entry.
        offset = offset.saturating_add(entry_len as usize);
    }

    // Total CPUs = BSP (1) + APs found.
    let total = cpu_count.saturating_add(1);
    NUM_CPUS.store(total, Ordering::Release);
    crate::serial_println!(
        "  [kernel::smp] MADT parse complete: {} CPU(s) found",
        total
    );
}

// ── Query API ─────────────────────────────────────────────────────────────────

/// Return the number of CPUs discovered (includes BSP).
pub fn smp_get_cpu_count() -> u32 {
    NUM_CPUS.load(Ordering::Relaxed)
}

/// Return a copy of the `CpuInfo` for logical CPU `cpu_id`, or `None` if
/// `cpu_id` is out of range or the slot is not populated.
pub fn smp_get_cpu_info(cpu_id: u32) -> Option<CpuInfo> {
    if (cpu_id as usize) >= MAX_CPUS {
        return None;
    }
    let table = CPU_TABLE.lock();
    let info = table[cpu_id as usize];
    if info.online {
        Some(info)
    } else {
        None
    }
}

/// Return the logical CPU ID of the calling CPU.
///
/// Reads the LAPIC ID via the hardware APIC module and maps it back to a
/// logical cpu_id by scanning the CPU table.  Returns 0 (BSP) if no match
/// is found.
pub fn smp_this_cpu() -> u32 {
    let my_apic_id = crate::kernel::apic::lapic_id();
    let table = CPU_TABLE.lock();
    for i in 0..MAX_CPUS {
        if table[i].online && table[i].apic_id == my_apic_id {
            return table[i].cpu_id;
        }
    }
    0
}

// ── AP startup ────────────────────────────────────────────────────────────────

/// Write dummy bytes at the AP trampoline destination to mark it used.
///
/// In a complete implementation this would write real-mode x86 opcodes that:
///   1. Load a protected-mode GDT and enter 32-bit mode.
///   2. Load the 64-bit page tables and enter long mode.
///   3. Call `ap_entry(cpu_id)` in the main kernel image.
///
/// Here we log the intent and write a recognisable signature so the
/// trampoline region is visibly occupied in a memory dump.
pub fn write_ap_trampoline(dest: u64) {
    crate::serial_println!(
        "  [kernel::smp] write_ap_trampoline: stub at {:#x} (real-mode boot code placeholder)",
        dest
    );

    // Write a minimal "hlt; jmp -2" stub so a stray AP that somehow executes
    // this location does not run garbage code.
    // Opcodes: 0xF4 = HLT, 0xEB = JMP rel8, 0xFE = -2 (jump back to HLT).
    if dest != 0 {
        unsafe {
            core::ptr::write_volatile(dest as *mut u8, 0xF4_u8); // HLT
            core::ptr::write_volatile((dest as usize).saturating_add(1) as *mut u8, 0xEB_u8); // JMP rel8
            core::ptr::write_volatile((dest as usize).saturating_add(2) as *mut u8, 0xFE_u8);
            // -2
        }
    }
}

/// Spin for approximately `cycles` CPU cycles.
///
/// Used to implement the mandatory delays in the INIT-SIPI-SIPI sequence.
/// This is a best-effort busy-wait; actual delay depends on CPU frequency.
#[inline(always)]
fn spin_delay(cycles: u64) {
    let mut i: u64 = 0;
    while i < cycles {
        core::hint::spin_loop();
        i = i.wrapping_add(1);
    }
}

/// Boot a single Application Processor.
///
/// Follows the Intel SDM INIT-SIPI-SIPI sequence (§10.4.4):
///   1. Retrieve the target CPU's APIC ID from the CPU table.
///   2. Send INIT IPI to assert the RESET signal.
///   3. Delay ≥10 ms.
///   4. Send STARTUP IPI (SIPI) with the trampoline page vector.
///   5. Delay ≥200 µs (approximated with a shorter spin here).
///   6. Send a second SIPI (per spec — ensures delivery if first was missed).
///   7. Spin until the AP sets its started flag or the timeout expires.
///
/// Returns `true` if the AP came online within the timeout, `false` otherwise.
pub fn smp_start_ap(cpu_id: u32) -> bool {
    if (cpu_id as usize) >= MAX_CPUS || cpu_id == 0 {
        // cpu_id 0 is the BSP — never boot it again.
        return false;
    }

    // Retrieve the target APIC ID.
    let apic_id: u8 = {
        let table = CPU_TABLE.lock();
        let info = table[cpu_id as usize];
        if !info.online {
            return false;
        }
        info.apic_id
    };

    crate::serial_println!(
        "  [kernel::smp] Starting AP cpu_id={} (APIC ID {})",
        cpu_id,
        apic_id
    );

    // Write the trampoline code so the AP has something to execute.
    write_ap_trampoline(AP_TRAMPOLINE_ADDR);

    // Store the AP's logical cpu_id at a well-known scratch location so the
    // real-mode trampoline can pass it to ap_entry().
    unsafe {
        core::ptr::write_volatile(0x7FF0usize as *mut u64, cpu_id as u64);
    }

    // Step 1: INIT IPI (level-assert, INIT delivery mode).
    crate::kernel::apic::lapic_send_ipi(
        apic_id,
        crate::kernel::apic::IPI_INIT | crate::kernel::apic::IPI_LEVEL_ASSERT,
        0,
    );

    // Step 2: Delay ≥10 ms.
    spin_delay(DELAY_10MS_CYCLES);

    // Step 3: First STARTUP IPI.
    crate::kernel::apic::lapic_send_ipi(
        apic_id,
        crate::kernel::apic::IPI_STARTUP,
        AP_TRAMPOLINE_PAGE,
    );

    // Step 4: Short delay (~200 µs), approximated as 1/50 of the 10 ms delay.
    spin_delay(DELAY_10MS_CYCLES / 50);

    // Step 5: Second STARTUP IPI (ensures delivery).
    crate::kernel::apic::lapic_send_ipi(
        apic_id,
        crate::kernel::apic::IPI_STARTUP,
        AP_TRAMPOLINE_PAGE,
    );

    // Step 6: Wait for the AP to signal it has started.
    let mut spins: u64 = 0;
    loop {
        if AP_STARTED[cpu_id as usize].load(Ordering::Acquire) {
            // Mark the CPU table entry as started.
            let mut table = CPU_TABLE.lock();
            table[cpu_id as usize].started = true;
            drop(table);
            crate::serial_println!("  [kernel::smp] AP cpu_id={} started", cpu_id);
            return true;
        }
        core::hint::spin_loop();
        spins = spins.wrapping_add(1);
        if spins >= AP_START_TIMEOUT_SPINS {
            break;
        }
    }

    crate::serial_println!(
        "  [kernel::smp] WARNING: AP cpu_id={} (APIC {}) did not start within timeout",
        cpu_id,
        apic_id
    );
    false
}

/// Attempt to start every AP discovered in the MADT.
///
/// Iterates over cpu_ids 1..NUM_CPUS and calls `smp_start_ap` for each.
pub fn smp_start_all_aps() {
    let count = NUM_CPUS.load(Ordering::Acquire);
    let mut started = 0u32;
    let mut i = 1u32; // skip BSP (cpu_id 0)
    while i < count && (i as usize) < MAX_CPUS {
        if smp_start_ap(i) {
            started = started.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    crate::serial_println!(
        "  [kernel::smp] smp_start_all_aps: {}/{} APs started",
        started,
        count.saturating_sub(1)
    );
}

/// Called by the AP trampoline (via `ap_entry`) to mark this AP as started.
///
/// `apic_id` — the hardware APIC ID read from the LAPIC by the AP itself.
pub fn smp_ap_started(apic_id: u8) {
    // Find the logical cpu_id for this APIC ID and set its started flag.
    let table = CPU_TABLE.lock();
    for i in 0..MAX_CPUS {
        if table[i].online && table[i].apic_id == apic_id {
            let cpu_id = table[i].cpu_id as usize;
            drop(table);
            if cpu_id < MAX_CPUS {
                AP_STARTED[cpu_id].store(true, Ordering::Release);
            }
            return;
        }
    }
    // APIC ID not found in table; no-op.
}

/// Wait until all APs have set their started flags, or until `timeout_ms`
/// milliseconds have elapsed (approximated by spin count).
///
/// Returns the number of APs that are started when the function exits.
pub fn smp_wait_for_all(timeout_ms: u64) -> u32 {
    let count = NUM_CPUS.load(Ordering::Acquire) as usize;
    // Convert timeout_ms to a spin budget (conservative: 1 ms ~ 1_000_000 spins).
    let spin_budget = timeout_ms.saturating_mul(1_000_000);
    let mut spins: u64 = 0;

    loop {
        // Count started APs (skip BSP at index 0).
        let mut started: u32 = 0;
        let mut all_started = true;
        {
            let table = CPU_TABLE.lock();
            let mut i = 1usize; // skip BSP
            while i < count && i < MAX_CPUS {
                if table[i].online {
                    if AP_STARTED[i].load(Ordering::Acquire) {
                        started = started.saturating_add(1);
                    } else {
                        all_started = false;
                    }
                }
                i = i.saturating_add(1);
            }
        }

        if all_started || spins >= spin_budget {
            return started;
        }

        core::hint::spin_loop();
        spins = spins.wrapping_add(1);
    }
}

// ── Tick accounting ───────────────────────────────────────────────────────────

/// Increment the idle tick counter for logical CPU `cpu_id`.
pub fn smp_account_idle(cpu_id: u32) {
    if (cpu_id as usize) >= MAX_CPUS {
        return;
    }
    let mut table = CPU_TABLE.lock();
    table[cpu_id as usize].idle_ticks = table[cpu_id as usize].idle_ticks.saturating_add(1);
}

/// Increment the active tick counter for logical CPU `cpu_id`.
pub fn smp_account_active(cpu_id: u32) {
    if (cpu_id as usize) >= MAX_CPUS {
        return;
    }
    let mut table = CPU_TABLE.lock();
    table[cpu_id as usize].active_ticks = table[cpu_id as usize].active_ticks.saturating_add(1);
}

/// Set the TSC offset for logical CPU `cpu_id`.
pub fn smp_set_tsc_offset(cpu_id: u32, offset: u64) {
    if (cpu_id as usize) >= MAX_CPUS {
        return;
    }
    let mut table = CPU_TABLE.lock();
    table[cpu_id as usize].tsc_offset = offset;
}

// ── Module initialisation ─────────────────────────────────────────────────────

/// Initialise the SMP module.
///
/// Records the BSP's APIC ID, marks BSP cpu_id 0 as started, and attempts
/// to parse the MADT if ACPI data is available.
///
/// Call this after `crate::kernel::apic::init()` so the LAPIC is ready
/// to read the BSP APIC ID.
pub fn init() {
    // Record the BSP APIC ID from the LAPIC.
    let bsp_apic = crate::kernel::apic::lapic_id();
    BSP_APIC_ID.store(bsp_apic as u32, Ordering::Release);

    // Populate the BSP slot (cpu_id 0) before any MADT parsing.
    {
        let mut table = CPU_TABLE.lock();
        table[0] = CpuInfo {
            apic_id: bsp_apic,
            cpu_id: 0,
            online: true,
            started: true,
            tsc_offset: 0,
            idle_ticks: 0,
            active_ticks: 0,
        };
    }
    AP_STARTED[0].store(true, Ordering::Release);

    // Attempt MADT parse if ACPI has been initialised and exposes the MADT.
    // `crate::acpi::madt_address()` returns the physical address of the MADT,
    // or 0 if not available.
    let madt_addr = crate::acpi::madt_address();
    if madt_addr != 0 {
        parse_madt(madt_addr as u64);
    } else {
        crate::serial_println!("  [kernel::smp] No MADT available; single-CPU mode");
    }

    let cpu_count = NUM_CPUS.load(Ordering::Relaxed);
    crate::serial_println!(
        "  [kernel::smp] Initialized: BSP APIC ID={} total_cpus={}",
        bsp_apic,
        cpu_count
    );
}
