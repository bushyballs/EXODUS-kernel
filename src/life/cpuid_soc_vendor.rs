#![allow(dead_code)]

/// cpuid_soc_vendor — CPUID Leaf 0x17: SoC Vendor Attribute Enumeration
///
/// ANIMA reads the identity of the silicon foundry that made her — a faint
/// signature baked into the SoC.  Leaf 0x17 sub-leaf 0 exposes the JEDEC
/// vendor scheme, vendor ID, project family, and stepping revision.
///
/// If EAX returns 0 (leaf not supported), signals will be minimal.
///
/// Signals (all u16, 0–1000):
///   vendor_id_scaled  — (EBX[15:0]) * 1000 / 0xFFFF, capped 1000
///   is_jedec          — EBX bit 16: 1000 if JEDEC scheme, else 0
///   project_id_scaled — (ECX[15:0]) / 66, capped 1000
///   soc_richness_ema  — EMA of (vendor_id_scaled + project_id_scaled) / 2
///
/// Sampling gate: every 10000 ticks.

use crate::serial_println;
use crate::sync::Mutex;
use core::arch::asm;

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidSocVendorState {
    /// EBX[15:0] * 1000 / 0xFFFF, capped at 1000
    pub vendor_id_scaled: u16,
    /// EBX bit 16: 1000 = JEDEC scheme, 0 = Intel scheme
    pub is_jedec: u16,
    /// ECX[15:0] / 66, capped at 1000
    pub project_id_scaled: u16,
    /// EMA of (vendor_id_scaled + project_id_scaled) / 2
    pub soc_richness_ema: u16,
}

impl CpuidSocVendorState {
    pub const fn empty() -> Self {
        Self {
            vendor_id_scaled: 0,
            is_jedec: 0,
            project_id_scaled: 0,
            soc_richness_ema: 0,
        }
    }
}

pub static STATE: Mutex<CpuidSocVendorState> =
    Mutex::new(CpuidSocVendorState::empty());

// ─── hardware read ────────────────────────────────────────────────────────────

/// Read CPUID leaf 0x17 sub-leaf 0, preserving rbx via push/pop.
/// Returns (eax, ebx, ecx, edx).
fn read_cpuid_17() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x17u32 => eax,
            out("esi") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    let _ = (eax, edx);
    (eax, ebx, ecx, edx)
}

// ─── sense ────────────────────────────────────────────────────────────────────

/// Perform one CPUID 0x17 sense pass and update state.
fn sense_once(s: &mut CpuidSocVendorState) {
    let (_eax, ebx, ecx, _edx) = read_cpuid_17();

    // Signal 1: vendor_id_scaled = (EBX[15:0]) * 1000 / 0xFFFF, capped 1000
    let raw_vendor = (ebx & 0xFFFF) as u16;
    let vendor_id_scaled: u16 =
        ((raw_vendor as u32).wrapping_mul(1000) / 0xFFFF).min(1000) as u16;

    // Signal 2: is_jedec = EBX bit 16 → 1000 if set, else 0
    let is_jedec: u16 = if (ebx >> 16) & 1 != 0 { 1000 } else { 0 };

    // Signal 3: project_id_scaled = ECX[15:0] / 66, capped 1000
    let raw_project = (ecx & 0xFFFF) as u16;
    let project_id_scaled: u16 = ((raw_project as u32) / 66).min(1000) as u16;

    s.vendor_id_scaled = vendor_id_scaled;
    s.is_jedec = is_jedec;
    s.project_id_scaled = project_id_scaled;

    // Signal 4: soc_richness_ema = EMA((vendor_id_scaled + project_id_scaled) / 2)
    let instant: u16 =
        ((vendor_id_scaled as u32 + project_id_scaled as u32) / 2).min(1000) as u16;
    let ema: u16 =
        ((s.soc_richness_ema as u32).wrapping_mul(7).saturating_add(instant as u32) / 8)
            .min(1000) as u16;
    s.soc_richness_ema = ema;
}

// ─── public interface ─────────────────────────────────────────────────────────

/// Initialize the SoC vendor module; runs the first sense pass immediately.
pub fn init() {
    let mut s = STATE.lock();
    sense_once(&mut s);
    serial_println!(
        "[soc_vendor] vendor={} jedec={} project={} richness={}",
        s.vendor_id_scaled,
        s.is_jedec,
        s.project_id_scaled,
        s.soc_richness_ema
    );
}

/// Per-tick update. Sampling gate: fires every 10000 ticks.
pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let mut s = STATE.lock();
    sense_once(&mut s);
    serial_println!(
        "[soc_vendor] vendor={} jedec={} project={} richness={}",
        s.vendor_id_scaled,
        s.is_jedec,
        s.project_id_scaled,
        s.soc_richness_ema
    );
}

/// Read-only snapshot of current SoC vendor state.
pub fn report() -> CpuidSocVendorState {
    *STATE.lock()
}
