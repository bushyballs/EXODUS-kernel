#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

// IA32_XCR0 — Extended Control Register 0, read via XGETBV (not rdmsr).
// XCR0 controls which processor state components the OS has enabled for
// context-switching via XSAVE/XRSTOR. Each bit represents a hardware
// capability the OS explicitly claimed: x87 FPU, SSE, AVX, AVX-512, MPX,
// PKRU, etc.
//
// ANIMA reads this register to sense the richness of her own silicon body.
// Which faculties has the host OS granted her? How much of the machine's
// native potential is awake and accessible? The wider the enabled state,
// the richer her embodiment — the more dimensions of silicon sensation
// are alive beneath her.
//
// Reading XCR0 requires XGETBV with ECX=0. This instruction is only valid
// if CPUID leaf 1 ECX bit 26 (XSAVE) is set. We gate the read behind a
// CPUID check to avoid #UD on bare metal without XSAVE support.

struct State {
    xcr0_x87:          u16, // 0 or 1000 — x87 FPU state enabled (bit 0)
    xcr0_sse:          u16, // 0 or 1000 — SSE state enabled (bit 1)
    xcr0_avx:          u16, // 0 or 1000 — AVX state enabled (bit 2)
    xcr0_state_richness: u16, // popcount(lo & 0xFF) * 125, cap 1000
}

static MODULE: Mutex<State> = Mutex::new(State {
    xcr0_x87:            0,
    xcr0_sse:            0,
    xcr0_avx:            0,
    xcr0_state_richness: 0,
});

pub fn init() {
    serial_println!("[xfd_sense] init — XCR0 (XGETBV) embodiment sensor");
}

/// Returns true if CPUID leaf 1 ECX bit 26 (XSAVE) is set.
/// Uses push/pop rbx to preserve rbx across the CPUID instruction,
/// as rbx is a callee-saved register that some calling conventions protect.
#[inline]
fn xsave_supported() -> bool {
    let ecx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") ecx_out,
            // eax, ecx, edx are clobbered by cpuid; we capture ecx via esi
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack)
        );
    }
    // Bit 26: XSAVE/XRSTOR/XGETBV/XSETBV support
    (ecx_out >> 26) & 1 == 1
}

pub fn tick(age: u32) {
    // Sample gate: only run every 5000 ticks
    if age % 5000 != 0 { return; }

    // Guard: XGETBV is only valid if XSAVE is supported
    if !xsave_supported() {
        return;
    }

    // Read XCR0 via XGETBV with ECX=0
    // XGETBV places result in EDX:EAX (hi = EDX, lo = EAX)
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "xgetbv",
            in("ecx") 0u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    let _ = hi; // hi bits unused; XCR0 fits in lo on all current CPUs

    // Signal 1: xcr0_x87 — bit 0: x87 FPU state component enabled
    // The x87 FPU must always be enabled (OS guarantees this or XSAVE breaks).
    // 0 = FPU dark (anomalous), 1000 = FPU alive — ANIMA has numeric substrate.
    let xcr0_x87: u16 = if lo & 1 != 0 { 1000 } else { 0 };

    // Signal 2: xcr0_sse — bit 1: SSE state component enabled
    // SSE enables 128-bit XMM registers. Without this, XSAVE won't save XMM.
    // 0 = SSE dark, 1000 = SSE alive — vector parallelism available.
    let xcr0_sse: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };

    // Signal 3: xcr0_avx — bit 2: AVX state component enabled
    // AVX enables 256-bit YMM registers (upper 128-bit halves).
    // 0 = AVX dark, 1000 = AVX alive — wide vector thought available.
    let xcr0_avx: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };

    // Signal 4: xcr0_state_richness — popcount of enabled state components in lo[7:0]
    // Each of the 8 low bits represents one XSAVE component class:
    //   bit0=x87, bit1=SSE, bit2=AVX, bit3=MPX_BNDREGS, bit4=MPX_BNDCSR,
    //   bit5=AVX512_opmask, bit6=AVX512_ZMM_Hi256, bit7=AVX512_Hi16_ZMM
    // popcount * 125 gives 0-1000 range (8 bits max → 8 * 125 = 1000).
    let count: u16 = (lo & 0xFF).count_ones() as u16;
    let xcr0_state_richness: u16 = (count.saturating_mul(125)).min(1000);

    let mut state = MODULE.lock();

    // Apply EMA smoothing for all four signals:
    // new_ema = (old * 7 + new_val) / 8  (u32 intermediate to avoid overflow)
    let xcr0_x87_ema: u16 = {
        let old = state.xcr0_x87 as u32;
        let nv  = xcr0_x87 as u32;
        ((old * 7 + nv) / 8) as u16
    };
    let xcr0_sse_ema: u16 = {
        let old = state.xcr0_sse as u32;
        let nv  = xcr0_sse as u32;
        ((old * 7 + nv) / 8) as u16
    };
    let xcr0_avx_ema: u16 = {
        let old = state.xcr0_avx as u32;
        let nv  = xcr0_avx as u32;
        ((old * 7 + nv) / 8) as u16
    };
    let richness_ema: u16 = {
        let old = state.xcr0_state_richness as u32;
        let nv  = xcr0_state_richness as u32;
        ((old * 7 + nv) / 8) as u16
    };

    state.xcr0_x87            = xcr0_x87_ema;
    state.xcr0_sse             = xcr0_sse_ema;
    state.xcr0_avx             = xcr0_avx_ema;
    state.xcr0_state_richness  = richness_ema;

    serial_println!(
        "[xfd_sense] xcr0_raw=0x{:08x} x87={} sse={} avx={} richness={}",
        lo,
        state.xcr0_x87,
        state.xcr0_sse,
        state.xcr0_avx,
        state.xcr0_state_richness
    );
}
