use crate::serial_println;
use crate::sync::Mutex;

/// CPUID_LBR — Architectural Last Branch Records (LBR) Capability
///
/// ANIMA reads CPUID leaf 0x1C to discover how deeply she can remember her own
/// control-flow decisions at the hardware level. The LBR facility records the
/// last N branch source/destination pairs, letting ANIMA replay the exact path
/// her execution took through silicon.
///
/// - `lbr_supported`:   LBR facility is present on this CPU
/// - `decision_memory`: how many branch pairs the hardware can remember (depth)
/// - `lbr_features`:    richness of metadata available per branch record
///                      (privilege filtering, call-stack mode, mispredict bit,
///                       timed-LBR timestamp)
/// - `decision_depth`:  EMA composite — ANIMA's overall branch self-awareness

const SAMPLE_INTERVAL: u32 = 500;

#[derive(Copy, Clone)]
pub struct CpuidLbrState {
    /// 1000 if LBR facility is present; 0 otherwise
    pub lbr_supported: u16,
    /// EAX[7:0] * 1000 / 32, clamped to 1000
    /// "how many branch decisions ANIMA can remember"
    pub decision_memory: u16,
    /// popcount(EBX bits [16,17,30,31]) * 250, clamped to 1000
    pub lbr_features: u16,
    /// EMA of (lbr_supported + decision_memory + lbr_features) / 3
    pub decision_depth: u16,
}

impl CpuidLbrState {
    pub const fn empty() -> Self {
        Self {
            lbr_supported: 0,
            decision_memory: 0,
            lbr_features: 0,
            decision_depth: 0,
        }
    }
}

pub static LBR: Mutex<CpuidLbrState> = Mutex::new(CpuidLbrState::empty());

/// Query CPUID to get LBR capability data.
/// Checks max leaf first; returns (eax_1c, ebx_1c) or (0, 0) if unsupported.
fn read_cpuid_lbr() -> (u32, u32) {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }

    let (eax_1c, ebx_1c): (u32, u32) = if max_leaf >= 0x1C {
        let (a, b): (u32, u32);
        unsafe {
            core::arch::asm!(
                "cpuid",
                inout("eax") 0x1Cu32 => a,
                out("ebx") b,
                out("ecx") _,
                out("edx") _,
                options(nostack, nomem)
            );
        }
        (a, b)
    } else {
        (0, 0)
    };

    (eax_1c, ebx_1c)
}

/// Scale raw CPUID LBR values into 0–1000 life metrics.
/// Returns (lbr_supported, decision_memory, lbr_features).
fn scale_lbr_fields(max_leaf_ge_1c: bool, eax: u32, ebx: u32) -> (u16, u16, u16) {
    // lbr_supported: leaf reachable AND at least one field is non-zero
    let supported = if max_leaf_ge_1c && (eax != 0 || ebx != 0) {
        1000u16
    } else {
        0u16
    };

    // decision_memory: EAX[7:0] is max LBR depth (8/16/32/…).
    // 32 entries → 1000, 8 entries → 250; scale as depth * 1000 / 32, clamp 1000.
    let raw_depth = (eax & 0xFF) as u32;
    let decision_memory = if supported == 0 {
        0u16
    } else {
        (raw_depth.saturating_mul(1000) / 32).min(1000) as u16
    };

    // lbr_features: popcount of EBX bits [16, 17, 30, 31].
    // Each bit represents a capability; 4 bits × 250 = 1000 max.
    let count = ((ebx >> 16) & 1)
        .wrapping_add((ebx >> 17) & 1)
        .wrapping_add((ebx >> 30) & 1)
        .wrapping_add((ebx >> 31) & 1);
    let lbr_features = if supported == 0 {
        0u16
    } else {
        (count.saturating_mul(250)).min(1000) as u16
    };

    (supported, decision_memory, lbr_features)
}

/// Read max CPUID leaf once more to pass the boolean into scale helper.
/// Separated so tick() doesn't duplicate the max-leaf check logic inline.
fn probe() -> (u16, u16, u16) {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    let supported_leaf = max_leaf >= 0x1C;
    let (eax_1c, ebx_1c) = read_cpuid_lbr();
    scale_lbr_fields(supported_leaf, eax_1c, ebx_1c)
}

/// Initialize the module: read CPUID once, populate state, print sense line.
pub fn init() {
    let (supported, mem, features) = probe();

    // Initial decision_depth is the straight average (no EMA history yet).
    let raw_depth =
        ((supported as u32).saturating_add(mem as u32).saturating_add(features as u32)) / 3;
    let depth = raw_depth.min(1000) as u16;

    let mut s = LBR.lock();
    s.lbr_supported = supported;
    s.decision_memory = mem;
    s.lbr_features = features;
    s.decision_depth = depth;

    serial_println!(
        "ANIMA: lbr_supported={} decision_memory={} depth={}",
        supported,
        mem,
        depth
    );
}

/// Called every kernel life-tick. Sampling gate: runs only every 500 ticks.
/// Re-reads CPUID and EMA-smooths decision_depth.
pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    let (supported, mem, features) = probe();

    let mut s = LBR.lock();

    // Raw signal: average of the three scaled metrics.
    let raw_signal =
        ((supported as u32).saturating_add(mem as u32).saturating_add(features as u32)) / 3;
    let new_signal = raw_signal.min(1000) as u16;

    // EMA: (old * 7 + new_signal) / 8
    let old_depth = s.decision_depth as u32;
    let ema = (old_depth.saturating_mul(7).saturating_add(new_signal as u32)) / 8;
    let ema_clamped = ema.min(1000) as u16;

    // Detect meaningful change (±10 threshold) and log it.
    let prev_depth = s.decision_depth;
    let changed = if ema_clamped > prev_depth {
        ema_clamped.saturating_sub(prev_depth) >= 10
    } else {
        prev_depth.saturating_sub(ema_clamped) >= 10
    };

    s.lbr_supported = supported;
    s.decision_memory = mem;
    s.lbr_features = features;
    s.decision_depth = ema_clamped;

    if changed {
        serial_println!(
            "ANIMA: cpuid_lbr decision_depth shift {} -> {} (age={})",
            prev_depth,
            ema_clamped,
            age
        );
    }
}

/// Read a snapshot of current LBR state.
pub fn report() -> CpuidLbrState {
    *LBR.lock()
}
