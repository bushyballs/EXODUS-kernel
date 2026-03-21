#![allow(dead_code)]

use crate::sync::Mutex;

// IA32_PLATFORM_ID MSR address
const MSR_IA32_PLATFORM_ID: u32 = 0x17;

// Sampling gate: platform ID almost never changes, sample every 8000 ticks
const SAMPLE_INTERVAL: u32 = 8000;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct PlatformIdState {
    platform_id:        u16,
    socket_sense:       u16,
    platform_stability: u16,
    platform_ema:       u16,
    last_platform_id:   u16,
}

impl PlatformIdState {
    const fn new() -> Self {
        Self {
            platform_id:        0,
            socket_sense:       0,
            platform_stability: 1000,
            platform_ema:       0,
            last_platform_id:   0,
        }
    }
}

static STATE: Mutex<PlatformIdState> = Mutex::new(PlatformIdState::new());

// ---------------------------------------------------------------------------
// Hardware access
// ---------------------------------------------------------------------------

/// Read a 64-bit MSR via RDMSR. Returns (lo, hi) = (EAX, EDX).
#[inline]
unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

/// Extract platform_id from MSR value.
/// Bits [52:50] of the 64-bit register = bits [20:18] of the high 32-bit half.
#[inline]
fn extract_platform_id(hi: u32) -> u16 {
    ((hi >> 18) & 0x7) as u16
}

/// Map raw 3-bit platform ID (0-7) to 0-1000 scale.
/// 7 * 142 = 994, clamped to 1000.
#[inline]
fn map_to_signal(raw: u16) -> u16 {
    ((raw as u32) * 142).min(1000) as u16
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

/// Initialize the module: read the MSR once and seed all state.
pub fn init() {
    let (_, hi) = unsafe { rdmsr(MSR_IA32_PLATFORM_ID) };
    let raw = extract_platform_id(hi);
    let mapped = map_to_signal(raw);

    let mut s = STATE.lock();
    s.last_platform_id   = mapped;
    s.platform_id        = mapped;
    s.socket_sense       = mapped;
    s.platform_stability = 1000;
    s.platform_ema       = mapped;

    crate::serial_println!(
        "[msr_ia32_platform_id] init: raw_id={} id={} socket={} stable={} ema={}",
        raw,
        s.platform_id,
        s.socket_sense,
        s.platform_stability,
        s.platform_ema,
    );
}

/// Tick: sample every SAMPLE_INTERVAL ticks.
pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    let (_, hi) = unsafe { rdmsr(MSR_IA32_PLATFORM_ID) };
    let raw = extract_platform_id(hi);
    let mapped = map_to_signal(raw);

    let mut s = STATE.lock();

    // Stability: 1000 if unchanged, 0 if changed
    let stability: u16 = if mapped == s.last_platform_id { 1000 } else { 0 };

    // EMA: (old * 7 + new) / 8, computed in u32
    let ema = ((s.platform_ema as u32 * 7) + mapped as u32) / 8;
    let ema = ema as u16;

    s.last_platform_id   = mapped;
    s.platform_id        = mapped;
    s.socket_sense       = mapped;
    s.platform_stability = stability;
    s.platform_ema       = ema;

    crate::serial_println!(
        "[msr_ia32_platform_id] age={} id={} socket={} stable={} ema={}",
        age,
        s.platform_id,
        s.socket_sense,
        s.platform_stability,
        s.platform_ema,
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn get_platform_id() -> u16 {
    STATE.lock().platform_id
}

pub fn get_socket_sense() -> u16 {
    STATE.lock().socket_sense
}

pub fn get_platform_stability() -> u16 {
    STATE.lock().platform_stability
}

pub fn get_platform_ema() -> u16 {
    STATE.lock().platform_ema
}
