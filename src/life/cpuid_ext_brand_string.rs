#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

/// CPUID leaves 0x80000002–0x80000004 — Processor Brand String
/// 48 ASCII chars. Signals: brand_richness, brand_nonzero, brand_digit_density, brand_ema
struct State {
    brand_richness: u16,
    brand_nonzero: u16,
    brand_digit_density: u16,
    brand_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    brand_richness: 0,
    brand_nonzero: 0,
    brand_digit_density: 0,
    brand_ema: 0,
});

/// Returns (eax, ebx, ecx, edx) for a given CPUID leaf.
#[inline]
unsafe fn cpuid_full(leaf: u32) -> (u32, u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    asm!(
        "push rbx",
        "cpuid",
        "mov {1:e}, ebx",
        "pop rbx",
        inout("eax") leaf => eax_out,
        out(reg) ebx_out,
        inout("ecx") 0u32 => ecx_out,
        out("edx") edx_out,
        options(nostack, nomem),
    );
    (eax_out, ebx_out, ecx_out, edx_out)
}

fn popcount_byte(b: u8) -> u32 {
    let mut v = b as u32;
    let mut c = 0u32;
    while v != 0 { c += v & 1; v >>= 1; }
    c
}

pub fn init() { serial_println!("[cpuid_ext_brand_string] init"); }

pub fn tick(age: u32) {
    if age % 8000 != 0 { return; }

    // Check extended CPUID max leaf
    let max_ext: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0x80000000u32 => max_ext,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    if max_ext < 0x80000004 { return; }

    // Collect 12 dwords (48 bytes) from leaves 2,3,4
    let mut bytes = [0u8; 48];
    let leaves = [0x80000002u32, 0x80000003u32, 0x80000004u32];
    for (i, &leaf) in leaves.iter().enumerate() {
        let (a, b, c, d) = unsafe { cpuid_full(leaf) };
        let base = i * 16;
        bytes[base..base+4].copy_from_slice(&a.to_le_bytes());
        bytes[base+4..base+8].copy_from_slice(&b.to_le_bytes());
        bytes[base+8..base+12].copy_from_slice(&c.to_le_bytes());
        bytes[base+12..base+16].copy_from_slice(&d.to_le_bytes());
    }

    // Count non-null bytes → brand_nonzero (0-1000)
    let nonzero_count = bytes.iter().filter(|&&b| b != 0).count() as u32;
    let brand_nonzero = ((nonzero_count * 1000) / 48).min(1000) as u16;

    // Popcount of all bytes → richness (more 1-bits = richer brand string)
    let total_bits: u32 = bytes.iter().map(|&b| popcount_byte(b)).sum();
    let max_bits: u32 = 48 * 8; // 384
    let brand_richness = ((total_bits * 1000) / max_bits).min(1000) as u16;

    // Count ASCII digit bytes ('0'-'9') → digit_density
    let digit_count = bytes.iter().filter(|&&b| b >= b'0' && b <= b'9').count() as u32;
    let brand_digit_density = ((digit_count * 1000) / 48).min(1000) as u16;

    let composite: u16 = (brand_richness / 4)
        .saturating_add(brand_nonzero / 4)
        .saturating_add(brand_digit_density / 2);

    let mut s = MODULE.lock();
    let ema = ((s.brand_ema as u32).wrapping_mul(7).saturating_add(composite as u32) / 8)
        .min(1000) as u16;
    s.brand_richness = brand_richness;
    s.brand_nonzero = brand_nonzero;
    s.brand_digit_density = brand_digit_density;
    s.brand_ema = ema;

    serial_println!("[cpuid_ext_brand_string] age={} richness={} nonzero={} digits={} ema={}",
        age, brand_richness, brand_nonzero, brand_digit_density, ema);
}

pub fn get_brand_richness() -> u16 { MODULE.lock().brand_richness }
pub fn get_brand_nonzero() -> u16 { MODULE.lock().brand_nonzero }
pub fn get_brand_digit_density() -> u16 { MODULE.lock().brand_digit_density }
pub fn get_brand_ema() -> u16 { MODULE.lock().brand_ema }
