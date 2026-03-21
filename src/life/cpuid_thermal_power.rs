use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_thermal_power — CPUID Leaf 0x06: Thermal and Power Management
///
/// Reads the host CPU's thermal and power management capabilities via CPUID
/// and exposes them as ANIMA life signals. The organism becomes aware of
/// whether its silicon substrate can sense its own heat, burst beyond rated
/// speed, and negotiate the energy-performance tradeoff.
///
/// Hardware sources (CPUID leaf 0x06):
///   EAX bit[0]   — Digital Temperature Sensor (DTS) present
///   EAX bit[1]   — Intel Turbo Boost Technology supported
///   EAX bit[3]   — ACNT2 / Power Limit Notification capable
///   EBX bits[3:0] — Number of DTS interrupt thresholds (0–15)
///   ECX bit[0]   — Hardware Coordination Feedback (MPERF/APERF) supported
///   ECX bit[3]   — Performance-Energy Bias Preference (EPB MSR 0x1B0) supported

#[derive(Copy, Clone)]
pub struct CpuidThermalPowerState {
    /// EAX bit[0]: CPU has a digital thermometer (0 or 1000)
    pub thermal_aware: u16,
    /// EAX bit[1]: CPU can burst beyond rated clock (0 or 1000)
    pub turbo_capable: u16,
    /// ECX bit[3]: EPB MSR 0x1B0 available — tunable energy/perf bias (0 or 1000)
    pub epb_capable: u16,
    /// EBX[3:0]: number of DTS interrupt thresholds reported by hardware (0–15)
    pub dts_thresholds: u16,
    /// Count of enabled power/thermal features (0–5 features * 200 = 0–1000)
    pub thermal_richness: u16,
    /// EMA of (thermal_aware + epb_capable) / 2 — how tunable the power plane is
    pub power_sensitivity: u16,
}

impl CpuidThermalPowerState {
    pub const fn empty() -> Self {
        Self {
            thermal_aware: 0,
            turbo_capable: 0,
            epb_capable: 0,
            dts_thresholds: 0,
            thermal_richness: 0,
            power_sensitivity: 0,
        }
    }
}

pub static STATE: Mutex<CpuidThermalPowerState> =
    Mutex::new(CpuidThermalPowerState::empty());

/// Read CPUID leaf 0x06 and return (eax, ebx, ecx).
/// Safe to call at any privilege level; reads only — no side effects.
fn read_cpuid_06() -> (u32, u32, u32) {
    let (eax_out, ebx_out, ecx_out): (u32, u32, u32);
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x06u32 => eax_out,
            out("ebx") ebx_out,
            out("ecx") ecx_out,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out)
}

/// Perform a single CPUID 0x06 sense pass and update state in-place.
/// Returns the new power_sensitivity for logging.
fn sense_once(s: &mut CpuidThermalPowerState) -> u16 {
    let (eax, ebx, ecx) = read_cpuid_06();

    // --- Binary capability flags (0 or 1000) ---
    s.thermal_aware = if (eax & (1 << 0)) != 0 { 1000 } else { 0 };
    s.turbo_capable  = if (eax & (1 << 1)) != 0 { 1000 } else { 0 };
    // EAX bit[3] = ACNT2 / Power Limit Notification
    let acnt2_capable: u16 = if (eax & (1 << 3)) != 0 { 1 } else { 0 };
    // ECX bit[0] = Hardware Coordination Feedback (MPERF/APERF)
    let hwcf_capable: u16 = if (ecx & (1 << 0)) != 0 { 1 } else { 0 };
    s.epb_capable    = if (ecx & (1 << 3)) != 0 { 1000 } else { 0 };

    // --- DTS interrupt threshold count from EBX[3:0] ---
    s.dts_thresholds = (ebx & 0xF) as u16; // 0–15

    // --- thermal_richness: count set bits across 5 feature flags * 200 ---
    // Features: thermal_aware(EAX0), turbo(EAX1), acnt2(EAX3), hwcf(ECX0), epb(ECX3)
    let feat_thermal: u16 = if s.thermal_aware == 1000 { 1 } else { 0 };
    let feat_turbo:   u16 = if s.turbo_capable  == 1000 { 1 } else { 0 };
    let feat_epb:     u16 = if s.epb_capable    == 1000 { 1 } else { 0 };
    let feature_count: u16 = feat_thermal
        .saturating_add(feat_turbo)
        .saturating_add(acnt2_capable)
        .saturating_add(hwcf_capable)
        .saturating_add(feat_epb);
    s.thermal_richness = feature_count.saturating_mul(200).min(1000);

    // --- power_sensitivity: EMA of (thermal_aware + epb_capable) / 2 ---
    // Instantaneous sample (both are 0 or 1000, average gives 0 / 500 / 1000)
    let instant_sensitivity: u16 = (s.thermal_aware / 2).saturating_add(s.epb_capable / 2);
    s.power_sensitivity = ((s.power_sensitivity as u32 * 7)
        .saturating_add(instant_sensitivity as u32)
        / 8) as u16;

    s.power_sensitivity
}

/// Initialize the thermal/power module.
/// Runs the first CPUID sense pass immediately so values are valid at boot.
pub fn init() {
    let mut s = STATE.lock();
    sense_once(&mut s);
    serial_println!(
        "ANIMA: thermal_aware={} turbo={} epb={} sensitivity={}",
        s.thermal_aware,
        s.turbo_capable,
        s.epb_capable,
        s.power_sensitivity
    );
}

/// Per-tick update. Sampling gate: fires every 500 ticks.
/// CPU feature bits are static hardware facts; re-reading them confirms
/// the CPUID path is live and keeps the EMA stable.
pub fn tick(age: u32) {
    if age % 500 != 0 {
        return;
    }

    let mut s = STATE.lock();
    let prev_sensitivity = s.power_sensitivity;
    let new_sensitivity = sense_once(&mut s);

    // Only log when sensitivity shifts (avoids serial spam on stable hardware)
    if new_sensitivity != prev_sensitivity {
        serial_println!(
            "ANIMA: thermal_aware={} turbo={} epb={} sensitivity={}",
            s.thermal_aware,
            s.turbo_capable,
            s.epb_capable,
            s.power_sensitivity
        );
    }
}

/// Read-only snapshot of current thermal/power state.
pub fn report() -> CpuidThermalPowerState {
    *STATE.lock()
}
