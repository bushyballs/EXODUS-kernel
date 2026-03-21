// msr_mtrr_fix4k_c8.rs — IA32_MTRR_FIX4K_C8000 MSR (0x269)
// Fixed-range MTRR for region 0xC8000–0xCFFFF (32KB, 8 × 4KB sub-ranges)
// Part of the ROM/option-ROM region above 0xC0000.
// EXODUS kernel — bare-metal, no_std, no heap, no floats.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct MsrMtrrFix4kC8State {
    /// Count of WP (type 5) sub-ranges × 125, capped 1000
    pub fix4k_c8_wp: u16,
    /// Count of WB (type 6) sub-ranges × 125, capped 1000
    pub fix4k_c8_wb: u16,
    /// Count of UC (type 0) sub-ranges × 125, capped 1000
    pub fix4k_c8_uc: u16,
    /// EMA of (lo ^ hi).count_ones() × 31, capped 1000
    pub fix4k_c8_ema: u16,
}

impl MsrMtrrFix4kC8State {
    const fn zero() -> Self {
        Self {
            fix4k_c8_wp: 0,
            fix4k_c8_wb: 0,
            fix4k_c8_uc: 0,
            fix4k_c8_ema: 0,
        }
    }
}

static STATE: Mutex<MsrMtrrFix4kC8State> = Mutex::new(MsrMtrrFix4kC8State::zero());

// ---------------------------------------------------------------------------
// CPUID guard — check leaf 1 EDX bit 12 (MTRR supported)
// Uses push rbx/cpuid/mov esi,edx/pop rbx to preserve rbx (PIC register).
// ---------------------------------------------------------------------------

#[inline(always)]
fn mtrr_supported() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov esi, edx",
            "pop rbx",
            out("eax") _,
            out("esi") edx,
            out("ecx") _,
            options(nostack, preserves_flags),
        );
    }
    (edx >> 12) & 1 == 1
}

// ---------------------------------------------------------------------------
// rdmsr 0x269 — IA32_MTRR_FIX4K_C8000
// Returns (lo, hi) where lo = bits[31:0], hi = bits[63:32].
// Each byte encodes one 4KB sub-range memory type (8 sub-ranges total).
// ---------------------------------------------------------------------------

#[inline(always)]
fn read_mtrr_fix4k_c8() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x269u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, preserves_flags),
        );
    }
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Signal helpers
// ---------------------------------------------------------------------------

/// Extract the 8 memory-type bytes from the 64-bit MSR value (lo + hi).
/// Byte order: lo[7:0], lo[15:8], lo[23:16], lo[31:24],
///             hi[7:0], hi[15:8], hi[23:16], hi[31:24].
#[inline(always)]
fn extract_bytes(lo: u32, hi: u32) -> [u8; 8] {
    [
        (lo & 0xFF) as u8,
        ((lo >> 8) & 0xFF) as u8,
        ((lo >> 16) & 0xFF) as u8,
        ((lo >> 24) & 0xFF) as u8,
        (hi & 0xFF) as u8,
        ((hi >> 8) & 0xFF) as u8,
        ((hi >> 16) & 0xFF) as u8,
        ((hi >> 24) & 0xFF) as u8,
    ]
}

/// Count sub-ranges with the given memory type, multiply by 125, cap at 1000.
#[inline(always)]
fn count_type(bytes: &[u8; 8], mtype: u8) -> u16 {
    let mut count: u32 = 0;
    let mut i = 0usize;
    while i < 8 {
        if bytes[i] == mtype {
            count = count.wrapping_add(1);
        }
        i += 1;
    }
    // count is 0..=8; × 125 gives 0..=1000
    let scaled = count.wrapping_mul(125);
    if scaled > 1000 {
        1000u16
    } else {
        scaled as u16
    }
}

/// Compute EMA signal from (lo ^ hi).count_ones().
/// raw = (lo ^ hi).count_ones() × 31, capped to 1000.
/// EMA = (old × 7 + raw) / 8  — done in u32, cast to u16.
#[inline(always)]
fn compute_ema(lo: u32, hi: u32, old_ema: u16) -> u16 {
    let xor_ones = (lo ^ hi).count_ones(); // 0..=32
    let raw: u32 = xor_ones.wrapping_mul(31);
    let raw_capped: u32 = if raw > 1000 { 1000 } else { raw };

    let ema_u32: u32 = ((old_ema as u32).wrapping_mul(7).saturating_add(raw_capped)) / 8;
    if ema_u32 > 1000 {
        1000u16
    } else {
        ema_u32 as u16
    }
}

// ---------------------------------------------------------------------------
// Public tick — called from the life_tick() pipeline
// ---------------------------------------------------------------------------

pub fn tick(age: u32) {
    // Sample gate: only sample every 1000 ticks
    if age % 1000 != 0 {
        return;
    }

    // MTRR CPUID guard
    if !mtrr_supported() {
        crate::serial_println!(
            "[msr_mtrr_fix4k_c8] tick={} MTRR not supported (CPUID leaf1.EDX bit12=0), skipping",
            age
        );
        return;
    }

    // Read MSR 0x269
    let (lo, hi) = read_mtrr_fix4k_c8();

    let bytes = extract_bytes(lo, hi);

    // Compute signals
    let wp_signal = count_type(&bytes, 5); // WP = type 5
    let wb_signal = count_type(&bytes, 6); // WB = type 6
    let uc_signal = count_type(&bytes, 0); // UC = type 0

    let mut state = STATE.lock();
    let ema_signal = compute_ema(lo, hi, state.fix4k_c8_ema);

    state.fix4k_c8_wp = wp_signal;
    state.fix4k_c8_wb = wb_signal;
    state.fix4k_c8_uc = uc_signal;
    state.fix4k_c8_ema = ema_signal;

    crate::serial_println!(
        "[msr_mtrr_fix4k_c8] tick={} msr=0x{:08X}{:08X} wp={} wb={} uc={} ema={}",
        age,
        hi,
        lo,
        wp_signal,
        wb_signal,
        uc_signal,
        ema_signal
    );
}

// ---------------------------------------------------------------------------
// Public read — snapshot current state without ticking
// ---------------------------------------------------------------------------

pub fn read_state() -> MsrMtrrFix4kC8State {
    *STATE.lock()
}
