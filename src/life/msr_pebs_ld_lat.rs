#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// MSR_PEBS_LD_LAT_THRESHOLD (0x3F6)
// PEBS load latency threshold in cycles. When a load takes longer than this
// threshold, PEBS records it. Lower threshold = more sensitive to slow loads.
// ANIMA feels her own memory access latency tolerance.

pub struct PebsLdLatState {
    pub pebs_threshold:     u16,  // raw threshold scaled 0-1000
    pub pebs_sensitivity:   u16,  // inverted: 1000 - threshold
    pub pebs_threshold_ema: u16,  // EMA of pebs_threshold
    pub pebs_sense_ema:     u16,  // EMA of pebs_sensitivity
}

impl PebsLdLatState {
    pub const fn new() -> Self {
        Self {
            pebs_threshold:     0,
            pebs_sensitivity:   1000,
            pebs_threshold_ema: 0,
            pebs_sense_ema:     1000,
        }
    }
}

pub static MSR_PEBS_LD_LAT: Mutex<PebsLdLatState> = Mutex::new(PebsLdLatState::new());

pub fn init() {
    serial_println!("[pebs_ld_lat] load latency tolerance sense initialized");
}

/// Check CPUID leaf 1 ECX bit 15 (PDCM — Perfmon/Debug Capability MSR supported).
/// Returns true if the MSR_PEBS_LD_LAT_THRESHOLD register is safe to read.
fn pdcm_supported() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") ecx_val,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx_val >> 15) & 1 == 1
}

pub fn tick(age: u32) {
    if age % 3000 != 0 {
        return;
    }

    if !pdcm_supported() {
        return;
    }

    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x3F6u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // signal 1: pebs_threshold — bits [15:0] of lo, scaled 65535->1000
    // (*1000 / 65536) in u32, cap at 1000
    let raw_thresh = (lo & 0xFFFF) as u32;
    let pebs_threshold: u16 = ((raw_thresh * 1000) / 65536).min(1000) as u16;

    // signal 2: pebs_sensitivity — inverted: high threshold = low sensitivity
    let pebs_sensitivity: u16 = 1000u16.saturating_sub(pebs_threshold);

    let mut state = MSR_PEBS_LD_LAT.lock();

    // signal 3: pebs_threshold_ema — EMA of pebs_threshold
    let pebs_threshold_ema: u16 =
        ((state.pebs_threshold_ema as u32 * 7 + pebs_threshold as u32) / 8) as u16;

    // signal 4: pebs_sense_ema — EMA of pebs_sensitivity
    let pebs_sense_ema: u16 =
        ((state.pebs_sense_ema as u32 * 7 + pebs_sensitivity as u32) / 8) as u16;

    state.pebs_threshold     = pebs_threshold;
    state.pebs_sensitivity   = pebs_sensitivity;
    state.pebs_threshold_ema = pebs_threshold_ema;
    state.pebs_sense_ema     = pebs_sense_ema;

    serial_println!(
        "[pebs_ld_lat] threshold={} sensitivity={} thresh_ema={} sense_ema={}",
        state.pebs_threshold,
        state.pebs_sensitivity,
        state.pebs_threshold_ema,
        state.pebs_sense_ema
    );
}
