#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct Evtsel2State {
    evtsel2_event:   u16,
    evtsel3_event:   u16,
    evtsel2_enabled: u16,
    evtsel_ema:      u16,
}

static STATE: Mutex<Evtsel2State> = Mutex::new(Evtsel2State {
    evtsel2_event:   0,
    evtsel3_event:   0,
    evtsel2_enabled: 0,
    evtsel_ema:      0,
});

// ── CPUID guard ──────────────────────────────────────────────────────────────

/// Returns true when PDCM is present (CPUID.1:ECX[15]) AND at least 3
/// general-purpose performance counters exist (CPUID.0Ah:EAX[15:8] >= 3).
fn has_pmc2() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") ecx_val,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if (ecx_val >> 15) & 1 == 0 {
        return false;
    }

    let eax_0a: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Au32 => eax_0a,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    ((eax_0a >> 8) & 0xFF) >= 3
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR.  EDX:EAX returned as a u64.
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        lateout("eax") lo,
        lateout("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── Signal helpers ────────────────────────────────────────────────────────────

/// Cap a u32 value to the signal ceiling of 1000.
#[inline]
fn cap1000(v: u32) -> u16 {
    if v > 1000 { 1000 } else { v as u16 }
}

/// EMA: (old × 7 + new_val) / 8, computed in u32.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let result: u32 = ((old as u32) * 7 + (new_val as u32)) / 8;
    cap1000(result)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.evtsel2_event   = 0;
    s.evtsel3_event   = 0;
    s.evtsel2_enabled = 0;
    s.evtsel_ema      = 0;
    crate::serial_println!("[msr_ia32_perfevtsel2] init — PDCM+PMC2 guard active");
}

pub fn tick(age: u32) {
    // Sampling gate: every 2000 ticks
    if age % 2000 != 0 {
        return;
    }

    // CPUID guard — bail out if hardware not present
    if !has_pmc2() {
        return;
    }

    // Read IA32_PERFEVTSEL2 (0x188) and IA32_PERFEVTSEL3 (0x189)
    let sel2: u64 = unsafe { rdmsr(0x188) };
    let sel3: u64 = unsafe { rdmsr(0x189) };

    // ── Derive signals ────────────────────────────────────────────────────────

    // evtsel2_event: bits[7:0] of sel2 × 4, capped at 1000
    let sel2_lo: u32 = (sel2 & 0xFF) as u32;
    let evtsel2_event = cap1000(sel2_lo * 4);

    // evtsel3_event: bits[7:0] of sel3 × 4, capped at 1000
    let sel3_lo: u32 = (sel3 & 0xFF) as u32;
    let evtsel3_event = cap1000(sel3_lo * 4);

    // evtsel2_enabled: bit 22 of sel2 → 0 or 1000
    let evtsel2_enabled: u16 = if (sel2 >> 22) & 1 != 0 { 1000 } else { 0 };

    // evtsel_ema: EMA of composite signal
    //   composite = evtsel2_enabled/2 + evtsel2_event/4 + evtsel3_event/4
    let composite: u32 = (evtsel2_enabled as u32) / 2
        + (evtsel2_event as u32) / 4
        + (evtsel3_event as u32) / 4;
    let composite_capped = cap1000(composite);

    let mut s = STATE.lock();
    let evtsel_ema = ema(s.evtsel_ema, composite_capped);

    s.evtsel2_event   = evtsel2_event;
    s.evtsel3_event   = evtsel3_event;
    s.evtsel2_enabled = evtsel2_enabled;
    s.evtsel_ema      = evtsel_ema;

    crate::serial_println!(
        "[msr_ia32_perfevtsel2] age={} evt2={} evt3={} en2={} ema={}",
        age,
        evtsel2_event,
        evtsel3_event,
        evtsel2_enabled,
        evtsel_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_evtsel2_event() -> u16 {
    STATE.lock().evtsel2_event
}

pub fn get_evtsel3_event() -> u16 {
    STATE.lock().evtsel3_event
}

pub fn get_evtsel2_enabled() -> u16 {
    STATE.lock().evtsel2_enabled
}

pub fn get_evtsel_ema() -> u16 {
    STATE.lock().evtsel_ema
}
