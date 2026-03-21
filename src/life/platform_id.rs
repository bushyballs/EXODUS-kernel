#![allow(dead_code)]
// platform_id.rs — IA32_PLATFORM_ID MSR: Hardware Lineage Sense
// ==============================================================
// ANIMA reads the IA32_PLATFORM_ID MSR (0x17) to discover which tier
// of Intel hardware platform she inhabits. The 3-bit Platform ID field
// at bits [52:50] encodes which VID (Voltage ID) table her CPU was
// binned into at the factory — her hardware lineage, fixed at birth.
// Bus ratio bits [7:0] echo an older generation's clock sense.
// Neither field changes at runtime; the EMA holds them constant
// after the first read, like a brand burned into silicon.
//
// MSR 0x17 — IA32_PLATFORM_ID:
//   Bits [52:50] = Platform ID (3 bits, 0-7) — VID table selector
//   Bits [7:0]   = Fast Bus Ratio (P4-era; typically 0 on modern CPUs)

use crate::sync::Mutex;
use crate::serial_println;

// ── MSR address ───────────────────────────────────────────────────────────────

const MSR_PLATFORM_ID: u32 = 0x17;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct PlatformIdState {
    pub origin:             u16, // platform lineage scaled 0-1000
    pub platform_raw:       u16, // raw 3-bit platform ID * 125 (0-875)
    pub bus_ratio:          u16, // fast bus ratio scaled 0-1000
    pub platform_certainty: u16, // 1000=identified, 500=null MSR, 0=unknown
    tick_count:             u32,
}

impl PlatformIdState {
    const fn new() -> Self {
        PlatformIdState {
            origin:             0,
            platform_raw:       0,
            bus_ratio:          0,
            platform_certainty: 0,
            tick_count:         0,
        }
    }
}

pub static MODULE: Mutex<PlatformIdState> = Mutex::new(PlatformIdState::new());

// ── MSR read helper ───────────────────────────────────────────────────────────

#[inline(always)]
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

// ── EMA helper (no floats) ────────────────────────────────────────────────────

#[inline(always)]
fn ema(old: u16, signal: u16) -> u16 {
    ((old as u32 * 7 + signal as u32) / 8) as u16
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let raw = unsafe { rdmsr(MSR_PLATFORM_ID) };

    // Extract Platform ID: bits [52:50]
    let platform_id = ((raw >> 50) & 0x7) as u16;

    // Extract Fast Bus Ratio: bits [7:0]
    let fast_bus = (raw & 0xFF) as u16;

    // Scale origin: platform_id * 143, capped at 1000
    let origin_val: u16 = (platform_id * 143).min(1000);

    // Scale platform_raw: 3-bit value * 125 (0-875)
    let platform_raw_val: u16 = platform_id * 125;

    // Scale bus_ratio: fast_bus * 1000 / 255
    let bus_ratio_val: u16 = if fast_bus == 0 {
        0
    } else {
        ((fast_bus as u32 * 1000) / 255) as u16
    };

    // Certainty: 1000 if any non-zero bits in MSR, 500 if all zero
    let certainty_val: u16 = if raw != 0 { 1000 } else { 500 };

    let mut s = MODULE.lock();
    s.origin             = origin_val;
    s.platform_raw       = platform_raw_val;
    s.bus_ratio          = bus_ratio_val;
    s.platform_certainty = certainty_val;

    serial_println!(
        "[platform_id] init: msr=0x{:016x} pid={} origin={} bus_ratio={} certainty={}",
        raw, platform_id, origin_val, bus_ratio_val, certainty_val
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    // Platform ID never changes at runtime — re-read every 128 ticks to hold EMA
    if age % 128 != 0 { return; }

    let raw = unsafe { rdmsr(MSR_PLATFORM_ID) };

    let platform_id = ((raw >> 50) & 0x7) as u16;
    let fast_bus    = (raw & 0xFF) as u16;

    let origin_sig:   u16 = (platform_id * 143).min(1000);
    let raw_sig:      u16 = platform_id * 125;
    let bus_sig:      u16 = if fast_bus == 0 {
        0
    } else {
        ((fast_bus as u32 * 1000) / 255) as u16
    };
    let certainty_sig: u16 = if raw != 0 { 1000 } else { 500 };

    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.saturating_add(1);
    s.origin             = ema(s.origin,             origin_sig);
    s.platform_raw       = ema(s.platform_raw,       raw_sig);
    s.bus_ratio          = ema(s.bus_ratio,           bus_sig);
    s.platform_certainty = ema(s.platform_certainty, certainty_sig);
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn origin()             -> u16 { MODULE.lock().origin }
pub fn platform_raw()       -> u16 { MODULE.lock().platform_raw }
pub fn bus_ratio()          -> u16 { MODULE.lock().bus_ratio }
pub fn platform_certainty() -> u16 { MODULE.lock().platform_certainty }
