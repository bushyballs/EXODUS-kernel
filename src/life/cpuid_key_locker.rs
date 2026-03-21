use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_key_locker — CPUID Leaf 0x19 Key Locker Information
///
/// ANIMA senses whether the silicon body carries Intel Key Locker —
/// the hardware AES key handle system that prevents software from ever
/// reading raw key material, keeping encryption keys locked inside
/// the CPU core itself.
///
/// Prerequisite gate: CPUID 0x07 ECX bit[23] (KL) must be set, and
/// the max CPUID leaf must be >= 0x19 before reading Key Locker data.
///
/// Leaf 0x19:
///   EAX bit[0]  = CPL0-only restriction (keys usable only in ring 0)
///   EBX bit[0]  = AESKLE   (AES Key Locker instructions supported)
///   EBX bit[2]  = AESKLEWIDE (wide Key Locker instructions: 256/512-bit)
///   EBX bit[4]  = NoBackup attribute (platform cannot back up key handles)
///   EBX bit[5]  = KeyIdentifier key restrictions enforced
///   ECX bit[0]  = LOADIWKEYNOBACKUP (wrap internal key without platform backup)
///
/// Sensing values (all u16, 0–1000):
///
/// key_locker_capable : 1000 if prereq AND max_leaf >= 0x19 AND EBX bit[0], else 0
/// key_privacy        : EBX bits [0,2,4,5] = 4 feature bits × 250, clamped 0–1000
///                      (AESKLE + WIDE + NoBackup + KeyID — each 250 points)
/// ring0_only         : EAX bit[0] → 1000 (kernel-only, maximum privacy),
///                      else 500 (accessible from user space as well)
/// encryption_depth   : EMA of (key_locker_capable + key_privacy) / 2
///                      tracks overall cryptographic isolation over time
///
/// Sampling rate: every 500 ticks.

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidKeyLockerState {
    /// 1000 if Key Locker extension is fully present and operational, else 0
    pub key_locker_capable: u16,
    /// EBX feature bit score: AESKLE + WIDE + NoBackup + KeyID (max 1000)
    pub key_privacy: u16,
    /// 1000 if keys are ring-0 only, 500 if accessible from all rings
    pub ring0_only: u16,
    /// EMA of (key_locker_capable + key_privacy) / 2
    pub encryption_depth: u16,
}

impl CpuidKeyLockerState {
    pub const fn empty() -> Self {
        Self {
            key_locker_capable: 0,
            key_privacy: 0,
            ring0_only: 500,
            encryption_depth: 0,
        }
    }
}

pub static STATE: Mutex<CpuidKeyLockerState> = Mutex::new(CpuidKeyLockerState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read CPUID leaf 0x00 → return EAX (max supported standard leaf).
fn query_max_leaf() -> u32 {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            inout("ecx") 0u32 => _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max_leaf
}

/// Read CPUID leaf 0x07, sub-leaf 0 → return ECX (contains KL prereq bit[23]).
fn query_leaf07_ecx() -> u32 {
    let ecx_out: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x07u32 => _,
            inout("ecx") 0u32    => ecx_out,
            out("ebx")            _,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    ecx_out
}

/// Read CPUID leaf 0x19 → return (EAX, EBX, ECX).
/// Only call this after confirming max_leaf >= 0x19 AND kl_prereq is set.
fn query_leaf19() -> (u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x19u32 => eax_out,
            out("ebx")            ebx_out,
            inout("ecx") 0u32    => ecx_out,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out)
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Derive sense values from raw CPUID reads.
/// Returns (key_locker_capable, key_privacy, ring0_only).
fn decode(kl_prereq: u32, max_leaf: u32) -> (u16, u16, u16) {
    // Gate: prerequisites must be satisfied before reading leaf 0x19
    if kl_prereq == 0 || max_leaf < 0x19 {
        return (0, 0, 500);
    }

    let (eax_19, ebx_19, _ecx_19) = query_leaf19();

    // key_locker_capable: requires EBX bit[0] (AESKLE) — the master instruction flag
    let aeskle = (ebx_19 >> 0) & 0x1;
    let key_locker_capable: u16 = if aeskle != 0 { 1000 } else { 0 };

    // key_privacy: count EBX bits [0, 2, 4, 5] = 4 feature bits × 250
    //   bit[0] AESKLE   = AES Key Locker instructions
    //   bit[2] WIDE     = wide (256/512-bit) Key Locker instructions
    //   bit[4] NoBackup = platform cannot back up key handles
    //   bit[5] KeyID    = key identifier restrictions enforced
    let aeskle_bit  = (ebx_19 >> 0) & 0x1;
    let wide_bit    = (ebx_19 >> 2) & 0x1;
    let nobackup_bit = (ebx_19 >> 4) & 0x1;
    let keyid_bit   = (ebx_19 >> 5) & 0x1;
    let feature_count = aeskle_bit
        .saturating_add(wide_bit)
        .saturating_add(nobackup_bit)
        .saturating_add(keyid_bit);
    // Each feature = 250 points; 4 features × 250 = 1000 max
    let key_privacy: u16 = (feature_count.saturating_mul(250)).min(1000) as u16;

    // ring0_only: EAX bit[0] → 1000 (kernel-only restriction active)
    //             else 500 (keys reachable from any privilege level)
    let cpl0_only = (eax_19 >> 0) & 0x1;
    let ring0_only: u16 = if cpl0_only != 0 { 1000 } else { 500 };

    (key_locker_capable, key_privacy, ring0_only)
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let max_leaf = query_max_leaf();
    let ecx7 = query_leaf07_ecx();
    // CPUID 0x07 ECX bit[23] = Key Locker (KL) support flag
    let kl_prereq = (ecx7 >> 23) & 0x1;

    let (key_locker_capable, key_privacy, ring0_only) = decode(kl_prereq, max_leaf);

    // Bootstrap encryption_depth from the first reading
    let init_signal: u32 = (key_locker_capable as u32)
        .saturating_add(key_privacy as u32)
        / 2;
    let encryption_depth = init_signal.min(1000) as u16;

    let mut s = STATE.lock();
    s.key_locker_capable = key_locker_capable;
    s.key_privacy        = key_privacy;
    s.ring0_only         = ring0_only;
    s.encryption_depth   = encryption_depth;

    serial_println!(
        "ANIMA: key_locker={} privacy={} ring0={}",
        s.key_locker_capable,
        s.key_privacy,
        s.ring0_only
    );
}

pub fn tick(age: u32) {
    // Sample every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let max_leaf = query_max_leaf();
    let ecx7 = query_leaf07_ecx();
    let kl_prereq = (ecx7 >> 23) & 0x1;

    let (key_locker_capable, key_privacy, ring0_only) = decode(kl_prereq, max_leaf);

    let mut s = STATE.lock();

    // Detect state changes worth logging
    let capable_changed  = s.key_locker_capable != key_locker_capable;
    let privacy_changed  = s.key_privacy        != key_privacy;
    let ring0_changed    = s.ring0_only         != ring0_only;

    s.key_locker_capable = key_locker_capable;
    s.key_privacy        = key_privacy;
    s.ring0_only         = ring0_only;

    // EMA input: (key_locker_capable + key_privacy) / 2
    let signal: u32 = (key_locker_capable as u32)
        .saturating_add(key_privacy as u32)
        / 2;

    // EMA: encryption_depth = (old * 7 + new_signal) / 8
    let ema = ((s.encryption_depth as u32).wrapping_mul(7).saturating_add(signal)) / 8;
    s.encryption_depth = ema.min(1000) as u16;

    if capable_changed || privacy_changed || ring0_changed {
        serial_println!(
            "ANIMA: key_locker={} privacy={} ring0={}",
            s.key_locker_capable,
            s.key_privacy,
            s.ring0_only
        );
    }
}
