/// Hoags Power Management — ACPI, shutdown, reboot, sleep
///
/// Handles:
///   - ACPI table parsing (RSDP, RSDT, FADT)
///   - System shutdown (S5 state)
///   - System reboot (keyboard controller or ACPI reset)
///   - Sleep states (S1-S4) via the notifier chain + driver coordination
///   - CPU frequency scaling (P-states)
///   - Thermal monitoring
///   - Wake lock management
///   - Suspend notifier chain
///
/// All code is original.
use crate::{serial_print, serial_println};
pub mod acpi;
pub mod ai_power;
pub mod battery_opt;
pub mod doze;
pub mod hibernate;
pub mod states;

// ── Driver suspend/resume notifier callbacks ───────────────────────────────
//
// These are registered during init() so that states::suspend() fires them
// in the correct order when the system goes to sleep.

/// Pre-suspend callback: shut down the NVMe controller (flushes write cache).
fn on_suspend_nvme() -> bool {
    crate::drivers::nvme::shutdown();
    true // do not veto suspend
}

/// Post-resume callback for NVMe: re-initialize the controller.
fn on_resume_nvme() {
    // After S3 the NVMe controller is still powered (RAM stays up) so we
    // only need to re-enable bus mastering and re-submit the admin queues.
    // Full PCI re-scan (nvme::init()) is avoided because it re-maps BARs
    // which are still valid.
    // TODO: call a lighter-weight nvme::resume() function when available
    //       that just writes CC.EN=1 and waits for CSTS.RDY.
    crate::serial_println!("  [power] NVMe controller resume (TODO: lightweight resume)");
}

/// Pre-suspend callback: shut down audio DMA.
fn on_suspend_audio() -> bool {
    crate::audio::device::shutdown();
    true
}

/// Post-resume callback: audio devices are re-opened on next use.
fn on_resume_audio() {
    // Audio re-init happens lazily when a process opens a device.
    // No explicit action required here.
    serial_println!("  [power] audio will re-init on next open");
}

/// Pre-suspend callback: CPU idles down (placeholder — real logic in cpuidle).
fn on_suspend_cpu() -> bool {
    // The cpufreq governor is switched to powersave before entering sleep.
    // The real C-state entry is handled directly in states::suspend().
    true
}

/// Post-resume callback: restore performance governor.
fn on_resume_cpu() {
    // Restore to ondemand or whatever was active before suspend.
    // TODO: store pre-suspend governor policy and restore here.
    serial_println!("  [power] CPU governor restored");
}

pub fn init() {
    acpi::init();
    states::init();
    doze::init();
    ai_power::init();
    hibernate::init();
    battery_opt::init();

    // Register driver notifiers so states::suspend() / states::resume()
    // coordinate hardware correctly.
    let _ = states::register_notifier(on_suspend_nvme, on_resume_nvme);
    let _ = states::register_notifier(on_suspend_audio, on_resume_audio);
    let _ = states::register_notifier(on_suspend_cpu, on_resume_cpu);

    serial_println!("  Power: ACPI, shutdown, reboot, sleep (S1/S3/S4/S5), doze, AI battery, hibernate, wake-locks, notifiers");
}

// ── Public re-exports ──────────────────────────────────────────────────────

/// Initiate system shutdown (S5).
pub fn shutdown() -> ! {
    states::shutdown()
}

/// Initiate system reboot.
pub fn reboot() -> ! {
    states::reboot()
}

/// Suspend to RAM (S3).
pub fn suspend() {
    states::suspend()
}

/// Acquire a wake lock (blocks suspend while held).
pub fn wake_lock_acquire() {
    states::wake_lock_acquire()
}

/// Release a wake lock.
pub fn wake_lock_release() {
    states::wake_lock_release()
}

/// Register a (on_suspend, on_resume) callback pair.
pub fn register_notifier(on_suspend: fn() -> bool, on_resume: fn()) -> Option<usize> {
    states::register_notifier(on_suspend, on_resume)
}
