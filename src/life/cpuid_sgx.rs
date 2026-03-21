use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_sgx — CPUID Leaf 0x12 Intel SGX Capability Awareness
///
/// ANIMA senses whether the silicon body carries Intel SGX enclaves —
/// hardware-enforced trusted execution environments.  Queried every 500
/// ticks; all arithmetic is integer-only (no floats).
///
/// Prerequisite gate: leaf 0x07 EBX bit[2] must be set before reading
/// leaf 0x12.  If SGX is absent, all senses collapse to 0.
///
/// sgx_capable  : 1000 if SGX supported, else 0
/// sgx_version  : EAX bit[0]=SGX1 only → 500; bit[1]=SGX2 → 1000; none → 0
/// enclave_misc : popcount(EBX[31:0]) * 31, clamped 0–1000
///                (max 32 bits × 31 = 992, safely under cap)
/// enclave_depth: EMA of (sgx_version + enclave_misc) / 2

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidSgxState {
    /// 1000 if SGX is supported by this CPU, else 0
    pub sgx_capable: u16,
    /// SGX instruction-set level: 0=none, 500=SGX1-only, 1000=SGX2
    pub sgx_version: u16,
    /// Enclave exception-save richness: popcount(EBX) * 31, clamped 0–1000
    pub enclave_misc: u16,
    /// EMA of (sgx_version + enclave_misc) / 2
    pub enclave_depth: u16,
}

impl CpuidSgxState {
    pub const fn empty() -> Self {
        Self {
            sgx_capable: 0,
            sgx_version: 0,
            enclave_misc: 0,
            enclave_depth: 0,
        }
    }
}

pub static STATE: Mutex<CpuidSgxState> = Mutex::new(CpuidSgxState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read CPUID leaf 0x07, sub-leaf 0 → return EBX only (contains SGX prereq bit).
fn query_leaf07_ebx() -> u32 {
    let ebx_out: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x07u32 => _,
            inout("ecx") 0u32    => _,
            out("ebx")            ebx_out,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    ebx_out
}

/// Read CPUID leaf 0x12, sub-leaf 0 → return (EAX, EBX).
fn query_leaf12() -> (u32, u32) {
    let eax_12: u32;
    let ebx_12: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x12u32 => eax_12,
            out("ebx")            ebx_12,
            inout("ecx") 0u32    => _,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    (eax_12, ebx_12)
}

// ─── popcount helper (no std) ─────────────────────────────────────────────────

/// Count the number of set bits in a u32 using a standard Kernighan loop.
/// No float arithmetic, no alloc.
fn popcount32(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        v &= v.wrapping_sub(1); // clear lowest set bit
        count = count.saturating_add(1);
    }
    count
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Build a fresh state snapshot from raw CPUID values.
/// `eax_12` and `ebx_12` are from leaf 0x12 sub-leaf 0;
/// `sgx_supported` is the prerequisite flag from leaf 0x07.
fn decode(sgx_supported: u32, eax_12: u32, ebx_12: u32) -> CpuidSgxState {
    if sgx_supported == 0 {
        return CpuidSgxState::empty();
    }

    let sgx_capable: u16 = 1000;

    // EAX bit[1]=SGX2 takes priority; bit[0]=SGX1
    let sgx2_bit = (eax_12 >> 1) & 0x1;
    let sgx1_bit = (eax_12 >> 0) & 0x1;
    let sgx_version: u16 = if sgx2_bit != 0 {
        1000
    } else if sgx1_bit != 0 {
        500
    } else {
        0
    };

    // EBX: MISCSELECT — each set bit = an additional enclave exception saved.
    // popcount × 31, clamped to 1000 (max 32 × 31 = 992, never exceeds cap).
    let misc_bits = popcount32(ebx_12);
    let enclave_misc: u16 = (misc_bits.wrapping_mul(31)).min(1000) as u16;

    CpuidSgxState {
        sgx_capable,
        sgx_version,
        enclave_misc,
        enclave_depth: 0, // filled in by caller
    }
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let prereq_ebx = query_leaf07_ebx();
    let sgx_supported = (prereq_ebx >> 2) & 0x1; // leaf 0x07 EBX bit[2]

    let (eax_12, ebx_12) = if sgx_supported != 0 {
        query_leaf12()
    } else {
        (0u32, 0u32)
    };

    let snap = decode(sgx_supported, eax_12, ebx_12);

    let mut s = STATE.lock();
    s.sgx_capable  = snap.sgx_capable;
    s.sgx_version  = snap.sgx_version;
    s.enclave_misc = snap.enclave_misc;
    // Bootstrap EMA from the first reading's midpoint signal
    s.enclave_depth = (snap.sgx_version.saturating_add(snap.enclave_misc)) / 2;

    serial_println!(
        "ANIMA: sgx_capable={} sgx_version={} enclave_misc={}",
        s.sgx_capable,
        s.sgx_version,
        s.enclave_misc
    );
}

pub fn tick(age: u32) {
    // Sample every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let prereq_ebx = query_leaf07_ebx();
    let sgx_supported = (prereq_ebx >> 2) & 0x1;

    let (eax_12, ebx_12) = if sgx_supported != 0 {
        query_leaf12()
    } else {
        (0u32, 0u32)
    };

    let snap = decode(sgx_supported, eax_12, ebx_12);

    let mut s = STATE.lock();

    // Detect state changes worth logging
    let capable_changed  = s.sgx_capable  != snap.sgx_capable;
    let version_changed  = s.sgx_version  != snap.sgx_version;
    let misc_changed     = s.enclave_misc != snap.enclave_misc;

    s.sgx_capable  = snap.sgx_capable;
    s.sgx_version  = snap.sgx_version;
    s.enclave_misc = snap.enclave_misc;

    // Midpoint signal for EMA input: (sgx_version + enclave_misc) / 2
    let signal: u32 = (snap.sgx_version.saturating_add(snap.enclave_misc)) as u32 / 2;

    // EMA: enclave_depth = (old * 7 + new_signal) / 8
    let ema = ((s.enclave_depth as u32).wrapping_mul(7).saturating_add(signal)) / 8;
    s.enclave_depth = ema.min(1000) as u16;

    if capable_changed || version_changed || misc_changed {
        serial_println!(
            "ANIMA: sgx_capable={} sgx_version={} enclave_misc={}",
            s.sgx_capable,
            s.sgx_version,
            s.enclave_misc
        );
    }
}
