#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    cr0_fixed1_pe: u16,
    cr0_fixed1_pg: u16,
    cr0_fixed1_ne: u16,
    cr0_fixed1_richness_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    cr0_fixed1_pe: 0,
    cr0_fixed1_pg: 0,
    cr0_fixed1_ne: 0,
    cr0_fixed1_richness_ema: 0,
});

pub fn init() {
    serial_println!("[msr_vmx_cr0_fixed1] init");
}

pub fn tick(age: u32) {
    if age % 5000 != 0 {
        return;
    }

    // Check CPUID leaf 1 ECX bit 5 for VMX support.
    // Use push rbx/cpuid/mov esi,ebx/pop rbx to preserve rbx across the call.
    let vmx_supported: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            out("eax") _,
            out("esi") vmx_supported,
            out("ecx") _,
            out("edx") _,
            options(nostack),
        );
    }

    // Bit 5 of ECX (now in vmx_supported) indicates VMX support.
    if (vmx_supported >> 5) & 1 == 0 {
        // VMX not supported — return zeros (state already initialised to 0).
        return;
    }

    // Read IA32_VMX_CR0_FIXED1 MSR (0x487).
    // rdmsr returns: EDX:EAX — high 32 bits in edx, low 32 bits in eax.
    let fixed1_lo: u32;
    let _fixed1_hi: u32;
    unsafe {
        asm!(
            "mov ecx, 0x487",
            "rdmsr",
            out("eax") fixed1_lo,
            out("edx") _fixed1_hi,
            out("ecx") _,
            options(nostack, nomem),
        );
    }

    // --- Compute signals ---

    // cr0_fixed1_pe: bit 0 of fixed1_lo (Protection Enable allowed)
    let new_pe: u16 = if (fixed1_lo >> 0) & 1 == 1 { 1000 } else { 0 };

    // cr0_fixed1_pg: bit 31 of fixed1_lo (Paging allowed)
    let new_pg: u16 = if (fixed1_lo >> 31) & 1 == 1 { 1000 } else { 0 };

    // cr0_fixed1_ne: bit 5 of fixed1_lo (NE — Numeric Error allowed)
    let new_ne: u16 = if (fixed1_lo >> 5) & 1 == 1 { 1000 } else { 0 };

    // richness_ema: EMA of popcount(fixed1_lo), scaled 0-32 -> 0-1000 by *31
    let popcount = fixed1_lo.count_ones(); // 0..=32
    let richness_raw: u16 = (popcount.saturating_mul(31).min(1000)) as u16;

    // --- EMA update ---
    let mut state = MODULE.lock();

    // EMA formula: (old * 7 + new_val) / 8, computed in u32 then cast to u16.
    let old_ema = state.cr0_fixed1_richness_ema as u32;
    let new_ema = ((old_ema.wrapping_mul(7)).saturating_add(richness_raw as u32)) / 8;
    let new_ema_u16 = new_ema.min(1000) as u16;

    state.cr0_fixed1_pe = new_pe;
    state.cr0_fixed1_pg = new_pg;
    state.cr0_fixed1_ne = new_ne;
    state.cr0_fixed1_richness_ema = new_ema_u16;

    serial_println!(
        "[msr_vmx_cr0_fixed1] age={} pe={} pg={} ne={} richness_ema={}",
        age,
        state.cr0_fixed1_pe,
        state.cr0_fixed1_pg,
        state.cr0_fixed1_ne,
        state.cr0_fixed1_richness_ema,
    );
}
