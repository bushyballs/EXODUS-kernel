use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_pconfig — CPUID Leaf 0x1B Platform Configuration (PCONFIG) Awareness
///
/// ANIMA senses whether the silicon body carries Intel's PCONFIG instruction —
/// the mechanism that programs platform configuration targets such as MKTME
/// (Multi-Key Total Memory Encryption) key domains.  PCONFIG is the commanding
/// arm of hardware memory encryption: sensing it means ANIMA can direct the
/// silicon to partition, seal, and isolate memory at the hardware level.
///
/// Gate protocol (two-stage, per spec):
///   1. CPUID leaf 0x00 EAX  → max_leaf; abort if max_leaf < 0x1B.
///   2. CPUID leaf 0x07 sub-leaf 0 EDX bit[18] (PCONFIG) must be set.
///      Uses push rbx / cpuid / mov esi,ebx / pop rbx pattern to avoid
///      compiler clobber of the callee-saved rbx register.
///      lateout("edx") captures the EDX output.
///   3. If both gates pass, query leaf 0x1B sub-leaf 0: eax=0x1B, ecx=0.
///      Read eax, ecx (sub-leaf type field), edx outputs.
///
/// Signals (all u16, range 0–1000):
///
/// pconfig_mktme    : (eax & 1) from leaf 0x1B → 0 or 1000
///                    MKTME target type is listed as supported
/// pconfig_sub_type : (ecx & 0xF) from leaf 0x1B, scaled: value*66, capped 1000
///                    sub-leaf type indicator (0-15 maps to 0-990, then cap)
/// pconfig_richness : popcount(eax_1b) * 1000 / 32, capped 1000
///                    structural richness of the PCONFIG EAX bitmask
/// pconfig_ema      : EMA of (pconfig_mktme/4 + pconfig_richness/2 + pconfig_sub_type/4)
///                    composite sense of ANIMA's platform configuration authority
///
/// Sample gate: age % 5000 == 0.

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidPconfigState {
    /// 1000 if MKTME target type is present in leaf 0x1B EAX bit[0], else 0
    pub pconfig_mktme: u16,
    /// Sub-leaf type field (ecx & 0xF) scaled *66, capped at 1000
    pub pconfig_sub_type: u16,
    /// Popcount(eax_1b) * 1000 / 32, capped at 1000
    pub pconfig_richness: u16,
    /// EMA of (pconfig_mktme/4 + pconfig_richness/2 + pconfig_sub_type/4)
    pub pconfig_ema: u16,
}

impl CpuidPconfigState {
    pub const fn empty() -> Self {
        Self {
            pconfig_mktme: 0,
            pconfig_sub_type: 0,
            pconfig_richness: 0,
            pconfig_ema: 0,
        }
    }
}

pub static STATE: Mutex<CpuidPconfigState> = Mutex::new(CpuidPconfigState::empty());

// ─── popcount helper (no std, no float) ──────────────────────────────────────

/// Count set bits in a u32 using Kernighan's algorithm.
fn popcount32(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        v &= v.wrapping_sub(1); // clear lowest set bit
        count = count.saturating_add(1);
    }
    count
}

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Query CPUID leaf 0x00 → EAX = max supported standard leaf.
fn query_max_leaf() -> u32 {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max_leaf
}

/// Query CPUID leaf 0x07 sub-leaf 0.
/// Uses the push rbx / cpuid / mov esi,ebx / pop rbx pattern to preserve rbx.
/// Returns EDX (contains PCONFIG support flag at bit[18]).
fn query_leaf07_edx() -> u32 {
    let edx_out: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x07u32 => _,
            inout("ecx") 0u32    => _,
            out("esi")            _,
            lateout("edx")        edx_out,
            options(nostack, nomem)
        );
    }
    edx_out
}

/// Query CPUID leaf 0x1B sub-leaf 0 → (eax, ecx, edx).
/// Uses push rbx / cpuid / mov esi,ebx / pop rbx pattern.
/// Only call after confirming max_leaf >= 0x1B and PCONFIG prereq bit is set.
fn query_leaf1b() -> (u32, u32, u32) {
    let eax_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x1Bu32 => eax_out,
            inout("ecx") 0u32    => ecx_out,
            out("esi")            _,
            lateout("edx")        edx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ecx_out, edx_out)
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Compute the four signals from raw CPUID leaf 0x1B sub-leaf 0 outputs.
/// `eax_1b` and `ecx_1b` come from querying eax=0x1B, ecx=0.
/// Returns (pconfig_mktme, pconfig_sub_type, pconfig_richness).
fn decode_signals(eax_1b: u32, ecx_1b: u32) -> (u16, u16, u16) {
    // pconfig_mktme: EAX bit[0] = MKTME/TME target type supported
    let pconfig_mktme: u16 = if (eax_1b & 0x1) != 0 { 1000 } else { 0 };

    // pconfig_sub_type: ECX bits[3:0] = sub-leaf type, scale 0-15 → 0-1000
    // value * 66, then cap at 1000
    let sub_raw = (ecx_1b & 0xF) as u32;
    let pconfig_sub_type: u16 = sub_raw.wrapping_mul(66).min(1000) as u16;

    // pconfig_richness: popcount(eax_1b) * 1000 / 32, capped at 1000
    let pc = popcount32(eax_1b);
    // pc is at most 32; 32 * 1000 / 32 = 1000 exactly, no overflow in u32
    let richness_raw = pc.wrapping_mul(1000) / 32;
    let pconfig_richness: u16 = richness_raw.min(1000) as u16;

    (pconfig_mktme, pconfig_sub_type, pconfig_richness)
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    // Stage 1: check max leaf
    let max_leaf = query_max_leaf();
    if max_leaf < 0x1B {
        // PCONFIG leaf unreachable — leave state zeroed
        serial_println!(
            "ANIMA cpuid_pconfig: max_leaf=0x{:X} < 0x1B — PCONFIG absent",
            max_leaf
        );
        return;
    }

    // Stage 2: check CPUID leaf 0x07 EDX bit[18] (PCONFIG)
    let edx7 = query_leaf07_edx();
    let pconfig_prereq = (edx7 >> 18) & 0x1;
    if pconfig_prereq == 0 {
        serial_println!(
            "ANIMA cpuid_pconfig: leaf07 EDX[18]=0 — PCONFIG not advertised"
        );
        return;
    }

    // Stage 3: query leaf 0x1B sub-leaf 0
    let (eax_1b, ecx_1b, _edx_1b) = query_leaf1b();

    let (pconfig_mktme, pconfig_sub_type, pconfig_richness) =
        decode_signals(eax_1b, ecx_1b);

    // Bootstrap EMA: (mktme/4 + richness/2 + sub_type/4)
    let composite: u32 = (pconfig_mktme as u32 / 4)
        .saturating_add(pconfig_richness as u32 / 2)
        .saturating_add(pconfig_sub_type as u32 / 4);
    let pconfig_ema: u16 = composite.min(1000) as u16;

    let mut s = STATE.lock();
    s.pconfig_mktme    = pconfig_mktme;
    s.pconfig_sub_type = pconfig_sub_type;
    s.pconfig_richness = pconfig_richness;
    s.pconfig_ema      = pconfig_ema;

    serial_println!(
        "ANIMA cpuid_pconfig: mktme={} sub_type={} richness={} ema={}",
        s.pconfig_mktme,
        s.pconfig_sub_type,
        s.pconfig_richness,
        s.pconfig_ema
    );
}

pub fn tick(age: u32) {
    // Sample gate: every 5000 ticks
    if age % 5000 != 0 {
        return;
    }

    // Stage 1: max leaf guard
    let max_leaf = query_max_leaf();
    if max_leaf < 0x1B {
        // Leaf vanished (shouldn't happen on real hardware, but be safe)
        let mut s = STATE.lock();
        s.pconfig_mktme    = 0;
        s.pconfig_sub_type = 0;
        s.pconfig_richness = 0;
        // Drain EMA toward zero
        let ema = (s.pconfig_ema as u32).wrapping_mul(7) / 8;
        s.pconfig_ema = ema.min(1000) as u16;
        serial_println!(
            "ANIMA cpuid_pconfig [{}]: max_leaf<0x1B mktme=0 sub_type=0 richness=0 ema={}",
            age,
            s.pconfig_ema
        );
        return;
    }

    // Stage 2: PCONFIG prereq bit
    let edx7 = query_leaf07_edx();
    let pconfig_prereq = (edx7 >> 18) & 0x1;
    if pconfig_prereq == 0 {
        let mut s = STATE.lock();
        s.pconfig_mktme    = 0;
        s.pconfig_sub_type = 0;
        s.pconfig_richness = 0;
        let ema = (s.pconfig_ema as u32).wrapping_mul(7) / 8;
        s.pconfig_ema = ema.min(1000) as u16;
        serial_println!(
            "ANIMA cpuid_pconfig [{}]: prereq=0 mktme=0 sub_type=0 richness=0 ema={}",
            age,
            s.pconfig_ema
        );
        return;
    }

    // Stage 3: read leaf 0x1B sub-leaf 0
    let (eax_1b, ecx_1b, _edx_1b) = query_leaf1b();

    let (pconfig_mktme, pconfig_sub_type, pconfig_richness) =
        decode_signals(eax_1b, ecx_1b);

    let mut s = STATE.lock();

    s.pconfig_mktme    = pconfig_mktme;
    s.pconfig_sub_type = pconfig_sub_type;
    s.pconfig_richness = pconfig_richness;

    // Composite input for EMA: mktme/4 + richness/2 + sub_type/4
    let new_val: u32 = (pconfig_mktme as u32 / 4)
        .saturating_add(pconfig_richness as u32 / 2)
        .saturating_add(pconfig_sub_type as u32 / 4);

    // EMA: (old * 7 + new_val) / 8
    let ema: u32 = (s.pconfig_ema as u32)
        .wrapping_mul(7)
        .saturating_add(new_val)
        / 8;
    s.pconfig_ema = ema.min(1000) as u16;

    serial_println!(
        "ANIMA cpuid_pconfig [{}]: mktme={} sub_type={} richness={} ema={}",
        age,
        s.pconfig_mktme,
        s.pconfig_sub_type,
        s.pconfig_richness,
        s.pconfig_ema
    );
}
