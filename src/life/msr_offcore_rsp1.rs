#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    offcore1_req_mask: u16,
    offcore1_resp_mask: u16,
    offcore1_dram_sense: u16,
    offcore1_config_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    offcore1_req_mask: 0,
    offcore1_resp_mask: 0,
    offcore1_dram_sense: 0,
    offcore1_config_ema: 0,
});

#[inline]
fn has_pdcm() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "mov esi, ecx", "pop rbx",
            in("eax") 1u32, out("esi") ecx_val,
            lateout("eax") _, lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx_val >> 15) & 1 == 1
}

fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 { c += v & 1; v >>= 1; }
    c
}

pub fn init() { serial_println!("[msr_offcore_rsp1] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_pdcm() { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1A7u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    let req = popcount(lo & 0x7FFF);
    let offcore1_req_mask = (req.saturating_mul(66)).min(1000) as u16;

    let resp = popcount(hi & 0x3FF);
    let offcore1_resp_mask = (resp.saturating_mul(100)).min(1000) as u16;

    let offcore1_dram_sense = if (lo >> 3) & 1 != 0 { 1000u16 } else { 0u16 };

    let composite = (offcore1_req_mask as u32 / 4)
        .saturating_add(offcore1_resp_mask as u32 / 4)
        .saturating_add(offcore1_dram_sense as u32 / 2);

    let mut s = MODULE.lock();
    let offcore1_config_ema = ((s.offcore1_config_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.offcore1_req_mask = offcore1_req_mask;
    s.offcore1_resp_mask = offcore1_resp_mask;
    s.offcore1_dram_sense = offcore1_dram_sense;
    s.offcore1_config_ema = offcore1_config_ema;

    serial_println!("[msr_offcore_rsp1] age={} req={} resp={} dram={} ema={}",
        age, offcore1_req_mask, offcore1_resp_mask, offcore1_dram_sense, offcore1_config_ema);
}
