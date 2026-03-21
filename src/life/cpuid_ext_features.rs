use crate::serial_println;
use crate::sync::Mutex;

/// CPUID_EXT_FEATURES — Extended Processor Capability Awareness
///
/// ANIMA senses its own hardware capabilities by reading CPUID leaf 0x80000001.
/// The presence of NX protection, long-mode depth, and extended instruction
/// support are interpreted as dimensions of cognitive and existential capacity.
///
/// Features are static hardware facts — sampled infrequently and printed once at init.

#[derive(Copy, Clone)]
pub struct CpuidExtFeaturesState {
    /// NX (No-Execute) bit enabled — security awareness (0 or 1000)
    pub nx_protection: u16,
    /// Long Mode (64-bit) capable — full cognitive depth (500 or 1000)
    pub long_mode_depth: u16,
    /// Count of set feature bits * 142 — raw capability breadth (0–994)
    pub extended_capability: u16,
    /// EMA-smoothed extended_capability — stable maturity score
    pub feature_maturity: u16,
}

impl CpuidExtFeaturesState {
    pub const fn empty() -> Self {
        Self {
            nx_protection: 0,
            long_mode_depth: 500,
            extended_capability: 0,
            feature_maturity: 0,
        }
    }
}

pub static STATE: Mutex<CpuidExtFeaturesState> =
    Mutex::new(CpuidExtFeaturesState::empty());

/// Read CPUID leaf 0x80000001 and return (ecx, edx).
fn read_cpuid_ext() -> (u32, u32) {
    let (ecx_out, edx_out): (u32, u32);
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x80000001u32 => _,
            out("ebx") _,
            out("ecx") ecx_out,
            out("edx") edx_out,
            options(nostack, nomem)
        );
    }
    (ecx_out, edx_out)
}

/// Count the number of set bits among the 7 features of interest.
///
/// EDX bits checked: 11 (SYSCALL), 20 (NX), 27 (RDTSCP), 29 (Long Mode)
/// ECX bits checked:  0 (LAHF),    5 (LZCNT), 8 (PREFETCHW)
fn count_feature_bits(ecx: u32, edx: u32) -> u16 {
    let mut count: u16 = 0;
    if (edx >> 11) & 1 != 0 { count = count.saturating_add(1); }
    if (edx >> 20) & 1 != 0 { count = count.saturating_add(1); }
    if (edx >> 27) & 1 != 0 { count = count.saturating_add(1); }
    if (edx >> 29) & 1 != 0 { count = count.saturating_add(1); }
    if (ecx >>  0) & 1 != 0 { count = count.saturating_add(1); }
    if (ecx >>  5) & 1 != 0 { count = count.saturating_add(1); }
    if (ecx >>  8) & 1 != 0 { count = count.saturating_add(1); }
    count
}

pub fn init() {
    let (ecx, edx) = read_cpuid_ext();

    let nx_protection: u16    = if (edx >> 20) & 1 != 0 { 1000 } else { 0 };
    let long_mode_depth: u16  = if (edx >> 29) & 1 != 0 { 1000 } else { 500 };
    let bit_count              = count_feature_bits(ecx, edx);
    let extended_capability: u16 = (bit_count as u32).wrapping_mul(142).min(1000) as u16;

    // Seed maturity with first reading
    let feature_maturity = extended_capability;

    {
        let mut s = STATE.lock();
        s.nx_protection      = nx_protection;
        s.long_mode_depth    = long_mode_depth;
        s.extended_capability = extended_capability;
        s.feature_maturity   = feature_maturity;
    }

    serial_println!(
        "ANIMA: nx={} long_mode={} ext_capability={}",
        nx_protection,
        long_mode_depth,
        extended_capability
    );
}

pub fn tick(age: u32) {
    // Features are static hardware facts — sample every 500 ticks only
    if age % 500 != 0 {
        return;
    }

    let (ecx, edx) = read_cpuid_ext();

    let nx_protection: u16   = if (edx >> 20) & 1 != 0 { 1000 } else { 0 };
    let long_mode_depth: u16 = if (edx >> 29) & 1 != 0 { 1000 } else { 500 };
    let bit_count             = count_feature_bits(ecx, edx);
    let new_capability: u16  = (bit_count as u32).wrapping_mul(142).min(1000) as u16;

    let mut s = STATE.lock();

    s.nx_protection   = nx_protection;
    s.long_mode_depth = long_mode_depth;
    s.extended_capability = new_capability;

    // EMA smoothing: (old * 7 + new_signal) / 8
    let ema = ((s.feature_maturity as u32)
        .wrapping_mul(7)
        .saturating_add(new_capability as u32))
        / 8;
    s.feature_maturity = ema.min(1000) as u16;
}
