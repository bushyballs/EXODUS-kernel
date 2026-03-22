#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

/// IA32_PQR_ASSOC MSR address — associates this logical processor with an RDT
/// Class of Service (COS) and Resource Monitoring ID (RMID).
const IA32_PQR_ASSOC: u32 = 0xC8F;

/// Tick gate: sample every 2000 ticks.
const TICK_GATE: u32 = 2000;

// ── State ─────────────────────────────────────────────────────────────────────

struct PqrAssocState {
    /// RMID extracted from bits[31:16], scaled 0–1000.
    pqr_rmid: u16,
    /// COS ID extracted from bits[15:0], scaled 0–1000.
    pqr_cos_id: u16,
    /// 1000 if lo != 0 (non-default QoS class active), else 0.
    pqr_isolated: u16,
    /// EMA of (rmid/4 + cos_id/4 + isolated/2).
    pqr_ema: u16,
}

static STATE: Mutex<PqrAssocState> = Mutex::new(PqrAssocState {
    pqr_rmid:     0,
    pqr_cos_id:   0,
    pqr_isolated: 0,
    pqr_ema:      0,
});

// ── CPUID guard — CPUID leaf 0x10 EAX bit 1 (L3 CAT supported) ───────────────

fn has_l3_cat() -> bool {
    // Step 1: confirm max basic CPUID leaf >= 0x10.
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0x10 {
        return false;
    }

    // Step 2: CPUID leaf 0x10, sub-leaf 0 — EAX bit 1 == L3 CAT supported.
    let eax_10: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x10u32 => eax_10,
            in("ecx") 0u32,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax_10 >> 1) & 1 != 0
}

// ── RDMSR helper ──────────────────────────────────────────────────────────────

#[inline]
unsafe fn rdmsr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    (lo, _hi)
}

// ── EMA helper — exact formula from spec ──────────────────────────────────────

#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Scale a raw u16 value to the 0–1000 range:
///   scaled = raw * 1000 / 65535, capped at 1000.
#[inline]
fn scale_u16(raw: u32) -> u16 {
    let s = raw.saturating_mul(1000) / 65535;
    if s > 1000 { 1000 } else { s as u16 }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the module — zero all signals.
pub fn init() {
    let mut s = STATE.lock();
    s.pqr_rmid     = 0;
    s.pqr_cos_id   = 0;
    s.pqr_isolated = 0;
    s.pqr_ema      = 0;
    crate::serial_println!(
        "[msr_ia32_pqr_assoc] init — IA32_PQR_ASSOC (0x{:03X}) module ready",
        IA32_PQR_ASSOC
    );
}

/// Called every kernel tick. Samples every 2000 ticks when L3 CAT is present.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    // Hardware guard — skip silently on platforms without L3 CAT.
    if !has_l3_cat() {
        return;
    }

    // Read IA32_PQR_ASSOC (0xC8F).
    // lo bits[31:16] = RMID, lo bits[15:0] = COS ID.
    let (lo, _hi) = unsafe { rdmsr(IA32_PQR_ASSOC) };

    // pqr_rmid: bits[31:16], scaled 0–1000.
    let rmid_raw = (lo >> 16) & 0xFFFF;
    let pqr_rmid = scale_u16(rmid_raw);

    // pqr_cos_id: bits[15:0], scaled 0–1000.
    let cos_raw = lo & 0xFFFF;
    let pqr_cos_id = scale_u16(cos_raw);

    // pqr_isolated: 1000 if lo != 0 (non-default QoS class active), else 0.
    let pqr_isolated: u16 = if lo != 0 { 1000 } else { 0 };

    // Composite for EMA: rmid/4 + cos_id/4 + isolated/2, capped at 1000.
    let composite_u32 = (pqr_rmid as u32) / 4
        + (pqr_cos_id as u32) / 4
        + (pqr_isolated as u32) / 2;
    let composite: u16 = if composite_u32 > 1000 { 1000 } else { composite_u32 as u16 };

    let mut s = STATE.lock();

    // pqr_ema: EMA((old*7 + new) / 8).
    let pqr_ema = ema(s.pqr_ema, composite);

    s.pqr_rmid     = pqr_rmid;
    s.pqr_cos_id   = pqr_cos_id;
    s.pqr_isolated = pqr_isolated;
    s.pqr_ema      = pqr_ema;

    crate::serial_println!(
        "[msr_ia32_pqr_assoc] age={} lo=0x{:08x} rmid={} cos_id={} isolated={} ema={}",
        age, lo, pqr_rmid, pqr_cos_id, pqr_isolated, pqr_ema
    );
}

// ── Accessors ─────────────────────────────────────────────────────────────────

/// RMID (Resource Monitoring ID) from bits[31:16], scaled 0–1000.
pub fn get_pqr_rmid() -> u16 {
    STATE.lock().pqr_rmid
}

/// COS ID (Class of Service ID) from bits[15:0], scaled 0–1000.
pub fn get_pqr_cos_id() -> u16 {
    STATE.lock().pqr_cos_id
}

/// 1000 if lo != 0 (non-default QoS class active), else 0.
pub fn get_pqr_isolated() -> u16 {
    STATE.lock().pqr_isolated
}

/// EMA of (rmid/4 + cos_id/4 + isolated/2).
pub fn get_pqr_ema() -> u16 {
    STATE.lock().pqr_ema
}
