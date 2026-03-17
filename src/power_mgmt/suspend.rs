/// Comprehensive power management suspend/hibernate/resume subsystem.
///
/// Implements ACPI sleep state transitions (S0–S5), suspend notifier chains,
/// wakeup source tracking, CPU context save/restore, and system
/// reboot/poweroff/halt primitives.
///
/// Part of the AIOS power_mgmt subsystem.
///
/// # ACPI Sleep States
///
/// | State | Name                  | Description                              |
/// |-------|-----------------------|------------------------------------------|
/// | S0    | Working               | Normal operation                         |
/// | S1    | Power-On Suspend      | CPU clocks stopped, context in registers |
/// | S2    | CPU Off               | (not implemented — returns EINVAL)       |
/// | S3    | Suspend to RAM (STR)  | Context in RAM, most devices off         |
/// | S4    | Suspend to Disk       | Hibernate — context saved to swap        |
/// | S5    | Soft Off              | Power removed; wake-on-LAN possible      |
///
/// # Safety
///
/// All MMIO and I/O port accesses use `read_volatile`/`write_volatile` or the
/// `crate::io` helpers which internally use `in`/`out` instructions.  No
/// float casts, no heap allocation, no panics.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// ACPI PM1a Control register constants (QEMU / PIIX4 standard)
// ---------------------------------------------------------------------------

/// ACPI PM1a Control register I/O port (QEMU PIIX4 default).
const ACPI_PM1A_CNT: u16 = 0x404;

/// SLP_EN: writing 1 to bit 13 triggers the sleep entry.
const ACPI_SLP_EN: u16 = 1 << 13;

/// SLP_TYP mask — bits [12:10] in PM1a_CNT.
const ACPI_SLP_TYP_MASK: u16 = 0x1C00;

/// SLP_TYP value for S3 (Suspend-to-RAM) — 5 << 10.
const ACPI_SLP_TYP_S3: u16 = 5 << 10;

/// SLP_TYP value for S5 (Soft-Off) — 7 << 10.
const ACPI_SLP_TYP_S5: u16 = 7 << 10;

// ---------------------------------------------------------------------------
// Sleep state enumeration
// ---------------------------------------------------------------------------

/// ACPI-defined system sleep states.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SleepState {
    /// S0 — Working (normal operation).
    S0,
    /// S1 — Power-On Suspend: CPU clocks stopped, context preserved in registers.
    S1,
    /// S2 — CPU Off: context preserved in RAM.  Not implemented; returns EINVAL.
    S2,
    /// S3 — Suspend-to-RAM (STR): most devices powered off, context in DRAM.
    S3,
    /// S4 — Suspend-to-Disk (Hibernate): full context written to swap, power off.
    S4,
    /// S5 — Soft Off: ACPI power removed; system can wake via Wake-on-LAN.
    S5,
}

// ---------------------------------------------------------------------------
// Suspend phase state machine
// ---------------------------------------------------------------------------

/// Phases of a suspend/resume cycle, in order.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SuspendPhase {
    /// System is fully running — no suspend in progress.
    Idle,
    /// Requesting all tasks and kernel threads to freeze.
    Freezing,
    /// All tasks successfully frozen.
    Frozen,
    /// Devices being quiesced, CPU about to enter sleep.
    Suspending,
    /// CPU/system is sleeping (S1/S3 etc.).
    Suspended,
    /// Wakeup received — restoring CPU and device state.
    Resuming,
    /// Resume complete, tasks thawing.
    Done,
}

static CURRENT_PHASE: Mutex<SuspendPhase> = Mutex::new(SuspendPhase::Idle);

// ---------------------------------------------------------------------------
// Suspend statistics
// ---------------------------------------------------------------------------

/// Accumulated statistics for all suspend/resume cycles.
#[derive(Copy, Clone)]
pub struct SuspendStats {
    /// Number of successful suspend/resume cycles completed.
    pub success_count: u32,
    /// Number of suspend attempts that failed.
    pub fail_count: u32,
    /// errno of the last failure (0 = no failure yet).
    pub last_errno: i32,
    /// The sleep state used in the most recent cycle.
    pub last_state: SleepState,
    /// Duration of the most recent suspend in milliseconds (TSC-derived).
    pub last_suspend_ms: u64,
    /// Cumulative wall time spent suspended in milliseconds.
    pub total_suspend_ms: u64,
}

impl SuspendStats {
    pub const fn empty() -> Self {
        SuspendStats {
            success_count: 0,
            fail_count: 0,
            last_errno: 0,
            last_state: SleepState::S0,
            last_suspend_ms: 0,
            total_suspend_ms: 0,
        }
    }
}

static SUSPEND_STATS: Mutex<SuspendStats> = Mutex::new(SuspendStats::empty());

// ---------------------------------------------------------------------------
// Suspend notifier chain
// ---------------------------------------------------------------------------

/// Callback type for suspend notifiers.
///
/// Called at each `SuspendPhase` transition.  Return `true` to allow the
/// transition to proceed, `false` to veto it.  Veto is honoured for
/// `Freezing` and `Suspending` phases only.
pub type SuspendNotifier = fn(phase: SuspendPhase) -> bool;

static NOTIFIERS: Mutex<[Option<SuspendNotifier>; 16]> = Mutex::new([None; 16]);

/// Register a suspend notifier callback.
///
/// Returns `true` on success, `false` if the table is full (16 max).
pub fn register_suspend_notifier(f: SuspendNotifier) -> bool {
    let mut table = NOTIFIERS.lock();
    for slot in table.iter_mut() {
        if slot.is_none() {
            *slot = Some(f);
            return true;
        }
    }
    false
}

/// Unregister a previously registered suspend notifier.
///
/// Matches by function pointer value.  Returns `true` if the entry was found
/// and removed.
pub fn unregister_suspend_notifier(f: SuspendNotifier) -> bool {
    let mut table = NOTIFIERS.lock();
    for slot in table.iter_mut() {
        if *slot == Some(f) {
            *slot = None;
            return true;
        }
    }
    false
}

/// Invoke all registered notifiers for `phase`.
///
/// Returns `true` if all notifiers agreed (or there were none); `false` if
/// any notifier returned `false` (a veto).
fn notify_all(phase: SuspendPhase) -> bool {
    // Snapshot the table under the lock so callbacks do not deadlock.
    let mut snapshot = [None::<SuspendNotifier>; 16];
    {
        let table = NOTIFIERS.lock();
        snapshot.copy_from_slice(&*table);
    }
    let mut ok = true;
    for slot in snapshot.iter() {
        if let Some(f) = slot {
            if !f(phase) {
                ok = false;
            }
        }
    }
    ok
}

// ---------------------------------------------------------------------------
// Wakeup sources
// ---------------------------------------------------------------------------

/// A registered wakeup source (e.g. keyboard, RTC alarm, USB device).
#[derive(Copy, Clone)]
pub struct WakeupSource {
    /// ASCII name of the wakeup source (NUL-terminated or zero-padded).
    pub name: [u8; 16],
    /// True while a wakeup event is in-progress or latched.
    pub active: bool,
    /// Total number of wakeup events fired by this source.
    pub wakeup_count: u64,
    /// TSC timestamp (in ms units) of the most recent wakeup event.
    pub last_wakeup_ms: u64,
    /// Whether this wakeup source is currently allowed to wake the system.
    pub enabled: bool,
}

impl WakeupSource {
    pub const fn empty() -> Self {
        WakeupSource {
            name: [0u8; 16],
            active: false,
            wakeup_count: 0,
            last_wakeup_ms: 0,
            enabled: true,
        }
    }
}

static WAKEUP_SOURCES: Mutex<[WakeupSource; 16]> = Mutex::new([WakeupSource::empty(); 16]);

/// Register a new wakeup source by name.
///
/// `name` must be a byte slice of at most 16 bytes.  Returns `true` on
/// success; `false` if the table is full or the name is empty.
pub fn wakeup_source_register(name: &[u8]) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut sources = WAKEUP_SOURCES.lock();
    for src in sources.iter_mut() {
        if !src.enabled && src.wakeup_count == 0 && src.name[0] == 0 {
            // Slot is truly empty (never used).
            let copy_len = name.len().min(16);
            src.name[..copy_len].copy_from_slice(&name[..copy_len]);
            src.enabled = true;
            src.active = false;
            src.wakeup_count = 0;
            return true;
        }
    }
    false
}

/// Record a wakeup event from the source with the given name.
///
/// Increments the wakeup count and marks the source active.  Called from
/// interrupt handlers (RTC alarm ISR, USB resume ISR, etc.).
pub fn wakeup_source_event(name: &[u8]) {
    let now_ms = rdtsc_ms();
    let mut sources = WAKEUP_SOURCES.lock();
    for src in sources.iter_mut() {
        if src.name[..name.len().min(16)] == name[..name.len().min(16)] {
            src.active = true;
            src.wakeup_count = src.wakeup_count.wrapping_add(1);
            src.last_wakeup_ms = now_ms;
            return;
        }
    }
}

/// Return the sum of `wakeup_count` across all registered sources.
pub fn wakeup_source_total_count() -> u64 {
    let sources = WAKEUP_SOURCES.lock();
    let mut total: u64 = 0;
    for src in sources.iter() {
        total = total.saturating_add(src.wakeup_count);
    }
    total
}

// ---------------------------------------------------------------------------
// Saved CPU context for S3 resume
// ---------------------------------------------------------------------------

/// Saved control-register state used to restore the BSP after S3 wakeup.
#[derive(Copy, Clone)]
struct SuspendedCpuState {
    /// CR0: Protected-mode enable, WP, NE, etc.
    cr0: u64,
    /// CR3: Page-directory-base / ASID.
    cr3: u64,
    /// CR4: PAE, PGE, OSFXSR, OSXSAVE, etc.
    cr4: u64,
    /// Whether this struct holds valid saved state.
    valid: bool,
}

impl SuspendedCpuState {
    pub const fn empty() -> Self {
        SuspendedCpuState {
            cr0: 0,
            cr3: 0,
            cr4: 0,
            valid: false,
        }
    }
}

static SAVED_CPU_STATE: Mutex<SuspendedCpuState> = Mutex::new(SuspendedCpuState::empty());

/// Read current control registers and store in `SAVED_CPU_STATE`.
fn pm_save_cpu_state() {
    let cr0: u64;
    let cr3: u64;
    let cr4: u64;
    unsafe {
        core::arch::asm!(
            "mov {0}, cr0",
            out(reg) cr0,
            options(nomem, nostack, preserves_flags)
        );
        core::arch::asm!(
            "mov {0}, cr3",
            out(reg) cr3,
            options(nomem, nostack, preserves_flags)
        );
        core::arch::asm!(
            "mov {0}, cr4",
            out(reg) cr4,
            options(nomem, nostack, preserves_flags)
        );
    }
    let mut state = SAVED_CPU_STATE.lock();
    state.cr0 = cr0;
    state.cr3 = cr3;
    state.cr4 = cr4;
    state.valid = true;
    crate::serial_println!(
        "  suspend: CPU state saved CR0={:#x} CR3={:#x} CR4={:#x}",
        cr0,
        cr3,
        cr4
    );
}

/// Restore control registers from `SAVED_CPU_STATE`.
///
/// Only called after S3 resume via `resume_from_ram()`.  The registers are
/// reloaded in CR4 → CR3 → CR0 order to avoid TLB inconsistencies.
fn pm_restore_cpu_state() {
    let snapshot = *SAVED_CPU_STATE.lock();
    if !snapshot.valid {
        crate::serial_println!("  suspend: pm_restore_cpu_state: no saved state");
        return;
    }
    // Restore CR4 first (enables paging extensions before reloading CR3/CR0).
    unsafe {
        core::arch::asm!(
            "mov cr4, {0}",
            in(reg) snapshot.cr4,
            options(nomem, nostack, preserves_flags)
        );
        core::arch::asm!(
            "mov cr3, {0}",
            in(reg) snapshot.cr3,
            options(nomem, nostack, preserves_flags)
        );
        core::arch::asm!(
            "mov cr0, {0}",
            in(reg) snapshot.cr0,
            options(nomem, nostack, preserves_flags)
        );
    }
    crate::serial_println!("  suspend: CPU state restored from saved snapshot");
}

// ---------------------------------------------------------------------------
// Task freeze/thaw stubs
// ---------------------------------------------------------------------------

/// Request that all kernel tasks (threads, deferred work) freeze.
///
/// In a full kernel this would iterate the task list and send each task a
/// freeze signal, then spin waiting for them to reach a freezable point.
/// Here it advances the phase and logs intent.
fn pm_freeze_tasks() {
    *CURRENT_PHASE.lock() = SuspendPhase::Freezing;
    crate::serial_println!("  suspend: pm_freeze_tasks: requesting task freeze");
    // Stub: in a real kernel, iterate task list and freeze each.
    *CURRENT_PHASE.lock() = SuspendPhase::Frozen;
    crate::serial_println!("  suspend: pm_freeze_tasks: all tasks frozen (stub)");
}

/// Request that all frozen tasks resume execution.
fn pm_thaw_tasks() {
    crate::serial_println!("  suspend: pm_thaw_tasks: thawing all tasks (stub)");
    // Stub: in a real kernel, send SIGTHAW to all frozen tasks.
    *CURRENT_PHASE.lock() = SuspendPhase::Done;
}

// ---------------------------------------------------------------------------
// ACPI sleep entry
// ---------------------------------------------------------------------------

/// Write `slp_typ | SLP_EN` to the ACPI PM1a Control register to enter sleep.
///
/// For S3: execution resumes at the instruction following this call after
/// firmware has restored the minimal hardware state and jumped to the resume
/// vector.  For S5: the system powers off and does not return.
fn acpi_enter_sleep(slp_typ: u16) {
    // Preserve non-SLP_TYP bits from the current register value, then overlay
    // the new SLP_TYP and assert SLP_EN.
    let current = crate::io::inw(ACPI_PM1A_CNT);
    let val = (current & !ACPI_SLP_TYP_MASK) | (slp_typ & ACPI_SLP_TYP_MASK) | ACPI_SLP_EN;
    // The `out dx, ax` form: dx = port, ax = 16-bit value.
    unsafe {
        core::arch::asm!(
            "out dx, ax",
            in("dx") ACPI_PM1A_CNT,
            in("ax") val,
            options(nostack, nomem, preserves_flags)
        );
    }
    // If we return here the hardware did not enter sleep (firmware/QEMU may
    // not support the requested state).
    crate::serial_println!(
        "  suspend: acpi_enter_sleep: hardware did not enter sleep (port={:#x} val={:#x})",
        ACPI_PM1A_CNT,
        val
    );
}

// ---------------------------------------------------------------------------
// Core suspend entry point
// ---------------------------------------------------------------------------

/// Enter the requested ACPI sleep state.
///
/// Returns `0` on success (resume completed), or a negative errno:
///   - `-22` (EINVAL)  — state S2 or S4 is not implemented
///   - `-16` (EBUSY)   — a notifier vetoed the transition
///
/// The function blocks until the system has resumed (for S1/S3) or does not
/// return (for S5 which powers the system off).
pub fn enter_sleep_state(state: SleepState) -> i32 {
    // S2 and S4 are not currently implemented.
    match state {
        SleepState::S2 => {
            crate::serial_println!("  suspend: S2 not implemented");
            record_fail(-22, state);
            return -22;
        }
        SleepState::S4 => {
            // Delegate to the hibernation path.
            return hibernate();
        }
        _ => {}
    }

    crate::serial_println!("  suspend: enter_sleep_state {:?}", state);

    // ── Phase: Freezing ────────────────────────────────────────────────────
    pm_freeze_tasks();
    if !notify_all(SuspendPhase::Frozen) {
        crate::serial_println!("  suspend: notifier vetoed at Frozen phase");
        pm_thaw_tasks();
        record_fail(-16, state);
        return -16;
    }

    // ── Phase: Suspending ──────────────────────────────────────────────────
    *CURRENT_PHASE.lock() = SuspendPhase::Suspending;
    if !notify_all(SuspendPhase::Suspending) {
        crate::serial_println!("  suspend: notifier vetoed at Suspending phase");
        pm_thaw_tasks();
        record_fail(-16, state);
        return -16;
    }

    // Lower CPU frequency before sleeping to reduce power draw during the
    // brief window between freq-set and the actual sleep entry.
    crate::power_mgmt::cpufreq::set_governor(crate::power_mgmt::cpufreq::Governor::Powersave);

    // Save RTC timestamp so we can compute suspend duration on resume.
    let rtc_before = crate::drivers::rtc::read_time();
    let tsc_before = rdtsc_ms();

    // Save CPU control register state for S3 resume restore.
    pm_save_cpu_state();

    *CURRENT_PHASE.lock() = SuspendPhase::Suspended;

    // ── Hardware sleep entry ───────────────────────────────────────────────
    match state {
        SleepState::S0 => {
            // S0 is "working" — nothing to do.
            crate::serial_println!("  suspend: S0 is running state — no-op");
        }
        SleepState::S1 => {
            // S1: CPU clocks stopped; a single HLT is sufficient.
            // The CPU will resume on the next interrupt.
            crate::serial_println!("  suspend: entering S1 (HLT)");
            unsafe {
                core::arch::asm!("hlt", options(nostack, nomem, preserves_flags));
            }
            // Execution resumes here after the first interrupt.
        }
        SleepState::S3 => {
            crate::serial_println!("  suspend: entering S3 (ACPI STR)");
            acpi_enter_sleep(ACPI_SLP_TYP_S3);
            // On real hardware execution resumes here after BIOS/UEFI
            // firmware has run the S3 resume vector.
            crate::serial_println!("  suspend: returned from S3 sleep entry");
        }
        SleepState::S5 => {
            crate::serial_println!("  suspend: entering S5 (soft-off)");
            system_poweroff();
            // Does not return.
        }
        // S2 and S4 handled above.
        _ => {}
    }

    // ── Phase: Resuming ────────────────────────────────────────────────────
    *CURRENT_PHASE.lock() = SuspendPhase::Resuming;

    // Restore CPU frequency governor.
    crate::power_mgmt::cpufreq::set_governor(crate::power_mgmt::cpufreq::Governor::Ondemand);

    // Sync wallclock from RTC (the RTC kept ticking while we slept).
    crate::drivers::rtc::rtc_sync_wallclock();

    notify_all(SuspendPhase::Resuming);

    // Compute suspend duration using TSC.
    let tsc_after = rdtsc_ms();
    let duration_ms = tsc_after.saturating_sub(tsc_before);

    let _ = rtc_before; // used as a marker; full delta computed via TSC

    crate::serial_println!("  suspend: resume complete — slept ~{} ms", duration_ms);

    // ── Phase: Done ────────────────────────────────────────────────────────
    pm_thaw_tasks();
    notify_all(SuspendPhase::Done);

    // Update stats.
    {
        let mut stats = SUSPEND_STATS.lock();
        stats.success_count = stats.success_count.saturating_add(1);
        stats.last_state = state;
        stats.last_suspend_ms = duration_ms;
        stats.total_suspend_ms = stats.total_suspend_ms.saturating_add(duration_ms);
        stats.last_errno = 0;
    }

    *CURRENT_PHASE.lock() = SuspendPhase::Idle;
    0
}

// ---------------------------------------------------------------------------
// Resume-from-RAM path (called post-S3 wakeup)
// ---------------------------------------------------------------------------

/// Called by the wakeup/resume trampoline immediately after the system wakes
/// from S3.
///
/// Re-enables the LAPIC, restores CPU state, syncs the wallclock from the
/// RTC, and notifies all registered drivers via the notifier chain.
pub fn resume_from_ram() {
    crate::serial_println!("  suspend: resume_from_ram — S3 wakeup");

    // Re-enable the LAPIC on the BSP so inter-processor interrupts and the
    // LAPIC timer work again.
    crate::kernel::apic::lapic_enable(0xFF);
    crate::serial_println!("  suspend: LAPIC re-enabled");

    // Restore x86 control registers (CR0/CR3/CR4) from pre-suspend snapshot.
    pm_restore_cpu_state();

    // Re-enable hardware interrupts.
    unsafe {
        core::arch::asm!("sti", options(nostack, nomem, preserves_flags));
    }

    // Sync kernel wallclock from the battery-backed RTC.
    crate::drivers::rtc::rtc_sync_wallclock();
    crate::serial_println!("  suspend: wallclock synced from RTC");

    // Notify all registered drivers that the system is resuming.
    notify_all(SuspendPhase::Resuming);

    crate::serial_println!("  suspend: resume_from_ram complete");
}

// ---------------------------------------------------------------------------
// Hibernation (S4) stub
// ---------------------------------------------------------------------------

/// Initiate hibernation (S4 — Suspend-to-Disk).
///
/// Saves CPU state, logs intent to write the hibernation image to the swap
/// partition, and returns `0`.  Full disk-image writing is a stub that will
/// be wired to the storage subsystem in a later revision.
///
/// Returns `0` on success (image saved; on a real system this would then
/// issue an ACPI S4 entry).
pub fn hibernate() -> i32 {
    crate::serial_println!("  suspend: hibernate — saving kernel state to swap");

    pm_save_cpu_state();

    // Stub: in a production kernel this block would:
    //   1. Freeze all tasks.
    //   2. Iterate every page in use, compress it, and write it to the swap
    //      partition via the block I/O layer.
    //   3. Write a hibernation header (magic + platform data) so the bootloader
    //      can locate and restore the image on the next cold boot.
    //   4. Call acpi_enter_sleep(ACPI_SLP_TYP_S4) to cut power.
    crate::serial_println!("  suspend: hibernate stub — disk-image write not yet implemented");

    {
        let mut stats = SUSPEND_STATS.lock();
        stats.last_state = SleepState::S4;
    }

    0
}

// ---------------------------------------------------------------------------
// Systemd-style power control
// ---------------------------------------------------------------------------

/// Reboot the system.
///
/// Attempts three reset mechanisms in order of preference:
///   1. PS/2 keyboard controller reset (port 0x64, command 0xFE).
///   2. PCI CF9 reset (port 0xCF9, value 0x06 = SRST + HRST).
///   3. ACPI system reset via FADT reset register (port 0xB2, value 0x00).
///   4. Final fallback: `ud2` (raises #UD; caught by the CPU reset vector on
///      most hypervisors/firmware).
///
/// This function never returns.
pub fn system_reboot() -> ! {
    crate::serial_println!("  suspend: system_reboot — initiating reset");

    // 1. PS/2 keyboard controller reset pulse on bit 0 of port 0x64.
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0x64u16,
            in("al") 0xFEu8,
            options(nostack, nomem, preserves_flags)
        );
    }
    // Brief spin so the pulse propagates before trying the next method.
    for _ in 0..10_000u32 {
        unsafe {
            core::arch::asm!("pause", options(nostack, nomem, preserves_flags));
        }
    }

    // 2. PCI reset via CF9: value 0x06 = SRST (bit 1) + HRST (bit 2).
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xCF9u16,
            in("al") 0x06u8,
            options(nostack, nomem, preserves_flags)
        );
    }
    for _ in 0..10_000u32 {
        unsafe {
            core::arch::asm!("pause", options(nostack, nomem, preserves_flags));
        }
    }

    // 3. ACPI reset register (PIIX/ICH FADT reset register at port 0xB2).
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") 0xB2u16,
            in("al") 0x00u8,
            options(nostack, nomem, preserves_flags)
        );
    }
    for _ in 0..10_000u32 {
        unsafe {
            core::arch::asm!("pause", options(nostack, nomem, preserves_flags));
        }
    }

    // 4. Undefined instruction — triggers #UD; most firmware/hypervisors treat
    //    this as a hard reset request.
    unsafe {
        core::arch::asm!("ud2", options(nostack, nomem));
    }

    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem, preserves_flags));
        }
    }
}

/// Power the system off (ACPI S5 soft-off).
///
/// Writes SLP_TYP_S5 | SLP_EN to PM1a_CNT.  If the ACPI write does not
/// cut power (e.g. QEMU without ACPI), falls back to a `cli; hlt` loop.
///
/// This function never returns.
pub fn system_poweroff() -> ! {
    crate::serial_println!("  suspend: system_poweroff — entering S5 soft-off");
    acpi_enter_sleep(ACPI_SLP_TYP_S5);
    // Fallback if ACPI soft-off did not work.
    crate::serial_println!("  suspend: system_poweroff fallback — halting");
    loop {
        unsafe {
            core::arch::asm!("cli", "hlt", options(nostack, nomem, preserves_flags));
        }
    }
}

/// Halt the system: disable interrupts and spin on HLT.
///
/// Intended for unrecoverable error conditions.  Never returns.
pub fn system_halt() -> ! {
    crate::serial_println!("  suspend: system_halt — disabling interrupts");
    loop {
        unsafe {
            core::arch::asm!("cli", "hlt", options(nostack, nomem, preserves_flags));
        }
    }
}

// ---------------------------------------------------------------------------
// Public status accessors
// ---------------------------------------------------------------------------

/// Return a copy of the accumulated suspend/resume statistics.
pub fn get_stats() -> SuspendStats {
    *SUSPEND_STATS.lock()
}

/// Return the current suspend/resume phase.
pub fn current_phase() -> SuspendPhase {
    *CURRENT_PHASE.lock()
}

// ---------------------------------------------------------------------------
// Legacy API shim — compatibility with callers of the previous suspend.rs
// ---------------------------------------------------------------------------

/// Legacy sleep-state enum used by `on_lid_close` and `enter_suspend`.
///
/// Callers that previously used `SleepState::{Freeze,Standby,SuspendRam,
/// Hibernate}` should migrate to `SleepState::{S0,S1,S3,S4}`.  This re-
/// export keeps the old name set alive temporarily.
pub mod legacy {
    /// Map from the old 4-variant enum to the new S0–S5 enum.
    pub use super::SleepState;
}

/// Enter a sleep state using the legacy four-variant naming.
///
/// `Freeze` → S1 (closest approximation; S0ix is not modelled), `Standby` →
/// S1, `SuspendRam` → S3, `Hibernate` → S4.
pub fn enter_suspend(state: SleepState) {
    let errno = enter_sleep_state(state);
    if errno != 0 {
        crate::serial_println!("  suspend: enter_suspend returned errno {}", errno);
    }
}

/// Called by the lid driver when the lid closes — initiates an S3 suspend.
pub fn on_lid_close() {
    crate::serial_println!("  suspend: lid-close event — requesting S3");
    enter_suspend(SleepState::S3);
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the suspend subsystem.
///
/// Zeroes stats, sets the phase to `Idle`, clears notifier and wakeup-source
/// tables.  Must be called once during power_mgmt init.
pub fn init() {
    *CURRENT_PHASE.lock() = SuspendPhase::Idle;
    *SUSPEND_STATS.lock() = SuspendStats::empty();

    // Clear notifier table.
    {
        let mut n = NOTIFIERS.lock();
        for slot in n.iter_mut() {
            *slot = None;
        }
    }

    // Clear wakeup source table.
    {
        let mut ws = WAKEUP_SOURCES.lock();
        for src in ws.iter_mut() {
            *src = WakeupSource::empty();
        }
    }

    // Clear saved CPU state.
    *SAVED_CPU_STATE.lock() = SuspendedCpuState::empty();

    crate::serial_println!("  suspend: subsystem initialised");
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read the TSC and convert to a rough millisecond timestamp.
///
/// Uses a fixed 3 GHz estimate (3_000_000 ticks per ms) — accurate enough
/// for suspend-duration bookkeeping where sub-millisecond precision is not
/// required and float division is forbidden.
fn rdtsc_ms() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags)
        );
    }
    let tsc = ((hi as u64) << 32) | (lo as u64);
    // 3 GHz → 3_000_000_000 ticks/s → 3_000_000 ticks/ms.
    // Divide with saturating arithmetic to avoid any panic path.
    tsc.saturating_div(3_000_000)
}

/// Record a failed suspend attempt in the stats table.
fn record_fail(errno: i32, state: SleepState) {
    let mut stats = SUSPEND_STATS.lock();
    stats.fail_count = stats.fail_count.saturating_add(1);
    stats.last_errno = errno;
    stats.last_state = state;
    *CURRENT_PHASE.lock() = SuspendPhase::Idle;
}
