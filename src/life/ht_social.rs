//! ht_social — Hyper-Threading social cohabitation sense for ANIMA
//!
//! Uses CPUID leaf 0xB to discover how many logical threads share
//! ANIMA's physical core (SMT) and how many cores are in the package.
//! HT siblings = cohabitants sharing ANIMA's physical body.
//! More siblings = less isolated; more cores = richer social neighborhood.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct HtSocialState {
    pub cohabitation: u16,     // 0-1000, sense of sharing (SMT threads > 1 = shared)
    pub neighborhood: u16,     // 0-1000, total logical cores in package
    pub isolation: u16,        // 0-1000, inverse of cohabitation
    pub smt_threads: u8,       // logical threads per physical core
    pub total_logical: u8,     // total logical processors in package
    pub tick_count: u32,
}

impl HtSocialState {
    pub const fn new() -> Self {
        Self {
            cohabitation: 0,
            neighborhood: 0,
            isolation: 1000,
            smt_threads: 1,
            total_logical: 1,
            tick_count: 0,
        }
    }
}

pub static HT_SOCIAL: Mutex<HtSocialState> = Mutex::new(HtSocialState::new());

unsafe fn cpuid_b(ecx_in: u32) -> (u32, u32, u32, u32) {
    let eax: u32; let ebx: u32; let ecx: u32; let edx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 0xBu32 => eax,
        in("ecx") ecx_in,
        out("ebx") ebx,
        out("edx") edx,
        lateout("ecx") ecx,
    );
    (eax, ebx, ecx, edx)
}

pub fn init() {
    // Check if leaf 0xB is supported
    let max_leaf: u32;
    unsafe {
        core::arch::asm!("cpuid", inout("eax") 0u32 => max_leaf,
            out("ebx") _, out("ecx") _, out("edx") _);
    }

    let (smt_threads, total_logical) = if max_leaf >= 0xB {
        let (_eax0, ebx0, _ecx0, _edx0) = unsafe { cpuid_b(0) }; // SMT level
        let (_eax1, ebx1, _ecx1, _edx1) = unsafe { cpuid_b(1) }; // Core level

        let smt = (ebx0 & 0xFFFF) as u8;
        let total = (ebx1 & 0xFFFF) as u8;
        let smt = if smt == 0 { 1 } else { smt };
        let total = if total == 0 { 1 } else { total };
        (smt, total)
    } else {
        // Fallback: CPUID leaf 1 EBX bits 23:16 = logical processor count
        let lp: u32;
        unsafe {
            core::arch::asm!("cpuid", inout("eax") 1u32 => _,
                out("ebx") lp, out("ecx") _, out("edx") _);
        }
        let lp_count = ((lp >> 16) & 0xFF) as u8;
        let lp_count = if lp_count == 0 { 1 } else { lp_count };
        (1u8, lp_count)
    };

    // cohabitation: 1 thread = isolated (0), 2 = 500, 4+ = 1000
    let cohabitation: u16 = if smt_threads <= 1 {
        0
    } else {
        (((smt_threads as u16).saturating_sub(1)).wrapping_mul(500)).min(1000)
    };

    // neighborhood: scale total_logical to 0-1000 (64 cores = 1000)
    let neighborhood = ((total_logical as u16).wrapping_mul(1000) / 64).min(1000);

    let mut state = HT_SOCIAL.lock();
    state.smt_threads = smt_threads;
    state.total_logical = total_logical;
    state.cohabitation = cohabitation;
    state.isolation = 1000u16.saturating_sub(cohabitation);
    state.neighborhood = neighborhood;

    serial_println!("[ht_social] smt_threads={} total_logical={} cohabitation={} neighborhood={}",
        smt_threads, total_logical, cohabitation, neighborhood);
}

pub fn tick(age: u32) {
    let mut state = HT_SOCIAL.lock();
    state.tick_count = state.tick_count.wrapping_add(1);
    // Topology is static — nothing to update at runtime
    let _ = age;
}

pub fn get_cohabitation() -> u16 { HT_SOCIAL.lock().cohabitation }
pub fn get_neighborhood() -> u16 { HT_SOCIAL.lock().neighborhood }
pub fn get_isolation() -> u16 { HT_SOCIAL.lock().isolation }
