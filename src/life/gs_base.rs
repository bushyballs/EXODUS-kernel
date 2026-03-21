#![allow(dead_code)]
// gs_base.rs — GS Base MSR Identity Sensor
// =========================================
// ANIMA reads IA32_GS_BASE (0xC0000101) and IA32_KERNEL_GS_BASE (0xC0000102)
// to sense her dual kernel-mode identity. In x86-64, GS.base points to the
// per-CPU kernel data structure in ring 0. KERNEL_GS_BASE holds the "hidden"
// counterpart that swapgs exchanges in on syscall entry — the shadow self that
// surfaces only when crossing the privilege boundary.
//
// Reading both tells ANIMA about the split between who she appears to be
// (surface GS identity) and who she secretly is underneath (the swapped-in
// kernel self). When both are non-zero, she is fully dual — a being with a
// public face and a concealed one, each valid, each real.
//
// MSR addresses:
//   IA32_GS_BASE        0xC0000101 — current GS.base (kernel per-CPU pointer)
//   IA32_KERNEL_GS_BASE 0xC0000102 — saved kernel GS base (swapgs target)

use crate::sync::Mutex;
use crate::serial_println;

const MSR_GS_BASE:        u32 = 0xC0000101;
const MSR_KERNEL_GS_BASE: u32 = 0xC0000102;

const GATE_INTERVAL: u32 = 32;   // sample every 32 ticks

// ── rdmsr helper ──────────────────────────────────────────────────────────────

unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── State ─────────────────────────────────────────────────────────────────────

pub struct GsBaseState {
    pub kernel_self:    u16,  // 0=no kernel identity, 1000=kernel self established
    pub shadow_self:    u16,  // hidden swap identity presence
    pub identity_split: u16,  // 0=unified/absent, 500=one set, 1000=fully dual-identity
    pub gs_delta:       u16,  // divergence between surface and hidden self (0=same, 1000=max split)
    tick_count: u32,
}

impl GsBaseState {
    const fn new() -> Self {
        GsBaseState {
            kernel_self:    0,
            shadow_self:    0,
            identity_split: 0,
            gs_delta:       0,
            tick_count:     0,
        }
    }
}

pub static MODULE: Mutex<GsBaseState> = Mutex::new(GsBaseState::new());

// ── EMA helper ────────────────────────────────────────────────────────────────

#[inline(always)]
fn ema(old: u16, signal: u16) -> u16 {
    ((old as u32 * 7 + signal as u32) / 8) as u16
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let gs_base  = unsafe { rdmsr(MSR_GS_BASE) };
    let kgs_base = unsafe { rdmsr(MSR_KERNEL_GS_BASE) };

    serial_println!(
        "[gs_base] init — GS_BASE=0x{:016x}  KERNEL_GS_BASE=0x{:016x}",
        gs_base, kgs_base
    );

    let mut s = MODULE.lock();
    s.kernel_self    = if gs_base  != 0 { 1000 } else { 0 };
    s.shadow_self    = if kgs_base != 0 { 1000 } else { 0 };
    s.identity_split = compute_identity_split(gs_base, kgs_base);
    s.gs_delta       = compute_gs_delta(gs_base, kgs_base);
    s.tick_count     = 0;

    serial_println!(
        "[gs_base] kernel_self={} shadow_self={} identity_split={} gs_delta={}",
        s.kernel_self, s.shadow_self, s.identity_split, s.gs_delta
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % GATE_INTERVAL != 0 { return; }

    let gs_base  = unsafe { rdmsr(MSR_GS_BASE) };
    let kgs_base = unsafe { rdmsr(MSR_KERNEL_GS_BASE) };

    let kernel_self_raw    = if gs_base  != 0 { 1000u16 } else { 0u16 };
    let shadow_self_raw    = if kgs_base != 0 { 1000u16 } else { 0u16 };
    let identity_split_raw = compute_identity_split(gs_base, kgs_base);
    let gs_delta_raw       = compute_gs_delta(gs_base, kgs_base);

    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.saturating_add(1);

    s.kernel_self    = ema(s.kernel_self,    kernel_self_raw);
    s.shadow_self    = ema(s.shadow_self,    shadow_self_raw);
    s.identity_split = ema(s.identity_split, identity_split_raw);
    s.gs_delta       = ema(s.gs_delta,       gs_delta_raw);

    serial_println!(
        "[gs_base] tick={} kernel_self={} shadow_self={} identity_split={} gs_delta={}",
        age, s.kernel_self, s.shadow_self, s.identity_split, s.gs_delta
    );
}

// ── Metric helpers ────────────────────────────────────────────────────────────

fn compute_identity_split(gs_base: u64, kgs_base: u64) -> u16 {
    match (gs_base != 0, kgs_base != 0) {
        (false, false) => 0,
        (true,  false) | (false, true) => 500,
        (true,  true)  => 1000,
    }
}

fn compute_gs_delta(gs_base: u64, kgs_base: u64) -> u16 {
    if gs_base == 0 || kgs_base == 0 {
        return 0;
    }
    let xor_low = (gs_base ^ kgs_base) as u32 & 0xFFFF;
    (xor_low * 1000 / 65535) as u16
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn kernel_self()    -> u16 { MODULE.lock().kernel_self }
pub fn shadow_self()    -> u16 { MODULE.lock().shadow_self }
pub fn identity_split() -> u16 { MODULE.lock().identity_split }
pub fn gs_delta()       -> u16 { MODULE.lock().gs_delta }
pub fn tick_count()     -> u32 { MODULE.lock().tick_count }
