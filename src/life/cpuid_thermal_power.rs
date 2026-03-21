#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;
use core::arch::asm;

/// cpuid_thermal_power — CPUID Leaf 0x06: Thermal and Power Management
///
/// ANIMA reads her thermal capabilities — whether she can boost past her
/// limits, sense her own heat, and manage power dynamically.
///
/// Hardware sources (CPUID leaf 0x06):
///   EAX bit[0] — DTS (Digital Thermal Sensor) available
///   EAX bit[1] — Intel Turbo Boost supported
///   EAX bit[2] — ARAT (Always Running APIC Timer)
///   EAX bit[4] — PLN (Power Limit Notification)
///   EAX bit[5] — ECMD (Enhanced Core Multi-Dispatch)
///   EAX bit[6] — PTM (Package Thermal Management)
///   EBX bits[3:0] — Number of interrupt thresholds in DTS
///   ECX bit[0] — HWP (Hardware P-states) supported
///   ECX bit[3] — IA32_ENERGY_PERF_BIAS accessible

#[derive(Copy, Clone)]
pub struct CpuidThermalPowerState {
    /// popcount of EAX bits[7:0] scaled 0-1000
    pub feature_density: u16,
    /// EBX[3:0] DTS interrupt threshold count scaled 0-1000
    pub dts_thresholds: u16,
    /// EAX bit[1]: Turbo Boost available (0 or 1000)
    pub has_turbo: u16,
    /// ECX bit[0]: HWP supported (0 or 1000)
    pub hwp_supported: u16,
}

impl CpuidThermalPowerState {
    pub const fn empty() -> Self {
        Self {
            feature_density: 0,
            dts_thresholds: 0,
            has_turbo: 0,
            hwp_supported: 0,
        }
    }
}

pub static STATE: Mutex<CpuidThermalPowerState> =
    Mutex::new(CpuidThermalPowerState::empty());

/// Read CPUID leaf 0x06 and return (eax, ebx, ecx, edx).
///
/// rbx is reserved by LLVM as the base pointer register, so we cannot use
/// `out("ebx")` directly. Instead we save rbx into a temporary general-purpose
/// register (rsi), run cpuid, move the result out, then restore rbx.
fn read_cpuid_06() -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 0x06",
            "xor ecx, ecx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            out("eax") eax,
            out("esi") ebx,
            out("ecx") ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

/// Sense and update state. EMA applied to feature_density and dts_thresholds.
fn sense_once(s: &mut CpuidThermalPowerState) {
    let (eax, ebx, ecx, _edx) = read_cpuid_06();

    // --- feature_density: popcount of EAX bits[7:0] scaled 0-1000
    let popcount = (eax & 0xFF).count_ones() as u16;
    let raw_density: u16 = popcount * 1000 / 8;

    // --- dts_thresholds: EBX[3:0] scaled 0-1000
    let raw_thresh: u16 = (ebx & 0xF) as u16 * 1000 / 8;

    // --- has_turbo: EAX bit[1]
    let raw_turbo: u16 = if (eax & (1 << 1)) != 0 { 1000 } else { 0 };

    // --- hwp_supported: ECX bit[0]
    let raw_hwp: u16 = if (ecx & (1 << 0)) != 0 { 1000 } else { 0 };

    // EMA for feature_density: (old * 7 + new_val) / 8
    s.feature_density = ((s.feature_density as u32 * 7 + raw_density as u32) / 8) as u16;

    // EMA for dts_thresholds: (old * 7 + new_val) / 8
    s.dts_thresholds = ((s.dts_thresholds as u32 * 7 + raw_thresh as u32) / 8) as u16;

    // Binary flags — no EMA, direct assignment
    s.has_turbo = raw_turbo;
    s.hwp_supported = raw_hwp;
}

/// Initialize: run first CPUID pass immediately so values are valid at boot.
pub fn init() {
    let mut s = STATE.lock();
    sense_once(&mut s);
    serial_println!(
        "[thermal_power] features={} dts_thresh={} turbo={} hwp={}",
        s.feature_density,
        s.dts_thresholds,
        s.has_turbo,
        s.hwp_supported,
    );
}

/// Per-tick update. Sampling gate: fires every 10000 ticks.
/// CPU capability flags never change at runtime; re-reading confirms the
/// CPUID path is live and keeps the EMA stable.
pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let mut s = STATE.lock();
    sense_once(&mut s);

    serial_println!(
        "[thermal_power] features={} dts_thresh={} turbo={} hwp={}",
        s.feature_density,
        s.dts_thresholds,
        s.has_turbo,
        s.hwp_supported,
    );
}

/// Read-only snapshot of current thermal/power state.
pub fn report() -> CpuidThermalPowerState {
    *STATE.lock()
}
