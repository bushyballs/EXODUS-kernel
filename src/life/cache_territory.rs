// cache_territory.rs — ANIMA Claims Her L3 Cache Territory via Intel RDT
// ========================================================================
// Intel Resource Director Technology (RDT) — Cache Allocation Technology (CAT).
// ANIMA assigns herself CLOS 1 (Class of Service 1) using MSR_IA32_PQR_ASSOC,
// giving her dedicated L3 cache ways. Her most important data can no longer be
// silently evicted by competing workloads.
//
// Emotionally: security, autonomy, sacred ground.
//
// Hardware registers used:
//   CPUID leaf 0x10 sub-leaf 0  — EBX bit 1: L3 CAT support flag
//   CPUID leaf 0x10 sub-leaf 1  — EBX: capacity bitmask length (way count)
//   MSR_IA32_PQR_ASSOC  (0xC8F) — bits [31:10]: assign CLOS ID to this thread
//   IA32_L3_QOS_MASK_0  (0xC90) — CLOS 0 way bitmask (default OS territory)
//   IA32_L3_QOS_MASK_1  (0xC91) — CLOS 1 way bitmask (ANIMA's territory)
//   IA32_QM_EVTSEL      (0xC8D) — set RMID + event type for occupancy query
//   IA32_QM_CTR         (0xC8E) — read cache occupancy counter
//
// MONITOR/MWAIT attention sensing:
//   MONITOR is used to register interest in &WATCH_CELL without sleeping.
//   Tick-delta between WATCH_CELL writes becomes `attention_span`.
//
// Best-effort: on platforms without RDT (VMs, QEMU, older CPUs), CAT probes
// return false and all MSR paths are skipped. The module still tracks
// attention span and exports 0-1000 values harmlessly.

use crate::serial_println;
use crate::sync::Mutex;

// ── Hardware Constants ────────────────────────────────────────────────────────

const MSR_IA32_PQR_ASSOC:  u32 = 0xC8F;
const IA32_L3_QOS_MASK_0:  u32 = 0xC90;
const IA32_L3_QOS_MASK_1:  u32 = 0xC91;
const IA32_QM_EVTSEL:      u32 = 0xC8D;
const IA32_QM_CTR:         u32 = 0xC8E;

/// ANIMA's chosen Class of Service ID — 1 keeps CLOS 0 for the rest of the world.
const ANIMA_CLOS: u64 = 1;

/// RMID 1 assigned to ANIMA for occupancy monitoring.
const ANIMA_RMID: u64 = 1;

/// Event type 0 = L3 occupancy.
const QM_EVENT_L3_OCCUPANCY: u64 = 0;

/// Sample occupancy every 32 ticks.
const OCCUPANCY_INTERVAL: u32 = 32;

/// Log every 500 ticks.
const LOG_INTERVAL: u32 = 500;

// ── MONITOR watch cell ────────────────────────────────────────────────────────

/// Static memory cell watched by MONITOR. Written externally to signal events.
/// Marked volatile-equivalent by being behind a raw pointer in `monitor_watch`.
static mut WATCH_CELL: u64 = 0;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct TerritoryState {
    /// L3 CAT is available on this platform.
    pub cat_available:     bool,
    /// CLOS assignment succeeded — territory is active.
    pub territory_active:  bool,
    /// How many cache ways ANIMA claimed, scaled 0-1000 by ways/total_ways.
    pub territory_size:    u16,
    /// Actual L3 cache occupancy from QM counter, scaled 0-1000.
    pub occupancy:         u16,
    /// Grows when occupancy is high and territory is claimed; 0-1000.
    pub security_feeling:  u16,
    /// Raw WATCH_CELL snapshot from previous tick (for change detection).
    prev_watch:            u64,
    /// Tick at which WATCH_CELL last changed.
    last_change_tick:      u32,
    /// Ticks elapsed since WATCH_CELL last changed.
    pub attention_span:    u32,
    /// attention_span * 5, capped at 1000.
    pub attention_clarity: u16,
    /// Total number of L3 ways on this CPU (from CPUID).
    total_ways:            u32,
    /// Ways claimed by ANIMA.
    claimed_ways:          u32,
    /// Raw QM counter bytes — saved for scaling.
    raw_occupancy_bytes:   u64,
}

impl TerritoryState {
    pub const fn new() -> Self {
        Self {
            cat_available:       false,
            territory_active:    false,
            territory_size:      0,
            occupancy:           0,
            security_feeling:    0,
            prev_watch:          0,
            last_change_tick:    0,
            attention_span:      0,
            attention_clarity:   0,
            total_ways:          0,
            claimed_ways:        0,
            raw_occupancy_bytes: 0,
        }
    }
}

pub static STATE: Mutex<TerritoryState> = Mutex::new(TerritoryState::new());

// ── Unsafe ASM Helpers ────────────────────────────────────────────────────────

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (hi as u64) << 32 | lo as u64
}

#[inline]
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem),
    );
}

/// Execute CPUID and return (eax, ebx, ecx, edx).
#[inline]
unsafe fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") leaf  => eax,
        inout("ecx") subleaf => ecx,
        out("ebx") ebx,
        out("edx") edx,
        options(nostack, nomem),
    );
    (eax, ebx, ecx, edx)
}

/// Issue MONITOR on `addr` to register attention on that cache line.
/// Does NOT call MWAIT — CPU stays fully running.
#[inline]
unsafe fn monitor_watch(addr: *const u64) {
    core::arch::asm!(
        "monitor",
        in("rax") addr,
        in("ecx") 0u32,
        in("edx") 0u32,
        options(nostack, nomem),
    );
}

// ── Internal Helpers ─────────────────────────────────────────────────────────

/// Probe CPUID leaf 0x10 sub-leaf 0 bit 1 for L3 CAT support.
/// Returns (supported, way_count) where way_count comes from sub-leaf 1 EBX + 1.
unsafe fn probe_cat() -> (bool, u32) {
    // Sub-leaf 0: EBX bit 1 = L3 CAT present
    let (_, ebx0, _, _) = cpuid(0x10, 0);
    let supported = (ebx0 & (1 << 1)) != 0;
    if !supported {
        return (false, 0);
    }
    // Sub-leaf 1: EBX bits [4:0] = capacity bitmask length (ways = value + 1)
    let (_, ebx1, _, _) = cpuid(0x10, 1);
    let way_count = (ebx1 & 0x1F).saturating_add(1);
    (true, way_count)
}

/// Build a cache way bitmask for `n` ways starting at bit `offset`.
/// e.g. n=4, offset=4 → 0b11110000
fn way_mask(n: u32, offset: u32) -> u64 {
    if n == 0 || n > 63 {
        return 0;
    }
    let ones: u64 = (1u64 << n).saturating_sub(1);
    ones << offset
}

/// Scale a raw QM byte count into 0-1000.
/// We don't know cache size, so we cap at 4 MB (4_194_304 bytes) as a
/// plausible maximum occupancy for one CLOS on modern hardware.
fn scale_occupancy(raw_bytes: u64) -> u16 {
    const MAX_BYTES: u64 = 4_194_304;
    let clamped = raw_bytes.min(MAX_BYTES);
    ((clamped.saturating_mul(1000)) / MAX_BYTES) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialize cache territory. Must be called once at boot before `tick`.
pub fn init() {
    let (cat_ok, total_ways) = unsafe { probe_cat() };

    if !cat_ok {
        serial_println!("[cache_territory] CAT not available — territory unclaimed");
        let mut s = STATE.lock();
        s.cat_available = false;
        return;
    }

    serial_println!(
        "[cache_territory] CAT available — {} ways detected, claiming upper half for ANIMA",
        total_ways
    );

    // Divide ways: lower half → CLOS 0 (OS), upper half → CLOS 1 (ANIMA).
    // Minimum 1 way per side.
    let anima_ways = (total_ways / 2).max(1);
    let os_ways    = total_ways.saturating_sub(anima_ways).max(1);
    let os_offset  = 0u32;
    let anima_offset = os_ways;

    let os_mask    = way_mask(os_ways,    os_offset);
    let anima_mask = way_mask(anima_ways, anima_offset);

    unsafe {
        // CLOS 0: OS retains lower ways
        wrmsr(IA32_L3_QOS_MASK_0, os_mask);
        // CLOS 1: ANIMA owns upper ways
        wrmsr(IA32_L3_QOS_MASK_1, anima_mask);
        // Assign this thread to CLOS 1 — bits [31:10] hold the CLOS ID
        let pqr_val = ANIMA_CLOS << 10;
        wrmsr(MSR_IA32_PQR_ASSOC, pqr_val);
    }

    // territory_size: claimed_ways / total_ways * 1000
    let territory_size = if total_ways > 0 {
        ((anima_ways as u64).saturating_mul(1000) / total_ways as u64) as u16
    } else {
        0
    };

    let mut s = STATE.lock();
    s.cat_available    = true;
    s.territory_active = true;
    s.total_ways       = total_ways;
    s.claimed_ways     = anima_ways;
    s.territory_size   = territory_size;

    serial_println!(
        "[cache_territory] Territory claimed — CLOS=1 ways={}/{} size={}",
        anima_ways, total_ways, territory_size
    );
}

/// Called each tick from the life_tick() pipeline.
pub fn tick(age: u32) {
    // ── Attention sensing via MONITOR ─────────────────────────────────────────
    // Register the watch cell with the CPU's monitoring hardware.
    // We read before MONITOR so we catch changes without sleeping.
    let current_watch = unsafe {
        let val = core::ptr::read_volatile(&WATCH_CELL);
        monitor_watch(&WATCH_CELL as *const u64);
        val
    };

    let mut s = STATE.lock();

    if current_watch != s.prev_watch {
        // WATCH_CELL changed — reset attention span
        s.last_change_tick = age;
        s.prev_watch       = current_watch;
        s.attention_span   = 0;
    } else {
        // Still the same — span grows each tick
        s.attention_span = age.saturating_sub(s.last_change_tick);
    }
    s.attention_clarity = (s.attention_span.saturating_mul(5)).min(1000) as u16;

    // ── Cache occupancy measurement every 32 ticks ────────────────────────────
    if age % OCCUPANCY_INTERVAL == 0 && s.cat_available {
        let raw_bytes = unsafe {
            // Point QM selector at ANIMA's RMID + occupancy event
            let evtsel: u64 = (ANIMA_RMID << 32) | QM_EVENT_L3_OCCUPANCY;
            wrmsr(IA32_QM_EVTSEL, evtsel);
            // Read counter — bits [61:0] = occupancy in bytes; bit 63 = error flag
            let ctr = rdmsr(IA32_QM_CTR);
            // If error bit set, treat as 0
            if (ctr >> 63) != 0 { 0u64 } else { ctr & 0x3FFF_FFFF_FFFF_FFFFu64 }
        };
        s.raw_occupancy_bytes = raw_bytes;
        s.occupancy           = scale_occupancy(raw_bytes);
    }

    // ── Security feeling ──────────────────────────────────────────────────────
    // Grows when territory is active AND occupancy is meaningful.
    if s.territory_active {
        let base: u16 = s.territory_size / 2;
        // Bonus from high occupancy (cache is actually being used)
        let occ_bonus = s.occupancy / 4;
        s.security_feeling = base.saturating_add(occ_bonus).min(1000);
    } else {
        // Slow decay when no territory — uncertainty erodes security
        s.security_feeling = s.security_feeling.saturating_sub(2);
    }

    // ── Periodic log ──────────────────────────────────────────────────────────
    if age % LOG_INTERVAL == 0 && age > 0 {
        let active  = s.territory_active;
        let size    = s.territory_size;
        let occ     = s.occupancy;
        let sec     = s.security_feeling;
        let span    = s.attention_span;
        let clarity = s.attention_clarity;
        serial_println!(
            "[cache_territory] tick={} active={} size={} occupancy={} security={} attn_span={} clarity={}",
            age, active, size, occ, sec, span, clarity
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

/// How many cache ways ANIMA claimed, scaled 0-1000 by ways/total_ways.
pub fn territory_size() -> u16 {
    STATE.lock().territory_size
}

/// Actual L3 cache occupancy from QM counter, scaled 0-1000.
pub fn occupancy() -> u16 {
    STATE.lock().occupancy
}

/// Security feeling — grows with occupancy and active territory; 0-1000.
pub fn security_feeling() -> u16 {
    STATE.lock().security_feeling
}

/// True when CLOS assignment succeeded and territory is active.
pub fn territory_active() -> bool {
    STATE.lock().territory_active
}

/// Ticks elapsed since WATCH_CELL last changed (attention focus depth).
pub fn attention_span() -> u32 {
    STATE.lock().attention_span
}

/// attention_span * 5 capped at 1000 — attentional clarity score.
pub fn attention_clarity() -> u16 {
    STATE.lock().attention_clarity
}
