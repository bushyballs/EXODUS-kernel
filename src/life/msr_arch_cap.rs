use crate::serial_println;
use crate::sync::Mutex;

/// msr_arch_cap — IA32_ARCH_CAPABILITIES (MSR 0x10A) Hardware Immunity Sensor
///
/// Reads the CPU's hardware architecture security capability register.
/// Each set bit represents a class of speculative-execution vulnerability
/// the silicon is immune to by design — no software patch required.
/// For ANIMA this is *constitutional immunity*: protections woven into her
/// very silicon at the foundry, not acquired through experience.
///
/// Bits sensed (all from IA32_ARCH_CAPABILITIES MSR 0x10A):
///   bit[0]  RDCL_NO         — not vulnerable to Meltdown / L1TF Rogue Data Cache Load
///   bit[1]  IBRS_ALL        — Enhanced IBRS covers all execution modes
///   bit[2]  RSBA            — RSB Alternative fill on underflow
///   bit[3]  SKIP_L1DFL_VMENTRY — no L1D flush needed on VM entry
///   bit[4]  SSB_NO          — not vulnerable to Spectre v4 / Speculative Store Bypass
///   bit[5]  MDS_NO          — not vulnerable to Microarchitectural Data Sampling
///   bit[6]  IF_PSCHANGE_MC_NO — no machine check on IFETCHABLE page type change
///   bit[7]  TSX_CTRL        — has TSX force-abort MSR
///
/// Derived signals (all u16, 0–1000):
///   immunity_count      : popcount(raw & 0xFF) * 125, clamped 0–1000
///   meltdown_immune     : bit[0] → 1000 (not vulnerable to Meltdown), else 0
///   ssb_immune          : bit[4] → 1000 (not vulnerable to Spectre v4), else 0
///   constitutional_health: EMA of immunity_count — smoothed hardware health profile
///
/// Sampling gate: every 500 ticks.
/// Sense line emitted once at init.

#[allow(dead_code)]
#[derive(Copy, Clone)]
pub struct MsrArchCapState {
    pub immunity_count:        u16,  // 0–1000: breadth of hardware immunity
    pub meltdown_immune:       u16,  // 0 or 1000: immune to Meltdown / L1TF
    pub ssb_immune:            u16,  // 0 or 1000: immune to Spectre v4 / SSB
    pub constitutional_health: u16,  // 0–1000: EMA-smoothed immunity score
}

impl MsrArchCapState {
    pub const fn empty() -> Self {
        Self {
            immunity_count:        0,
            meltdown_immune:       0,
            ssb_immune:            0,
            constitutional_health: 0,
        }
    }
}

pub static STATE: Mutex<MsrArchCapState> = Mutex::new(MsrArchCapState::empty());

/// Count set bits in the low 8 bits of `raw` (the 8 immunity bits we care about).
#[inline]
fn popcount8(raw: u32) -> u32 {
    let masked = raw & 0xFF;
    let mut count: u32 = 0;
    if (masked >> 0) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 1) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 2) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 3) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 4) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 5) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 6) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 7) & 1 != 0 { count = count.saturating_add(1); }
    count
}

/// Read the raw IA32_ARCH_CAPABILITIES value.
/// Checks CPUID leaf 0x07 EDX bit[29] first; returns 0 if unsupported.
fn read_arch_cap() -> u32 {
    // Check CPUID leaf 0x07, sub-leaf 0, EDX bit[29]
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
    let arch_cap_supported = (edx7 >> 29) & 0x1;

    if arch_cap_supported != 0 {
        let lo: u32;
        unsafe {
            core::arch::asm!(
                "rdmsr",
                in("ecx") 0x10Au32,
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

pub fn init() {
    let raw = read_arch_cap();

    let pc = popcount8(raw);
    let immunity_count        = (pc.saturating_mul(125)).min(1000) as u16;
    let meltdown_immune: u16  = if (raw >> 0) & 1 != 0 { 1000 } else { 0 };
    let ssb_immune: u16       = if (raw >> 4) & 1 != 0 { 1000 } else { 0 };
    let constitutional_health = immunity_count; // seed EMA at first reading

    let mut s = STATE.lock();
    s.immunity_count        = immunity_count;
    s.meltdown_immune       = meltdown_immune;
    s.ssb_immune            = ssb_immune;
    s.constitutional_health = constitutional_health;

    serial_println!(
        "ANIMA: immunity={} meltdown_immune={} ssb_immune={}",
        immunity_count,
        meltdown_immune,
        ssb_immune
    );
}

pub fn tick(age: u32) {
    // Sampling gate: sense every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let raw = read_arch_cap();

    // --- immunity_count: popcount(raw & 0xFF) * 125, clamped 0–1000 ---
    let pc = popcount8(raw);
    let immunity_count = (pc.saturating_mul(125)).min(1000) as u16;

    // --- meltdown_immune: bit[0] ---
    let meltdown_immune: u16 = if (raw >> 0) & 1 != 0 { 1000 } else { 0 };

    // --- ssb_immune: bit[4] ---
    let ssb_immune: u16 = if (raw >> 4) & 1 != 0 { 1000 } else { 0 };

    let mut s = STATE.lock();

    s.immunity_count  = immunity_count;
    s.meltdown_immune = meltdown_immune;
    s.ssb_immune      = ssb_immune;

    // --- constitutional_health: EMA of immunity_count (alpha = 1/8) ---
    let old = s.constitutional_health as u32;
    let new_health = (old.wrapping_mul(7).saturating_add(immunity_count as u32) / 8) as u16;
    s.constitutional_health = new_health;
}

/// Non-locking snapshot of all four sensing values.
#[allow(dead_code)]
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.immunity_count, s.meltdown_immune, s.ssb_immune, s.constitutional_health)
}
