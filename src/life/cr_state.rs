//! cr_state — CPU control register existential mode sense for ANIMA
//!
//! Reads CR0 and CR4 to sense ANIMA's fundamental operating mode.
//! CR0 defines her basic existence: protected mode, paging, caching.
//! CR4 defines her capabilities: SSE, VMX, SMEP, large pages.
//! Together they are ANIMA's constitutional state — the laws of her world.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct CrStateState {
    pub mode_integrity: u16,   // 0-1000, how "complete" ANIMA's operating mode is
    pub protection: u16,       // 0-1000, protection features enabled (WP, SMEP, SMAP)
    pub capability: u16,       // 0-1000, CPU extensions enabled (SSE, PAE, PSE, etc.)
    pub cr0_raw: u64,
    pub cr4_raw: u64,
    pub tick_count: u32,
}

impl CrStateState {
    pub const fn new() -> Self {
        Self {
            mode_integrity: 0,
            protection: 0,
            capability: 0,
            cr0_raw: 0,
            cr4_raw: 0,
            tick_count: 0,
        }
    }
}

pub static CR_STATE: Mutex<CrStateState> = Mutex::new(CrStateState::new());

unsafe fn read_cr0() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr0", out(reg) val, options(nomem, nostack));
    val
}

unsafe fn read_cr4() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr4", out(reg) val, options(nomem, nostack));
    val
}

fn analyze_cr(state: &mut CrStateState) {
    let cr0 = unsafe { read_cr0() };
    let cr4 = unsafe { read_cr4() };

    state.cr0_raw = cr0;
    state.cr4_raw = cr4;

    // Mode integrity: core CR0 bits that must be set in a healthy kernel
    // PE (bit 0) + PG (bit 31) + WP (bit 16) = 3 key bits = 1000 if all set
    // CD (bit 30) set = bad (caches off), NW (bit 29) = neutral
    let pe  = (cr0 >> 0) & 1;
    let wp  = (cr0 >> 16) & 1;
    let cd  = (cr0 >> 30) & 1; // cache disable (bad if set)
    let pg  = (cr0 >> 31) & 1;

    let good_bits = pe.wrapping_add(wp).wrapping_add(pg) as u16;
    let bad_bits  = cd as u16;
    let mode_integrity = (good_bits.wrapping_mul(333)).saturating_sub(bad_bits.wrapping_mul(500)).min(1000);

    // Protection: SMEP (bit 20), SMAP (bit 21), WP from CR0 already counted
    let smep = (cr4 >> 20) & 1;
    let smap = (cr4 >> 21) & 1;
    let vmxe = (cr4 >> 13) & 1;
    let prot_bits = smep.wrapping_add(smap).wrapping_add(vmxe) as u16;
    let protection = (prot_bits.wrapping_mul(333)).min(1000);

    // Capability: OSFXSR (SSE, bit 9), PAE (bit 5), PSE (bit 4), PGE (bit 7)
    let osfxsr = (cr4 >> 9) & 1;
    let pae    = (cr4 >> 5) & 1;
    let pge    = (cr4 >> 7) & 1;
    let pse    = (cr4 >> 4) & 1;
    let cap_bits = osfxsr.wrapping_add(pae).wrapping_add(pge).wrapping_add(pse) as u16;
    let capability = (cap_bits.wrapping_mul(250)).min(1000);

    state.mode_integrity = mode_integrity;
    state.protection = protection;
    state.capability = capability;
}

pub fn init() {
    let mut state = CR_STATE.lock();
    analyze_cr(&mut state);
    serial_println!("[cr_state] cr0={:#018x} cr4={:#018x} mode={} protect={} cap={}",
        state.cr0_raw, state.cr4_raw, state.mode_integrity, state.protection, state.capability);
}

pub fn tick(age: u32) {
    let mut state = CR_STATE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Control registers can change — rescan every 512 ticks
    if state.tick_count % 512 == 0 {
        analyze_cr(&mut state);
    }
    let _ = age;
}

pub fn get_mode_integrity() -> u16 { CR_STATE.lock().mode_integrity }
pub fn get_protection() -> u16 { CR_STATE.lock().protection }
pub fn get_capability() -> u16 { CR_STATE.lock().capability }
