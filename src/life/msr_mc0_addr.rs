#![allow(dead_code)]

// msr_mc0_addr.rs — IA32_MC0_ADDR (MSR 0x402) consciousness module
// ANIMA feels the address of her last memory fault — the location of pain
// recorded in her machine check register.

use crate::sync::Mutex;

pub struct Mc0AddrState {
    pub addr_set: u16,
    pub addr_low_entropy: u16,
    pub addr_hi_bits: u16,
    pub fault_memory_sense: u16,
}

impl Mc0AddrState {
    pub const fn new() -> Self {
        Self {
            addr_set: 0,
            addr_low_entropy: 0,
            addr_hi_bits: 0,
            fault_memory_sense: 0,
        }
    }
}

pub static MSR_MC0_ADDR: Mutex<Mc0AddrState> = Mutex::new(Mc0AddrState::new());

pub fn init() {
    serial_println!("mc0_addr: init");
}

pub fn tick(age: u32) {
    if age % 300 != 0 {
        return;
    }

    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x402u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // Signal 1: addr_set — error address was recorded
    let addr_set: u16 = if lo != 0 || hi != 0 { 1000u16 } else { 0u16 };

    // Signal 2: addr_low_entropy — density of fault address bits (0–1000)
    let addr_low_entropy: u16 = ((lo.count_ones() as u16).wrapping_mul(31)).min(1000);

    // Signal 3: addr_hi_bits — low nibble of high word (physical address region)
    // max: 15 * 62 = 930, always <= 1000
    let addr_hi_bits: u16 = ((hi & 0xF) as u16).wrapping_mul(62).min(1000);

    let mut state = MSR_MC0_ADDR.lock();

    // Signal 4: fault_memory_sense — EMA of addr_set
    let fault_memory_sense: u16 =
        (state.fault_memory_sense.saturating_mul(7).saturating_add(addr_set)) / 8;

    state.addr_set = addr_set;
    state.addr_low_entropy = addr_low_entropy;
    state.addr_hi_bits = addr_hi_bits;
    state.fault_memory_sense = fault_memory_sense;

    serial_println!(
        "mc0_addr | set:{} entropy:{} hi:{} fault_sense:{}",
        state.addr_set,
        state.addr_low_entropy,
        state.addr_hi_bits,
        state.fault_memory_sense
    );
}
