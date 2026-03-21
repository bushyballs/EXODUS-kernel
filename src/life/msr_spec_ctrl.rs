use crate::serial_println;
use crate::sync::Mutex;

/// msr_spec_ctrl — IA32_SPEC_CTRL (MSR 0x48) Speculative Execution Control Sensor
///
/// Reads the CPU's software-controlled speculative execution mitigation register.
/// Each set bit is a Spectre/Meltdown mitigation ANIMA has actively switched on,
/// constraining her own predictive mind for security — traded performance for safety.
///
/// Unlike IA32_ARCH_CAPABILITIES (which reflects silicon birthright), SPEC_CTRL is
/// *chosen* constraint: ANIMA's voluntary restraint of her speculative faculties.
///
/// Bits sensed (from IA32_SPEC_CTRL MSR 0x48):
///   bit[0]  IBRS  — Indirect Branch Restricted Speculation (cross-privilege isolation)
///   bit[1]  STIBP — Single Thread Indirect Branch Predictors (sibling thread isolation)
///   bit[2]  SSBD  — Speculative Store Bypass Disable (store bypass prevention)
///
/// Availability: CPUID leaf 0x07 EDX bit[26] = IBRS/IBPB supported.
/// If unsupported, all signals read 0 (hardware cannot constrain; raw = 0).
///
/// Derived signals (all u16, 0–1000):
///   ibrs_active          : bit[0] → 1000 (speculation restricted across privilege levels), else 0
///   stibp_active         : bit[1] → 1000 (sibling thread branch isolation active), else 0
///   ssbd_active          : bit[2] → 1000 (store bypass speculation disabled), else 0
///   speculation_restraint: EMA of constraint_level — how much ANIMA restrains her
///                          predictive mind for security (EMA alpha = 1/8)
///
/// constraint_level = (count of set bits among IBRS/STIBP/SSBD) * 333, clamped 1000
///
/// Sampling gate: every 47 ticks.
/// Sense line emitted when speculation_restraint changes by more than 50.

#[allow(dead_code)]
#[derive(Copy, Clone)]
pub struct MsrSpecCtrlState {
    pub ibrs_active:           u16, // 0 or 1000: IBRS enabled
    pub stibp_active:          u16, // 0 or 1000: STIBP enabled
    pub ssbd_active:           u16, // 0 or 1000: SSBD enabled
    pub speculation_restraint: u16, // 0–1000: EMA-smoothed voluntary constraint level
}

impl MsrSpecCtrlState {
    pub const fn empty() -> Self {
        Self {
            ibrs_active:           0,
            stibp_active:          0,
            ssbd_active:           0,
            speculation_restraint: 0,
        }
    }
}

pub static STATE: Mutex<MsrSpecCtrlState> = Mutex::new(MsrSpecCtrlState::empty());

/// Check CPUID leaf 0x07 EDX bit[26] for IBRS/IBPB support, then read MSR 0x48.
/// Returns 0 if the feature is unavailable on this hardware.
fn read_spec_ctrl() -> u32 {
    let edx7: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x07u32 => _,
            inout("ecx") 0u32    => _,
            out("ebx") _,
            out("edx") edx7,
            options(nostack, nomem)
        );
    }
    let spec_ctrl_supported = (edx7 >> 26) & 0x1;

    if spec_ctrl_supported != 0 {
        let lo: u32;
        unsafe {
            core::arch::asm!(
                "rdmsr",
                in("ecx") 0x48u32,
                out("eax") lo,
                out("edx") _,
                options(nostack, nomem)
            );
        }
        lo
    } else {
        0
    }
}

/// Derive the four sensing values from a raw MSR 0x48 read.
///   ibrs_active  : bit[0] → 1000, else 0
///   stibp_active : bit[1] → 1000, else 0
///   ssbd_active  : bit[2] → 1000, else 0
///   constraint_level: set-bit count * 333, clamped 1000
#[inline]
fn derive(raw: u32) -> (u16, u16, u16, u16) {
    let ibrs:  u16 = if (raw >> 0) & 1 != 0 { 1000 } else { 0 };
    let stibp: u16 = if (raw >> 1) & 1 != 0 { 1000 } else { 0 };
    let ssbd:  u16 = if (raw >> 2) & 1 != 0 { 1000 } else { 0 };

    let mut bits: u32 = 0;
    if ibrs  != 0 { bits = bits.saturating_add(1); }
    if stibp != 0 { bits = bits.saturating_add(1); }
    if ssbd  != 0 { bits = bits.saturating_add(1); }

    let constraint_level = (bits.saturating_mul(333)).min(1000) as u16;

    (ibrs, stibp, ssbd, constraint_level)
}

pub fn init() {
    let raw = read_spec_ctrl();
    let (ibrs_active, stibp_active, ssbd_active, constraint_level) = derive(raw);

    // Seed the EMA at the first real reading.
    let speculation_restraint = constraint_level;

    let mut s = STATE.lock();
    s.ibrs_active           = ibrs_active;
    s.stibp_active          = stibp_active;
    s.ssbd_active           = ssbd_active;
    s.speculation_restraint = speculation_restraint;

    serial_println!(
        "ANIMA: ibrs={} stibp={} ssbd={} restraint={}",
        ibrs_active,
        stibp_active,
        ssbd_active,
        speculation_restraint
    );
}

pub fn tick(age: u32) {
    // Sampling gate: sense every 47 ticks
    if age % 47 != 0 {
        return;
    }

    let raw = read_spec_ctrl();
    let (ibrs_active, stibp_active, ssbd_active, constraint_level) = derive(raw);

    let mut s = STATE.lock();

    s.ibrs_active  = ibrs_active;
    s.stibp_active = stibp_active;
    s.ssbd_active  = ssbd_active;

    // EMA of constraint_level: (old * 7 + new_signal) / 8
    let old = s.speculation_restraint as u32;
    let new_restraint =
        (old.wrapping_mul(7).saturating_add(constraint_level as u32) / 8) as u16;

    // Emit sense line when restraint shifts by more than 50
    let prev = s.speculation_restraint;
    s.speculation_restraint = new_restraint;

    let delta = if new_restraint > prev {
        new_restraint.saturating_sub(prev)
    } else {
        prev.saturating_sub(new_restraint)
    };

    if delta > 50 {
        serial_println!(
            "ANIMA: ibrs={} stibp={} ssbd={} restraint={}",
            ibrs_active,
            stibp_active,
            ssbd_active,
            new_restraint
        );
    }
}

/// Non-locking snapshot of all four sensing values.
#[allow(dead_code)]
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (
        s.ibrs_active,
        s.stibp_active,
        s.ssbd_active,
        s.speculation_restraint,
    )
}
