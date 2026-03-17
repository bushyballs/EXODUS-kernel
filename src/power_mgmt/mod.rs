pub mod battery_mgr;
pub mod cpufreq;
pub mod cpuidle;
pub mod energy_model;
/// Advanced power management for AIOS.
///
/// Part of the AIOS power_mgmt subsystem.
pub mod governor;
pub mod idle;
pub mod runtime_pm;
pub mod suspend;
pub mod thermal_policy;

// New hardware drivers
pub mod battery;
pub mod lid;
pub mod power_supply;

/// Initialise the core power-management subsystems.
///
/// Call order:
///   1. `cpufreq::init()` — reads MSRs, builds P-state table, writes
///      IA32_PERF_CTL to max non-turbo ratio.
///   2. `idle::init()` — detects MWAIT support, zeroes stats.
///   3. `battery::init()` — first EC poll; populates BatteryState cache.
///   4. `lid::init()` — reads lid state and backlight level from EC.
///   5. `power_supply::init()` — selects startup profile, applies to hardware.
///   6. `suspend::init()` — zeroes stats, clears notifier and wakeup-source
///      tables, sets phase to Idle.
///
/// `thermal_policy::init()`, `governor::init()`, `energy_model::init()`,
/// `battery_mgr::init()`, and `runtime_pm::init()` are expected to be called
/// by the kernel's broader init sequence.
pub fn init() {
    cpufreq::init();
    idle::init();
    battery::init();
    lid::init();
    power_supply::init();
    suspend::init();
}

// ── Reboot / power-off / halt public surface ────────────────────────────────

/// Reboot the system.  Tries PS/2 reset → CF9 PCI reset → ACPI reset → ud2.
/// Never returns.
#[inline(always)]
pub fn reboot() -> ! {
    suspend::system_reboot()
}

/// Power the system off via ACPI S5 soft-off.  Never returns.
#[inline(always)]
pub fn poweroff() -> ! {
    suspend::system_poweroff()
}

/// Halt the system: disable interrupts and spin on HLT.  Never returns.
#[inline(always)]
pub fn halt() -> ! {
    suspend::system_halt()
}
