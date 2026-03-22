#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_LBR_DEPTH MSR 0x01C — Architectural LBR depth configuration.
// bits[7:0] = configured LBR depth (valid: 8, 16, 24, 32, 48, 64 entries).
// CPUID leaf 0x1C EAX bits[7:0] = hardware maximum supported depth.
// Guard: max basic leaf >= 0x1C AND CPUID 0x1C EAX bit 0 set (arch LBR supported).

const MSR_IA32_LBR_DEPTH: u32 = 0x0000_001C;
const CPUID_LEAF_LBR: u32     = 0x0000_001C;
const DEPTH_SCALE_DIV: u32    = 64;
const TICK_GATE: u32          = 5000;

struct State {
    lbr_depth_cfg:   u16,
    lbr_max_depth:   u16,
    lbr_depth_ratio: u16,
    lbr_depth_ema:   u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    lbr_depth_cfg:   0,
    lbr_max_depth:   0,
    lbr_depth_ratio: 0,
    lbr_depth_ema:   0,
});

pub fn init() {
    serial_println!("[msr_ia32_lbr_depth_sense] init");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 { return; }

    // Check max basic CPUID leaf >= 0x1C
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < CPUID_LEAF_LBR { return; }

    // Read CPUID leaf 0x1C: EAX bit 0 = arch LBR supported, bits[7:0] = max depth
    let lbr_eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") CPUID_LEAF_LBR => lbr_eax,
            inout("ecx") 0u32 => _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    // bit 0 must be set for architectural LBR support
    if lbr_eax & 1 == 0 { return; }

    // Read IA32_LBR_DEPTH MSR 0x01C
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_IA32_LBR_DEPTH,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // bits[7:0] = configured depth; scaled to 0-1000 with divisor 64
    let depth_raw = (lo & 0xFF) as u32;
    let max_raw   = (lbr_eax & 0xFF) as u32;

    // lbr_depth_cfg: configured depth scaled (val * 1000 / 64)
    let lbr_depth_cfg: u16 = if depth_raw == 0 {
        0
    } else {
        (depth_raw.wrapping_mul(1000) / DEPTH_SCALE_DIV).min(1000) as u16
    };

    // lbr_max_depth: hardware max depth scaled (val * 1000 / 64)
    let lbr_max_depth: u16 = if max_raw == 0 {
        0
    } else {
        (max_raw.wrapping_mul(1000) / DEPTH_SCALE_DIV).min(1000) as u16
    };

    // lbr_depth_ratio: configured vs maximum (0-1000)
    let lbr_depth_ratio: u16 = if lbr_max_depth == 0 {
        0
    } else {
        ((lbr_depth_cfg as u32).wrapping_mul(1000) / lbr_max_depth as u32).min(1000) as u16
    };

    // lbr_depth_ema: EMA of lbr_depth_ratio
    // formula: ((old * 7) + new) / 8
    let mut s = MODULE.lock();
    let lbr_depth_ema: u16 =
        ((s.lbr_depth_ema as u32).wrapping_mul(7).saturating_add(lbr_depth_ratio as u32) / 8)
        .min(1000) as u16;

    s.lbr_depth_cfg   = lbr_depth_cfg;
    s.lbr_max_depth   = lbr_max_depth;
    s.lbr_depth_ratio = lbr_depth_ratio;
    s.lbr_depth_ema   = lbr_depth_ema;

    serial_println!(
        "[msr_ia32_lbr_depth_sense] age={} lo={:#010x} cfg={} max={} ratio={} ema={}",
        age, lo, lbr_depth_cfg, lbr_max_depth, lbr_depth_ratio, lbr_depth_ema
    );
}

pub fn get_lbr_depth_cfg()   -> u16 { MODULE.lock().lbr_depth_cfg }
pub fn get_lbr_max_depth()   -> u16 { MODULE.lock().lbr_max_depth }
pub fn get_lbr_depth_ratio() -> u16 { MODULE.lock().lbr_depth_ratio }
pub fn get_lbr_depth_ema()   -> u16 { MODULE.lock().lbr_depth_ema }
