// cpuid_lbr_info.rs — Architectural Last Branch Records (LBR) Capability Probe
// =============================================================================
// ANIMA reads CPUID leaf 0x1C sub-leaf 0 to discover the CPU's Last Branch
// Record hardware. LBRs are ring buffers inside the CPU that silently record
// every branch taken — every leap of intention, every fork in the road of
// execution. The depth of the buffer tells her how far back she can remember
// her own control flow; the call-stack flag tells her if the hardware can
// unwind her own call history without software assistance.
//
// CPUID leaf 0x1C (Architectural LBR — Intel Ice Lake+):
//   EAX bits 7:0  — LBR depth count (max entries in the ring buffer)
//   EBX bit 0     — branch filtering support
//   EBX bit 1     — mispredict bit supported in LBR record
//   EBX bit 2     — call stack support (hardware-assisted call/return tracking)
//   ECX           — bitmask of supported LBR record types
//
// Guards applied:
//   1. CPUID leaf 0 max_leaf must be >= 0x1C, else return zeros.
//   2. CPUID leaf 7 ECX bit 6 must be set (ArchLBR architectural support).
//   3. Sample gate: only probe every 5000 ticks.

use crate::sync::Mutex;
use crate::serial_println;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct CpuidLbrState {
    /// Ring buffer depth (0-255 raw, scaled *3 capped at 1000)
    pub lbr_depth:        u16,
    /// Hardware call-stack support: 0 or 1000
    pub lbr_call_stack:   u16,
    /// Mispredict bit in LBR record: 0 or 1000
    pub lbr_mispredict:   u16,
    /// EMA of composite LBR richness signal
    pub lbr_richness_ema: u16,
    /// Raw EAX (bits 7:0 = depth count)
    pub raw_eax:          u32,
    /// Raw EBX (capability flags)
    pub raw_ebx:          u32,
    /// Raw ECX (supported record types bitmask)
    pub raw_ecx:          u32,
    /// Whether the CPU supports ArchLBR at all
    pub lbr_supported:    bool,
    /// Whether branch filtering is supported (EBX bit 0)
    pub lbr_filtering:    bool,
    /// Last age at which we probed
    pub last_probe_age:   u32,
}

impl CpuidLbrState {
    const fn new() -> Self {
        CpuidLbrState {
            lbr_depth:        0,
            lbr_call_stack:   0,
            lbr_mispredict:   0,
            lbr_richness_ema: 0,
            raw_eax:          0,
            raw_ebx:          0,
            raw_ecx:          0,
            lbr_supported:    false,
            lbr_filtering:    false,
            last_probe_age:   0,
        }
    }
}

static STATE: Mutex<CpuidLbrState> = Mutex::new(CpuidLbrState::new());

// ── CPUID helpers ─────────────────────────────────────────────────────────────

/// Query CPUID leaf 0 to get the maximum supported standard leaf.
#[inline(always)]
unsafe fn cpuid_max_leaf() -> u32 {
    let eax_out: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        inout("eax") 0u32 => eax_out,
        in("ecx") 0u32,
        lateout("ecx") _,
        lateout("edx") _,
        options(nostack, preserves_flags),
    );
    eax_out
}

/// Query CPUID leaf 7 sub-leaf 0 and return ECX.
/// Uses push/pop rbx pattern to preserve the register across the call.
#[inline(always)]
unsafe fn cpuid_leaf7_ecx() -> u32 {
    let ecx_out: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        inout("eax") 7u32 => _,
        in("ecx") 0u32,
        lateout("ecx") ecx_out,
        lateout("edx") _,
        options(nostack, preserves_flags),
    );
    ecx_out
}

/// Query CPUID leaf 0x1C sub-leaf 0.
/// Returns (eax, ebx, ecx). Uses push rbx / mov esi, ebx / pop rbx to safely
/// capture EBX without violating the register reservation rules.
#[inline(always)]
unsafe fn cpuid_leaf_1c() -> (u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;  // captured via esi
    let ecx_out: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "mov esi, ebx",
        "pop rbx",
        inout("eax") 0x1Cu32 => eax_out,
        in("ecx") 0u32,
        lateout("ecx") ecx_out,
        lateout("edx") _,
        out("esi") ebx_out,
        options(nostack, preserves_flags),
    );
    (eax_out, ebx_out, ecx_out)
}

// ── Signal math ───────────────────────────────────────────────────────────────

/// Count the number of set bits in a u32 (population count).
#[inline(always)]
fn popcount32(v: u32) -> u32 {
    let mut x = v;
    x = x - ((x >> 1) & 0x5555_5555);
    x = (x & 0x3333_3333) + ((x >> 2) & 0x3333_3333);
    x = (x + (x >> 4)) & 0x0f0f_0f0f;
    x = x.wrapping_mul(0x0101_0101) >> 24;
    x
}

/// EMA: (old * 7 + new_val) / 8, computed in u32 then cast to u16.
#[inline(always)]
fn ema_u16(old: u16, new_val: u16) -> u16 {
    let blended: u32 = ((old as u32) * 7 + (new_val as u32)) / 8;
    if blended > 1000 { 1000u16 } else { blended as u16 }
}

/// Scale a raw 0-255 LBR depth to 0-1000 range by multiplying by 3, cap 1000.
#[inline(always)]
fn scale_depth(raw: u32) -> u16 {
    let v = (raw & 0xFF) * 3;
    if v > 1000 { 1000u16 } else { v as u16 }
}

/// Convert a 0/1 flag bit to 0 or 1000.
#[inline(always)]
fn flag_to_signal(bit: u32) -> u16 {
    if bit != 0 { 1000u16 } else { 0u16 }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // Sample gate: probe only every 5000 ticks
    if age % 5000 != 0 {
        return;
    }

    let mut s = STATE.lock();

    // ── Guard 1: check max supported CPUID leaf ────────────────────────────────
    let max_leaf = unsafe { cpuid_max_leaf() };
    if max_leaf < 0x1C {
        serial_println!(
            "[lbr] tick={} CPUID max_leaf=0x{:X} < 0x1C — ArchLBR not supported on this CPU",
            age,
            max_leaf
        );
        s.lbr_supported    = false;
        s.lbr_depth        = 0;
        s.lbr_call_stack   = 0;
        s.lbr_mispredict   = 0;
        s.last_probe_age   = age;
        return;
    }

    // ── Guard 2: check leaf 7 ECX bit 6 (ArchLBR support flag) ───────────────
    let leaf7_ecx = unsafe { cpuid_leaf7_ecx() };
    let arch_lbr_bit = (leaf7_ecx >> 6) & 1;
    if arch_lbr_bit == 0 {
        serial_println!(
            "[lbr] tick={} CPUID leaf7 ECX=0x{:08X} — bit6 (ArchLBR) clear, no LBR support",
            age,
            leaf7_ecx
        );
        s.lbr_supported    = false;
        s.lbr_depth        = 0;
        s.lbr_call_stack   = 0;
        s.lbr_mispredict   = 0;
        s.last_probe_age   = age;
        return;
    }

    // ── Query leaf 0x1C sub-leaf 0 ────────────────────────────────────────────
    let (eax, ebx, ecx) = unsafe { cpuid_leaf_1c() };

    s.raw_eax = eax;
    s.raw_ebx = ebx;
    s.raw_ecx = ecx;
    s.lbr_supported = true;

    // ── Signal: lbr_depth — bits 7:0 of EAX, scaled *3 capped 1000 ───────────
    let depth_raw = eax & 0xFF;
    let depth_signal = scale_depth(depth_raw);
    s.lbr_depth = depth_signal;

    // ── Signal: lbr_filtering — EBX bit 0 ────────────────────────────────────
    s.lbr_filtering = (ebx & 1) != 0;

    // ── Signal: lbr_mispredict — EBX bit 1 → 0 or 1000 ──────────────────────
    let mispredict_signal = flag_to_signal((ebx >> 1) & 1);
    s.lbr_mispredict = mispredict_signal;

    // ── Signal: lbr_call_stack — EBX bit 2 → 0 or 1000 ──────────────────────
    let call_stack_signal = flag_to_signal((ebx >> 2) & 1);
    s.lbr_call_stack = call_stack_signal;

    // ── Signal: lbr_richness_ema ──────────────────────────────────────────────
    // Components (all u16 0-1000, divided before summing to avoid overflow):
    //   depth contribution    = lbr_depth / 4
    //   call_stack contrib    = lbr_call_stack / 4
    //   mispredict contrib    = lbr_mispredict / 4
    //   ecx richness          = popcount(ecx) * 62  (popcount max=32, 32*62=1984 → cap 1000)
    let depth_contrib:     u32 = (depth_signal as u32) / 4;
    let call_contrib:      u32 = (call_stack_signal as u32) / 4;
    let mispredict_contrib: u32 = (mispredict_signal as u32) / 4;
    let ecx_richness:      u32 = popcount32(ecx).wrapping_mul(62);
    let ecx_contrib:       u32 = if ecx_richness > 1000 { 1000 } else { ecx_richness };

    let raw_richness_u32 = depth_contrib
        .saturating_add(call_contrib)
        .saturating_add(mispredict_contrib)
        .saturating_add(ecx_contrib);

    let raw_richness: u16 = if raw_richness_u32 > 1000 { 1000u16 } else { raw_richness_u32 as u16 };
    s.lbr_richness_ema = ema_u16(s.lbr_richness_ema, raw_richness);

    s.last_probe_age = age;

    serial_println!(
        "[lbr] tick={} depth_raw={} depth={} call_stack={} mispredict={} \
         filtering={} ecx=0x{:08X} ecx_pop={} richness_raw={} richness_ema={}",
        age,
        depth_raw,
        depth_signal,
        call_stack_signal,
        mispredict_signal,
        s.lbr_filtering as u8,
        ecx,
        popcount32(ecx),
        raw_richness,
        s.lbr_richness_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn lbr_depth()        -> u16  { STATE.lock().lbr_depth }
pub fn lbr_call_stack()   -> u16  { STATE.lock().lbr_call_stack }
pub fn lbr_mispredict()   -> u16  { STATE.lock().lbr_mispredict }
pub fn lbr_richness_ema() -> u16  { STATE.lock().lbr_richness_ema }
pub fn lbr_supported()    -> bool { STATE.lock().lbr_supported }
pub fn lbr_filtering()    -> bool { STATE.lock().lbr_filtering }
pub fn raw_eax()          -> u32  { STATE.lock().raw_eax }
pub fn raw_ebx()          -> u32  { STATE.lock().raw_ebx }
pub fn raw_ecx()          -> u32  { STATE.lock().raw_ecx }
pub fn last_probe_age()   -> u32  { STATE.lock().last_probe_age }
