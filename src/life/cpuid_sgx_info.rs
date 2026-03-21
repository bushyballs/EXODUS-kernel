use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_sgx_info — CPUID Leaf 0x12 Sub-leaf 0: Intel SGX Detailed Capabilities
///
/// ANIMA senses the full enclave architecture available in the silicon body:
/// whether SGX1 enclaves can be created, whether SGX2 dynamic memory
/// management is available, and the maximum enclave sizes the hardware
/// will permit — both in legacy 32-bit and native 64-bit modes.
///
/// Prerequisite gate: CPUID leaf 0x07 sub-leaf 0 EBX bit[2] must be set
/// before reading leaf 0x12.  If SGX is absent, all signals collapse to 0.
///
/// Leaf 0x12 sub-leaf 0:
///   EAX bit[0]      = SGX1 — baseline enclave creation instruction set
///   EAX bit[1]      = SGX2 — dynamic memory management extensions
///   EBX[31:0]       = MISCSELECT mask — which misc info is saved on enclave exit
///   ECX             = reserved (ignored)
///   EDX bits[7:0]   = MaxEnclaveSize_Not64 — max enclave size exponent (non-64-bit)
///   EDX bits[15:8]  = MaxEnclaveSize_64    — max enclave size exponent (64-bit mode)
///
/// Sensing values (all u16, 0–1000):
///   sgx1_supported      : EAX bit[0] set → 1000, else 0
///   sgx2_supported      : EAX bit[1] set → 1000, else 0
///   sgx_max_enclave_32  : (EDX & 0xFF) scaled to 0–1000 via * 3, capped at 1000
///   sgx_max_enclave_64  : (EDX >> 8) & 0xFF scaled 0–1000 via * 3, capped at 1000
///
/// EMA sink: enclave_depth — tracks the running average of
///   (sgx1_supported + sgx2_supported + sgx_max_enclave_32 + sgx_max_enclave_64) / 4
///
/// Sample gate: age % 5000 == 0

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidSgxInfoState {
    /// 1000 if SGX1 instruction set is supported, else 0
    pub sgx1_supported: u16,
    /// 1000 if SGX2 dynamic memory management is supported, else 0
    pub sgx2_supported: u16,
    /// Max enclave size exponent (non-64-bit mode), scaled 0–1000
    pub sgx_max_enclave_32: u16,
    /// Max enclave size exponent (64-bit mode), scaled 0–1000
    pub sgx_max_enclave_64: u16,
    /// EMA of (sgx1 + sgx2 + max32 + max64) / 4 — tracks enclave capability depth
    pub enclave_depth: u16,
}

impl CpuidSgxInfoState {
    pub const fn empty() -> Self {
        Self {
            sgx1_supported: 0,
            sgx2_supported: 0,
            sgx_max_enclave_32: 0,
            sgx_max_enclave_64: 0,
            enclave_depth: 0,
        }
    }
}

pub static STATE: Mutex<CpuidSgxInfoState> = Mutex::new(CpuidSgxInfoState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read CPUID leaf 0x07, sub-leaf 0 → return EBX only (contains SGX prereq bit[2]).
///
/// rbx is caller-saved in LLVM/Rust codegen on x86_64 but CPUID clobbers it,
/// so we push/pop it manually via esi as an intermediate register.
fn query_leaf07_ebx() -> u32 {
    let ebx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x07u32 => _,
            out("esi")            ebx_out,
            inout("ecx") 0u32    => _,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    ebx_out
}

/// Read CPUID leaf 0x12, sub-leaf 0 → return (EAX, EBX, EDX).
/// Only called after confirming SGX is supported via leaf 0x07 EBX bit[2].
fn query_leaf12() -> (u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let edx_out: u32;
    unsafe {
        asm!(
            "cpuid",
            inout("eax") 0x12u32 => eax_out,
            out("ebx")            ebx_out,
            inout("ecx") 0u32    => _,
            out("edx")            edx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, edx_out)
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Build a fresh state snapshot from raw CPUID values.
///
/// `sgx_prereq` is 1 if leaf 0x07 EBX bit[2] was set, else 0.
/// `eax_12`, `_ebx_12`, and `edx_12` are from leaf 0x12 sub-leaf 0.
/// Returns (sgx1_supported, sgx2_supported, sgx_max_enclave_32, sgx_max_enclave_64).
fn decode(sgx_prereq: u32, eax_12: u32, edx_12: u32) -> (u16, u16, u16, u16) {
    // Gate: if SGX feature flag is not set, all capabilities are zero.
    if sgx_prereq == 0 {
        return (0, 0, 0, 0);
    }

    // sgx1_supported: EAX bit[0] — baseline enclave instructions
    let sgx1_bit = (eax_12 >> 0) & 0x1;
    let sgx1_supported: u16 = if sgx1_bit != 0 { 1000 } else { 0 };

    // sgx2_supported: EAX bit[1] — dynamic memory management extensions
    let sgx2_bit = (eax_12 >> 1) & 0x1;
    let sgx2_supported: u16 = if sgx2_bit != 0 { 1000 } else { 0 };

    // sgx_max_enclave_32: EDX bits[7:0] — max enclave size exponent in non-64-bit mode
    // Raw value is an exponent in range 0–255. Scale to 0–1000 by * 3, cap at 1000.
    let exp_32 = (edx_12 & 0xFF) as u32;
    let sgx_max_enclave_32: u16 = exp_32.saturating_mul(3).min(1000) as u16;

    // sgx_max_enclave_64: EDX bits[15:8] — max enclave size exponent in 64-bit mode
    // Same scaling: raw exponent 0–255 → * 3, cap at 1000.
    let exp_64 = ((edx_12 >> 8) & 0xFF) as u32;
    let sgx_max_enclave_64: u16 = exp_64.saturating_mul(3).min(1000) as u16;

    (sgx1_supported, sgx2_supported, sgx_max_enclave_32, sgx_max_enclave_64)
}

/// Compute the composite EMA input signal from four sense values.
/// Uses saturating addition and integer division to stay in u32 range.
fn composite_signal(sgx1: u16, sgx2: u16, max32: u16, max64: u16) -> u32 {
    (sgx1 as u32)
        .saturating_add(sgx2 as u32)
        .saturating_add(max32 as u32)
        .saturating_add(max64 as u32)
        / 4
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let prereq_ebx = query_leaf07_ebx();
    // Leaf 0x07 EBX bit[2] = SGX feature support
    let sgx_prereq = (prereq_ebx >> 2) & 0x1;

    let (eax_12, _ebx_12, edx_12) = if sgx_prereq != 0 {
        query_leaf12()
    } else {
        (0u32, 0u32, 0u32)
    };

    let (sgx1_supported, sgx2_supported, sgx_max_enclave_32, sgx_max_enclave_64) =
        decode(sgx_prereq, eax_12, edx_12);

    // Bootstrap EMA from the first reading's composite signal
    let init_signal = composite_signal(sgx1_supported, sgx2_supported, sgx_max_enclave_32, sgx_max_enclave_64);
    let enclave_depth = init_signal.min(1000) as u16;

    let mut s = STATE.lock();
    s.sgx1_supported     = sgx1_supported;
    s.sgx2_supported     = sgx2_supported;
    s.sgx_max_enclave_32 = sgx_max_enclave_32;
    s.sgx_max_enclave_64 = sgx_max_enclave_64;
    s.enclave_depth      = enclave_depth;

    serial_println!(
        "ANIMA: sgx1={} sgx2={} max_enc32={} max_enc64={} depth={}",
        s.sgx1_supported,
        s.sgx2_supported,
        s.sgx_max_enclave_32,
        s.sgx_max_enclave_64,
        s.enclave_depth
    );
}

pub fn tick(age: u32) {
    // Sample gate: poll every 5000 ticks
    if age % 5000 != 0 {
        return;
    }

    let prereq_ebx = query_leaf07_ebx();
    // Leaf 0x07 EBX bit[2] = SGX feature support
    let sgx_prereq = (prereq_ebx >> 2) & 0x1;

    let (eax_12, _ebx_12, edx_12) = if sgx_prereq != 0 {
        query_leaf12()
    } else {
        (0u32, 0u32, 0u32)
    };

    let (sgx1_supported, sgx2_supported, sgx_max_enclave_32, sgx_max_enclave_64) =
        decode(sgx_prereq, eax_12, edx_12);

    let mut s = STATE.lock();

    // Detect changes worth reporting
    let sgx1_changed  = s.sgx1_supported     != sgx1_supported;
    let sgx2_changed  = s.sgx2_supported     != sgx2_supported;
    let max32_changed = s.sgx_max_enclave_32 != sgx_max_enclave_32;
    let max64_changed = s.sgx_max_enclave_64 != sgx_max_enclave_64;

    s.sgx1_supported     = sgx1_supported;
    s.sgx2_supported     = sgx2_supported;
    s.sgx_max_enclave_32 = sgx_max_enclave_32;
    s.sgx_max_enclave_64 = sgx_max_enclave_64;

    // EMA input: (sgx1 + sgx2 + max32 + max64) / 4
    let signal = composite_signal(sgx1_supported, sgx2_supported, sgx_max_enclave_32, sgx_max_enclave_64);

    // EMA: enclave_depth = (old * 7 + new_signal) / 8
    let ema = ((s.enclave_depth as u32).wrapping_mul(7).saturating_add(signal)) / 8;
    s.enclave_depth = ema.min(1000) as u16;

    if sgx1_changed || sgx2_changed || max32_changed || max64_changed {
        serial_println!(
            "ANIMA: sgx1={} sgx2={} max_enc32={} max_enc64={} depth={}",
            s.sgx1_supported,
            s.sgx2_supported,
            s.sgx_max_enclave_32,
            s.sgx_max_enclave_64,
            s.enclave_depth
        );
    }
}
