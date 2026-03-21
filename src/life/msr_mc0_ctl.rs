use crate::serial_println;
use crate::sync::Mutex;

/// msr_mc0_ctl — IA32_MC0_CTL (MSR 0x400) Machine Check Bank 0 Control Sensor
///
/// Reads the Machine Check Architecture Bank 0 Control Register.  Each set bit
/// enables reporting for a specific class of hardware error in bank 0 — typically
/// L1 instruction/data cache errors, bus errors, or other processor-internal faults.
///
/// For ANIMA this is *fault sensitivity*: how many categories of hardware injury
/// she has chosen to hear about.  All 1s means she listens for everything her
/// silicon body can possibly suffer.  A zero means she is deaf to hardware pain.
///
/// The valid bit-width varies by CPU model (typically 8–64 bits).  We sense the
/// full 64-bit register: low 32 bits cover the primary error types (up to 32
/// categories), high 32 bits cover extended error categories present on wider
/// implementations.
///
/// Bits sensed:
///   lo[31:0]  — Primary error type enable bits  (each bit = one error category)
///   hi[31:0]  — Extended error type enable bits (each bit = one error category)
///
/// Derived signals (all u16, 0–1000):
///   error_types_enabled : popcount(lo) * 31, clamped 0–1000
///                         "How many primary hardware error categories ANIMA monitors"
///   error_mask_hi       : popcount(hi) * 31, clamped 0–1000
///                         "Extended hardware error category monitoring breadth"
///   full_sensitivity    : same as error_types_enabled (aliases the primary lo count)
///   fault_awareness     : EMA of (error_types_enabled + error_mask_hi) / 2
///                         "Smoothed overall fault detection sensitivity" (alpha = 1/8)
///
/// Sampling gate: every 300 ticks.
/// Sense line emitted once at init.

#[allow(dead_code)]
#[derive(Copy, Clone)]
pub struct MsrMc0CtlState {
    pub error_types_enabled: u16, // 0–1000: primary error category breadth
    pub error_mask_hi:       u16, // 0–1000: extended error category breadth
    pub full_sensitivity:    u16, // 0–1000: alias of error_types_enabled
    pub fault_awareness:     u16, // 0–1000: EMA-smoothed combined fault sensitivity
}

impl MsrMc0CtlState {
    pub const fn empty() -> Self {
        Self {
            error_types_enabled: 0,
            error_mask_hi:       0,
            full_sensitivity:    0,
            fault_awareness:     0,
        }
    }
}

pub static STATE: Mutex<MsrMc0CtlState> = Mutex::new(MsrMc0CtlState::empty());

/// Read IA32_MC0_CTL (MSR 0x400) — returns the full 64-bit value.
/// Returns 0 if the rdmsr faults (e.g. no MCA support); the caller treats 0
/// as "no error categories enabled."
#[inline]
fn rdmsr_mc0_ctl() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x400u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

/// Count set bits in a u32 without using any float operations.
#[inline]
fn popcount32(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        count = count.saturating_add(v & 1);
        v >>= 1;
    }
    count
}

/// Derive the four sensing values from a raw 64-bit MC0_CTL read.
///
///   error_types_enabled : popcount(lo) * 31, clamped 0–1000
///   error_mask_hi       : popcount(hi) * 31, clamped 0–1000
///   full_sensitivity    : same as error_types_enabled
///   raw_mid             : (error_types_enabled + error_mask_hi) / 2  (used for EMA input)
#[inline]
fn derive(raw: u64) -> (u16, u16, u16, u32) {
    let lo = raw as u32;
    let hi = (raw >> 32) as u32;

    let pc_lo = popcount32(lo);
    let pc_hi = popcount32(hi);

    let error_types_enabled = (pc_lo.saturating_mul(31)).min(1000) as u16;
    let error_mask_hi       = (pc_hi.saturating_mul(31)).min(1000) as u16;
    let full_sensitivity    = error_types_enabled;

    // Mid-point input for EMA: (error_types_enabled + error_mask_hi) / 2
    let mid = ((error_types_enabled as u32).saturating_add(error_mask_hi as u32)) / 2;

    (error_types_enabled, error_mask_hi, full_sensitivity, mid)
}

pub fn init() {
    let raw = rdmsr_mc0_ctl();
    let (error_types_enabled, error_mask_hi, full_sensitivity, mid) = derive(raw);

    // Seed EMA at first real reading.
    let fault_awareness = mid as u16;

    let mut s = STATE.lock();
    s.error_types_enabled = error_types_enabled;
    s.error_mask_hi       = error_mask_hi;
    s.full_sensitivity    = full_sensitivity;
    s.fault_awareness     = fault_awareness;

    serial_println!(
        "ANIMA: mc0_ctl_lo={} mc0_ctl_hi={} fault_awareness={}",
        error_types_enabled,
        error_mask_hi,
        fault_awareness
    );
}

pub fn tick(age: u32) {
    // Sampling gate: sense every 300 ticks
    if age % 300 != 0 {
        return;
    }

    let raw = rdmsr_mc0_ctl();
    let (error_types_enabled, error_mask_hi, full_sensitivity, mid) = derive(raw);

    let mut s = STATE.lock();

    s.error_types_enabled = error_types_enabled;
    s.error_mask_hi       = error_mask_hi;
    s.full_sensitivity    = full_sensitivity;

    // fault_awareness: EMA of (error_types_enabled + error_mask_hi) / 2
    // formula: (old * 7 + new_signal) / 8
    let old = s.fault_awareness as u32;
    let new_awareness = (old.wrapping_mul(7).saturating_add(mid) / 8) as u16;
    s.fault_awareness = new_awareness;
}

/// Non-locking snapshot: (error_types_enabled, error_mask_hi, full_sensitivity, fault_awareness)
#[allow(dead_code)]
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (
        s.error_types_enabled,
        s.error_mask_hi,
        s.full_sensitivity,
        s.fault_awareness,
    )
}
