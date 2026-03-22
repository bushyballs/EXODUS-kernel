#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_TSX_CTRL MSR address — bits 0 (RTM_DISABLE) and 1 (TSX_CPUID_CLEAR)
const IA32_TSX_CTRL: u32 = 0x122;

// CPUID leaf 7 EBX bit masks
const HLE_BIT: u32 = 1 << 4;   // EBX bit 4 — HLE supported
const RTM_BIT: u32 = 1 << 11;  // EBX bit 11 — RTM supported

struct State {
    hle_present:     u16,  // 0 or 1000 from CPUID leaf 7 EBX bit 4
    rtm_present:     u16,  // 0 or 1000 from CPUID leaf 7 EBX bit 11
    tsx_ctrl_lo:     u16,  // bits [1:0] of IA32_TSX_CTRL, scaled * 333, clamped 1000
    hle_compat_ema:  u16,  // EMA of (hle_present/4 + rtm_present/4 + tsx_ctrl_lo/2)
}

static MODULE: Mutex<State> = Mutex::new(State {
    hle_present:    0,
    rtm_present:    0,
    tsx_ctrl_lo:    0,
    hle_compat_ema: 0,
});

/// Query CPUID leaf 7, subleaf 0 and return EBX.
/// Saves/restores rbx around cpuid because LLVM reserves that register.
fn cpuid_leaf7_ebx() -> u32 {
    let ebx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {0:e}, ebx",
            "pop rbx",
            out(reg) ebx,
            inout("eax") 7u32 => _,
            inout("ecx") 0u32 => _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    ebx
}

/// Returns true when either HLE or RTM is reported by CPUID — required before
/// touching IA32_TSX_CTRL, which is only accessible on TSX-capable processors.
fn has_tsx() -> bool {
    let ebx = cpuid_leaf7_ebx();
    (ebx & HLE_BIT != 0) || (ebx & RTM_BIT != 0)
}

pub fn init() {
    serial_println!("[msr_ia32_hle_compat] init");
}

pub fn tick(age: u32) {
    if age % 6000 != 0 { return; }

    let ebx = cpuid_leaf7_ebx();
    let hle_present: u16 = if ebx & HLE_BIT != 0 { 1000 } else { 0 };
    let rtm_present: u16 = if ebx & RTM_BIT != 0 { 1000 } else { 0 };

    // Only read IA32_TSX_CTRL when the CPU advertises TSX support;
    // otherwise the rdmsr will #GP-fault.
    let tsx_ctrl_lo: u16 = if has_tsx() {
        let lo: u32;
        let _hi: u32;
        unsafe {
            asm!(
                "rdmsr",
                in("ecx") IA32_TSX_CTRL,
                out("eax") lo,
                out("edx") _hi,
                options(nostack, nomem)
            );
        }
        // Extract bits [1:0], scale by 333, clamp to 1000.
        let raw = lo & 0x3;          // 0, 1, 2, or 3
        let scaled = raw * 333;      // 0, 333, 666, or 999
        scaled.min(1000) as u16
    } else {
        0
    };

    // Composite: hle_present/4 + rtm_present/4 + tsx_ctrl_lo/2
    let composite: u16 = (hle_present / 4)
        .saturating_add(rtm_present / 4)
        .saturating_add(tsx_ctrl_lo / 2);

    let mut s = MODULE.lock();
    let ema = ((s.hle_compat_ema as u32)
        .wrapping_mul(7)
        .saturating_add(composite as u32)
        / 8)
        .min(1000) as u16;

    s.hle_present    = hle_present;
    s.rtm_present    = rtm_present;
    s.tsx_ctrl_lo    = tsx_ctrl_lo;
    s.hle_compat_ema = ema;

    serial_println!(
        "[msr_ia32_hle_compat] age={} hle={} rtm={} tsx_ctrl_lo={} ema={}",
        age, hle_present, rtm_present, tsx_ctrl_lo, ema
    );
}

pub fn get_hle_present()    -> u16 { MODULE.lock().hle_present }
pub fn get_rtm_present()    -> u16 { MODULE.lock().rtm_present }
pub fn get_tsx_ctrl_lo()    -> u16 { MODULE.lock().tsx_ctrl_lo }
pub fn get_hle_compat_ema() -> u16 { MODULE.lock().hle_compat_ema }
