pub mod crash_dump;
/// Debug infrastructure for Genesis AIOS
///
/// Provides the following subsystems:
///
/// 1. `gdb_stub`    — GDB Remote Serial Protocol stub over COM2 (0x2F8).
///    Allows a host GDB session to attach, inspect/modify registers and
///    memory, set software breakpoints, and single-step the kernel.
///
/// 2. `oops`        — Kernel oops / panic handler.
///    Structured register dump, rbp-based stack unwinding, kallsyms
///    resolution, and persistent LAST_OOPS record.  All no-alloc.
///
/// 3. `watchdog`    — Hardware and software watchdog.
///    Up to 8 software watchdog channels plus Intel TCO hardware watchdog
///    support.  Timer ISR calls `watchdog_tick`; petted via `watchdog_pet`.
///
/// 4. `crash_dump`  — Crash dump to serial and reserved memory.
///    Writes a `CrashDump` record to a reserved physical region that
///    survives warm reboots, and emits a hex-encoded stream to COM1 serial
///    for host-side capture.  Checks for previous crashes on boot.
pub mod gdb_stub;
pub mod oops;
pub mod watchdog;

/// Initialize early debug infrastructure (safe before memory::init).
///
/// Only oops and watchdog — crash_dump accesses high virtual addresses
/// (CRASH_DUMP_PHYS = 0xFFFF_8000_0010_0000) so it must run AFTER
/// memory::init() sets up the kernel high-map. Call init_late() for that.
pub fn init() {
    oops::init();
    watchdog::init();
    gdb_stub::init();
    crate::serial_println!("  [debug] Early debug subsystem initialized");
}

/// Initialize late debug infrastructure (requires memory::init to have run).
pub fn init_late() {
    crash_dump::init();
    crate::serial_println!("  [debug] Late debug subsystem initialized");
}
