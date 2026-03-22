#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    supported: bool,
    cache_ways_bits: u16,
    way_density: u16,
    contiguous_ways: u16,
    l3_alloc_ema: u16,
    last_tick: u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    supported: false,
    cache_ways_bits: 0,
    way_density: 0,
    contiguous_ways: 0,
    l3_alloc_ema: 0,
    last_tick: 0,
});

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

fn has_rdt_alloc() -> bool {
    let ebx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_save:e}, ebx",
            "pop rbx",
            ebx_save = out(reg) ebx_out,
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ebx_out >> 15) & 1 == 1
}

fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    (lo, _hi)
}

pub fn init() {
    let mut s = MODULE.lock();
    s.supported = has_rdt_alloc();
    s.cache_ways_bits = 0;
    s.way_density = 0;
    s.contiguous_ways = 0;
    s.l3_alloc_ema = 0;
    s.last_tick = 0;
    serial_println!(
        "[msr_ia32_l3_qos_mask] init: rdt_alloc_supported={}",
        s.supported
    );
}

pub fn tick(age: u32) {
    {
        let s = MODULE.lock();
        if age.wrapping_sub(s.last_tick) < 5000 {
            return;
        }
    }

    let mut s = MODULE.lock();
    s.last_tick = age;

    if !s.supported {
        serial_println!(
            "[msr_ia32_l3_qos_mask] tick age={} RDT-A not supported, skipping",
            age
        );
        return;
    }

    // Read IA32_L3_QOS_MASK_0 (MSR 0xC82)
    let (lo, _hi) = read_msr(0xC82u32);

    // cache_ways_bits: popcount of bits[19:0], scaled *50, capped 1000
    let ways_raw = lo & 0x000F_FFFFu32;
    let count = popcount(ways_raw);
    let scaled = (count * 50).min(1000) as u16;
    s.cache_ways_bits = scaled;

    // way_density: EMA of cache_ways_bits
    s.way_density = ema(s.way_density, s.cache_ways_bits);

    // contiguous_ways: if all set bits are contiguous (lo & (lo+1) == 0) => 1000 else 0
    let is_contiguous = if ways_raw == 0 {
        false
    } else {
        (ways_raw & ways_raw.wrapping_add(1)) == 0
    };
    s.contiguous_ways = if is_contiguous { 1000 } else { 0 };

    // l3_alloc_ema: EMA of composite (cache_ways_bits/2 + contiguous_ways/2)
    let composite = (s.cache_ways_bits / 2).saturating_add(s.contiguous_ways / 2);
    s.l3_alloc_ema = ema(s.l3_alloc_ema, composite);

    serial_println!(
        "[msr_ia32_l3_qos_mask] tick age={} msr_lo=0x{:08x} ways={} cache_ways_bits={} way_density={} contiguous={} l3_alloc_ema={}",
        age,
        lo,
        count,
        s.cache_ways_bits,
        s.way_density,
        s.contiguous_ways,
        s.l3_alloc_ema,
    );
}
