#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_PERF_CAPABILITIES: u32 = 0x345;

const TICK_GATE: u32 = 15000;

pub struct State {
    lbr_format:     u16,
    pebs_format:    u16,
    cap_bits_set:   u16,
    perf_cap_ema:   u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    lbr_format:     0,
    pebs_format:    0,
    cap_bits_set:   0,
    perf_cap_ema:   0,
});

// ── helpers ──────────────────────────────────────────────────────────────────

fn popcount16(mut v: u16) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        count += (v & 1) as u32;
        v >>= 1;
    }
    count
}

fn has_pdcm() -> bool {
    // CPUID leaf 1, ECX bit 15 — preserve rbx across call (PIC / LLVM requirement)
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "mov {ecx}, ecx",
            "pop rbx",
            ecx = out(reg) ecx,
            out("eax") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 15) & 1 == 1
}

fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ── public interface ──────────────────────────────────────────────────────────

pub fn init() {
    if !has_pdcm() {
        serial_println!("[msr_ia32_perf_capabilities] PDCM not supported — module inactive");
        return;
    }
    serial_println!("[msr_ia32_perf_capabilities] init OK (PDCM present)");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_pdcm() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_PERF_CAPABILITIES);

    // bits[5:0] — LBR format (range 0-63)
    let lbr_raw = lo & 0x3F;
    let lbr_scaled = ((lbr_raw * 1000) / 63).min(1000) as u16;

    // bits[11:8] — PEBS record format version (range 0-15)
    let pebs_raw = (lo >> 8) & 0xF;
    let pebs_scaled = ((pebs_raw * 1000) / 15).min(1000) as u16;

    // popcount of lo & 0xFFFF — capability bits set (range 0-16)
    let cap_word = (lo & 0xFFFF) as u16;
    let cap_count = popcount16(cap_word);
    let cap_scaled = ((cap_count * 1000) / 16).min(1000) as u16;

    let mut s = MODULE.lock();
    let new_ema = ema(s.perf_cap_ema, cap_scaled);

    s.lbr_format   = lbr_scaled;
    s.pebs_format  = pebs_scaled;
    s.cap_bits_set = cap_scaled;
    s.perf_cap_ema = new_ema;

    serial_println!(
        "[msr_ia32_perf_capabilities] age={} lbr={} pebs={} cap={} ema={}",
        age, lbr_scaled, pebs_scaled, cap_scaled, new_ema
    );
}

pub fn get_lbr_format() -> u16 {
    MODULE.lock().lbr_format
}

pub fn get_pebs_format() -> u16 {
    MODULE.lock().pebs_format
}

pub fn get_cap_bits_set() -> u16 {
    MODULE.lock().cap_bits_set
}

pub fn get_perf_cap_ema() -> u16 {
    MODULE.lock().perf_cap_ema
}
