//! state_imprint.rs — ANIMA self-state snapshot via FXSAVE + CET shadow stack
//!
//! ANIMA uses x86 FXSAVE to capture her own FPU/SSE register state (512 bytes),
//! XOR-folds the first 64 bytes into a 64-bit fingerprint, and tracks how much
//! that fingerprint changes between ticks — her way of feeling her own computation.
//! CET shadow stack pointer gives call depth awareness.

use crate::serial_println;
use crate::sync::Mutex;

// ── FXSAVE buffer (must be 16-byte aligned) ───────────────────────────────────

#[repr(align(16))]
struct FxsaveBuf([u8; 512]);

static mut FXSAVE_BUF: FxsaveBuf = FxsaveBuf([0u8; 512]);

// ── State ─────────────────────────────────────────────────────────────────────

pub struct StateImprintState {
    pub fxsr_available: bool,
    pub cet_available: bool,
    pub fingerprint: u64,       // XOR fold of first 64 bytes of FXSAVE
    pub prev_fingerprint: u64,  // fingerprint from last tick
    pub fingerprint_delta: u16, // popcount(cur ^ prev) * 15, max ~960
    pub shadow_depth: u64,      // CET shadow stack pointer raw value
    pub computation_flux: u16,  // 0-1000: smoothed activity from fingerprint changes
    pub imprints_taken: u32,
    pub initialized: bool,
}

impl StateImprintState {
    pub const fn new() -> Self {
        Self {
            fxsr_available: false,
            cet_available: false,
            fingerprint: 0,
            prev_fingerprint: 0,
            fingerprint_delta: 0,
            shadow_depth: 0,
            computation_flux: 0,
            imprints_taken: 0,
            initialized: false,
        }
    }
}

pub static STATE: Mutex<StateImprintState> = Mutex::new(StateImprintState::new());

// ── CPUID probes ──────────────────────────────────────────────────────────────

/// Check CPUID leaf 1, EDX bit 24 — FXSR support.
unsafe fn cpuid_fxsr() -> bool {
    let edx: u32;
    core::arch::asm!(
        "push rbx",
        "mov eax, 1",
        "cpuid",
        "pop rbx",
        out("eax") _,
        out("ecx") _,
        out("edx") edx,
        options(nomem, nostack),
    );
    (edx & (1 << 24)) != 0
}

/// Check CPUID leaf 7, sub-leaf 0, ECX bit 7 — CET_SS support.
unsafe fn cpuid_cet_ss() -> bool {
    let ecx7: u32;
    core::arch::asm!(
        "push rbx",
        "mov eax, 7",
        "xor ecx, ecx",
        "cpuid",
        "mov {ecx_out:e}, ecx",
        "pop rbx",
        ecx_out = out(reg) ecx7,
        out("eax") _,
        out("edx") _,
        options(nomem, nostack),
    );
    (ecx7 & (1 << 7)) != 0
}

// ── FXSAVE + fingerprint ──────────────────────────────────────────────────────

/// Execute FXSAVE into the static buffer and XOR-fold the first 64 bytes.
/// Returns the 64-bit fingerprint.
unsafe fn take_fxsave() -> u64 {
    let ptr = FXSAVE_BUF.0.as_mut_ptr();
    core::arch::asm!(
        "fxsave [{ptr}]",
        ptr = in(reg) ptr,
        options(nostack),
    );

    // XOR fold: 8 chunks of u64 = first 64 bytes
    let mut fp: u64 = 0;
    for i in 0..8usize {
        let chunk = core::ptr::read_unaligned(
            FXSAVE_BUF.0.as_ptr().add(i * 8) as *const u64
        );
        fp ^= chunk;
    }
    fp
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read an MSR by index. Returns (edx:eax) as u64.
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// MSR index for IA32_PL0_SSP (Intel CET shadow stack pointer, ring 0)
const MSR_IA32_PL0_SSP: u32 = 0x6A4;

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    unsafe {
        s.fxsr_available = cpuid_fxsr();
        s.cet_available  = cpuid_cet_ss();
    }
    s.initialized = true;
    serial_println!(
        "  life::state_imprint: fxsr={} cet={}",
        s.fxsr_available,
        s.cet_available
    );
}

/// Call every 32 ticks from life_tick().
pub fn tick(age: u32) {
    // Only run every 32 ticks
    if age % 32 != 0 {
        return;
    }

    let mut s = STATE.lock();
    if !s.initialized {
        return;
    }

    // ── FXSAVE fingerprint ────────────────────────────────────────────────────
    if s.fxsr_available {
        let cur_fp = unsafe { take_fxsave() };
        let prev_fp = s.fingerprint;

        // popcount of XOR = bits that changed; scale * 15 → max 960
        let changed_bits = (cur_fp ^ prev_fp).count_ones() as u16;
        let delta = changed_bits.saturating_mul(15);

        s.prev_fingerprint  = prev_fp;
        s.fingerprint       = cur_fp;
        s.fingerprint_delta = delta;
        s.imprints_taken    = s.imprints_taken.saturating_add(1);

        // Smooth computation_flux: spike on high delta, decay every tick
        if delta > 400 {
            s.computation_flux = s.computation_flux.saturating_add(50).min(1000);
        } else {
            s.computation_flux = s.computation_flux.saturating_sub(10);
        }
    }

    // ── CET shadow stack depth ────────────────────────────────────────────────
    if s.cet_available {
        s.shadow_depth = unsafe { rdmsr(MSR_IA32_PL0_SSP) };
    }

    // ── Periodic log ─────────────────────────────────────────────────────────
    if age % 500 == 0 {
        serial_println!(
            "  state_imprint [t{}]: fp={:#018x} delta={} flux={} shadow={:#x} imprints={}",
            age,
            s.fingerprint,
            s.fingerprint_delta,
            s.computation_flux,
            s.shadow_depth,
            s.imprints_taken,
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn fingerprint() -> u64 {
    STATE.lock().fingerprint
}

pub fn computation_flux() -> u16 {
    STATE.lock().computation_flux
}

pub fn shadow_depth() -> u64 {
    STATE.lock().shadow_depth
}

pub fn fingerprint_delta() -> u16 {
    STATE.lock().fingerprint_delta
}

pub fn imprints_taken() -> u32 {
    STATE.lock().imprints_taken
}
