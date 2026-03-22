#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── MSR address ──────────────────────────────────────────────────────────────
const MSR_IA32_HWP_INTERRUPT: u32 = 0x773;

// ── Sampling period ──────────────────────────────────────────────────────────
const SAMPLE_EVERY: u32 = 3000;

// ── Module state ─────────────────────────────────────────────────────────────
struct HwpIntrState {
    hwp_intr_perf_change_en:       u16,
    hwp_intr_guaranteed_change_en: u16,
    hwp_intr_excursion_min_en:     u16,
    hwp_intr_sensitivity:          u16,
    initialized:                   bool,
    supported:                     bool,
}

impl HwpIntrState {
    const fn new() -> Self {
        Self {
            hwp_intr_perf_change_en:       0,
            hwp_intr_guaranteed_change_en: 0,
            hwp_intr_excursion_min_en:     0,
            hwp_intr_sensitivity:          0,
            initialized:                   false,
            supported:                     false,
        }
    }
}

static STATE: Mutex<HwpIntrState> = Mutex::new(HwpIntrState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────
// CPUID leaf 6, EAX bit 7 = HWP supported
//             EAX bit 3 = HWP interrupt (notification) supported
fn has_hwp_interrupt() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    ((eax_val >> 7) & 1 != 0) && ((eax_val >> 3) & 1 != 0)
}

// ── MSR read ─────────────────────────────────────────────────────────────────
unsafe fn read_hwp_interrupt() -> u32 {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") MSR_IA32_HWP_INTERRUPT,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem)
    );
    lo
}

// ── EMA helper ────────────────────────────────────────────────────────────────
// EMA = (old * 7 + new_val) / 8, all u32 intermediate, result cast to u16.
#[inline]
fn ema_u16(old: u16, new_val: u16) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8) as u16
}

// ── Clamp to 0-1000 ───────────────────────────────────────────────────────────
#[inline]
fn clamp1000(v: u32) -> u16 {
    if v > 1000 { 1000 } else { v as u16 }
}

// ── Public: init ──────────────────────────────────────────────────────────────
pub fn init() {
    let mut s = STATE.lock();
    if s.initialized {
        return;
    }
    s.supported   = has_hwp_interrupt();
    s.initialized = true;
    crate::serial_println!(
        "[msr_ia32_hwp_interrupt] init: hwp_interrupt_supported={}",
        s.supported
    );
}

// ── Public: tick ──────────────────────────────────────────────────────────────
pub fn tick(age: u32) {
    if age % SAMPLE_EVERY != 0 {
        return;
    }

    let mut s = STATE.lock();

    if !s.initialized {
        return;
    }

    if !s.supported {
        crate::serial_println!(
            "[msr_ia32_hwp_interrupt] age={} (unsupported — all signals 0)",
            age
        );
        return;
    }

    // Read IA32_HWP_INTERRUPT (0x773), low 32 bits only.
    let lo: u32 = unsafe { read_hwp_interrupt() };

    // bit 0 — performance change interrupt enabled
    let perf_change_en: u16 = if (lo >> 0) & 1 != 0 { 1000 } else { 0 };
    // bit 1 — guaranteed performance change interrupt enabled
    let guaranteed_change_en: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    // bit 2 — excursion-to-minimum interrupt enabled
    let excursion_min_en: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };

    // hwp_intr_sensitivity = EMA of weighted composite signal:
    //   (perf_change * 500 + guaranteed_change * 300 + excursion * 200) / 1000
    // All arithmetic in u32 to avoid overflow.
    let composite: u32 = (perf_change_en as u32)
        .wrapping_mul(500)
        .saturating_add((guaranteed_change_en as u32).wrapping_mul(300))
        .saturating_add((excursion_min_en as u32).wrapping_mul(200))
        / 1000;
    let composite_clamped: u16 = clamp1000(composite);
    let sensitivity: u16 = ema_u16(s.hwp_intr_sensitivity, composite_clamped);

    s.hwp_intr_perf_change_en       = perf_change_en;
    s.hwp_intr_guaranteed_change_en = guaranteed_change_en;
    s.hwp_intr_excursion_min_en     = excursion_min_en;
    s.hwp_intr_sensitivity          = sensitivity;

    crate::serial_println!(
        "[msr_ia32_hwp_interrupt] age={} perf_change_en={} guaranteed_change_en={} excursion_min_en={} sensitivity={}",
        age, perf_change_en, guaranteed_change_en, excursion_min_en, sensitivity
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────
pub fn get_hwp_intr_perf_change_en() -> u16 {
    STATE.lock().hwp_intr_perf_change_en
}

pub fn get_hwp_intr_guaranteed_change_en() -> u16 {
    STATE.lock().hwp_intr_guaranteed_change_en
}

pub fn get_hwp_intr_excursion_min_en() -> u16 {
    STATE.lock().hwp_intr_excursion_min_en
}

pub fn get_hwp_intr_sensitivity() -> u16 {
    STATE.lock().hwp_intr_sensitivity
}
