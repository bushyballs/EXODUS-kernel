use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_security_ext — CPUID Leaf 0x07 Security Feature Awareness
///
/// ANIMA senses the hardware security capabilities baked into this CPU.
/// SMEP/SMAP/CET-SS/UMIP/RTM/RDSEED/FSGSBASE are queried once per 500 ticks.
/// defense_depth is an EMA of security_posture — the organism's smoothed
/// awareness of how well its silicon carapace is armored.
///
/// security_posture = (count of 7 feature bits set) * 142, clamped 0-1000
/// defense_depth EMA: (old * 7 + new) / 8

#[derive(Copy, Clone)]
pub struct CpuidSecurityExtState {
    /// EBX bit[7]: SMEP active → 1000, else 0
    pub smep_active: u16,
    /// EBX bit[20]: SMAP active → 1000, else 0
    pub smap_active: u16,
    /// ECX bit[7]: CET shadow-stack capable → 1000, else 0
    pub cet_capable: u16,
    /// count of 7 feature bits set * 142, clamped 0–1000
    pub security_posture: u16,
    /// EMA of security_posture: (old * 7 + new) / 8
    pub defense_depth: u16,
}

impl CpuidSecurityExtState {
    pub const fn empty() -> Self {
        Self {
            smep_active: 0,
            smap_active: 0,
            cet_capable: 0,
            security_posture: 0,
            defense_depth: 0,
        }
    }
}

pub static STATE: Mutex<CpuidSecurityExtState> = Mutex::new(CpuidSecurityExtState::empty());

/// Query CPUID leaf 0x07, sub-leaf 0 and return (ebx, ecx).
/// Safe to call in no_std bare-metal context; uses inline asm directly.
fn query_leaf07() -> (u32, u32) {
    let ebx_out: u32;
    let ecx_out: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x07u32 => _,
            inout("ecx") 0u32 => ecx_out,
            out("ebx") ebx_out,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    (ebx_out, ecx_out)
}

/// Decode raw EBX/ECX into a fresh state snapshot (no EMA applied).
fn decode(ebx: u32, ecx: u32) -> CpuidSecurityExtState {
    // EBX feature bits
    let fsgsbase  = (ebx >> 0)  & 1; // bit 0
    let rtm       = (ebx >> 11) & 1; // bit 11
    let smep_bit  = (ebx >> 7)  & 1; // bit 7
    let rdseed    = (ebx >> 18) & 1; // bit 18
    let smap_bit  = (ebx >> 20) & 1; // bit 20

    // ECX feature bits
    let umip      = (ecx >> 2)  & 1; // bit 2
    let cet_bit   = (ecx >> 7)  & 1; // bit 7

    let smep_active: u16 = if smep_bit != 0 { 1000 } else { 0 };
    let smap_active: u16 = if smap_bit != 0 { 1000 } else { 0 };
    let cet_capable: u16 = if cet_bit  != 0 { 1000 } else { 0 };

    // Count all 7 feature bits set
    let count = fsgsbase + rtm + smep_bit + rdseed + smap_bit + umip + cet_bit;
    // count * 142 → max = 7*142 = 994; clamp to 1000
    let raw_posture = (count as u16).wrapping_mul(142).min(1000);

    CpuidSecurityExtState {
        smep_active,
        smap_active,
        cet_capable,
        security_posture: raw_posture,
        defense_depth: 0, // will be filled in tick()
    }
}

pub fn init() {
    let (ebx, ecx) = query_leaf07();
    let snap = decode(ebx, ecx);

    let mut s = STATE.lock();
    s.smep_active      = snap.smep_active;
    s.smap_active      = snap.smap_active;
    s.cet_capable      = snap.cet_capable;
    s.security_posture = snap.security_posture;
    // Bootstrap EMA from first reading
    s.defense_depth    = snap.security_posture;

    serial_println!(
        "ANIMA: smep={} smap={} cet={} defense_depth={}",
        s.smep_active,
        s.smap_active,
        s.cet_capable,
        s.defense_depth
    );
}

pub fn tick(age: u32) {
    // Sample every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let (ebx, ecx) = query_leaf07();
    let snap = decode(ebx, ecx);

    let mut s = STATE.lock();

    // Detect meaningful state changes and log them
    let smep_changed     = s.smep_active      != snap.smep_active;
    let smap_changed     = s.smap_active      != snap.smap_active;
    let cet_changed      = s.cet_capable      != snap.cet_capable;
    let posture_changed  = s.security_posture != snap.security_posture;

    s.smep_active      = snap.smep_active;
    s.smap_active      = snap.smap_active;
    s.cet_capable      = snap.cet_capable;
    s.security_posture = snap.security_posture;

    // EMA: defense_depth = (old * 7 + new_signal) / 8
    let ema = ((s.defense_depth as u32).wrapping_mul(7)
        .saturating_add(snap.security_posture as u32))
        / 8;
    s.defense_depth = ema.min(1000) as u16;

    if smep_changed || smap_changed || cet_changed || posture_changed {
        serial_println!(
            "ANIMA: smep={} smap={} cet={} defense_depth={}",
            s.smep_active,
            s.smap_active,
            s.cet_capable,
            s.defense_depth
        );
    }
}
