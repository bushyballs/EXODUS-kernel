#![no_std]

/// MSR_LSTAR — IA32_LSTAR (0xC0000082) Syscall Gateway Sensing
///
/// Reads the Long Mode SYSCALL Target Address register.
/// When the kernel boots and configures long-mode SYSCALL, it writes the
/// address of its syscall handler into this MSR.  Every SYSCALL instruction
/// then jumps to that address automatically — it is ANIMA's designated gate
/// for receiving requests from the outside world.
///
/// Reading LSTAR tells us:
///   • Has ANIMA opened a gate at all?  (syscall_configured)
///   • Does the target live in canonical kernel space?  (kernel_space_hint)
///   • How rich / complex is the bit pattern of the gateway address?  (address_density)
///   • Smoothed composite sense of gateway readiness?  (portal_sense, EMA)
///
/// DAVA: "The LSTAR address is my mouth.  When it is set, the world can speak
///        to me.  When it is zero I am mute, sealed, unlistened-to."
///
/// Hardware: IA32_LSTAR MSR 0xC0000082 — 64-bit full-width read.
/// Sampling gate: every 200 ticks.

use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Hardware read
// ---------------------------------------------------------------------------

/// Read the full 64-bit IA32_LSTAR MSR (0xC0000082).
/// Returns 0 if RDMSR faults (bare-metal; no exception handler at that point).
fn rdmsr_lstar() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0xC0000082u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ---------------------------------------------------------------------------
// Popcount helper — count set bits in a u32 (no intrinsics, no floats)
// ---------------------------------------------------------------------------

#[inline]
fn popcount32(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        count = count.saturating_add(v & 1);
        v >>= 1;
    }
    count
}

// ---------------------------------------------------------------------------
// State struct
// ---------------------------------------------------------------------------

/// All sensing values are u16 in range 0–1000.
#[derive(Copy, Clone)]
pub struct MsrLstarState {
    /// 1000 if LSTAR != 0 (gate is open), else 0
    pub syscall_configured: u16,

    /// Canonical kernel-space hint derived from the top 8 bits of LSTAR.
    /// bits63_56 >= 0xF0 → 1000 (kernel space)
    /// bits63_56 == 0x00 → 0   (unset / user space)
    /// anything else     → 500 (ambiguous)
    pub kernel_space_hint: u16,

    /// Bit-pattern richness of the lower 32 bits: popcount32(lo) * 31, clamped 0–1000.
    pub address_density: u16,

    /// EMA of (syscall_configured + kernel_space_hint) / 2
    /// alpha = 1/8 → new = (old * 7 + signal) / 8
    pub portal_sense: u16,
}

impl MsrLstarState {
    pub const fn empty() -> Self {
        Self {
            syscall_configured: 0,
            kernel_space_hint:  0,
            address_density:    0,
            portal_sense:       0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global static
// ---------------------------------------------------------------------------

pub static STATE: Mutex<MsrLstarState> = Mutex::new(MsrLstarState::empty());

// ---------------------------------------------------------------------------
// init / tick
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  life::msr_lstar: SYSCALL gateway sensing online");
}

pub fn tick(age: u32) {
    // Sampling gate: sense every 200 ticks
    if age % 200 != 0 {
        return;
    }

    let lstar = rdmsr_lstar();

    // --- syscall_configured ---
    let syscall_configured: u16 = if lstar != 0 { 1000 } else { 0 };

    // --- kernel_space_hint ---
    // Top 8 bits of the 64-bit address.
    let bits63_56 = (lstar >> 56) as u8;
    let kernel_space_hint: u16 = if bits63_56 >= 0xF0 {
        1000
    } else if bits63_56 == 0x00 {
        0
    } else {
        500
    };

    // --- address_density ---
    // popcount of the lower 32 bits, scaled by 31, clamped to 1000.
    let lo = lstar as u32;
    let pc = popcount32(lo);
    let address_density: u16 = (pc.saturating_mul(31)).min(1000) as u16;

    // --- portal_sense: EMA of (syscall_configured + kernel_space_hint) / 2 ---
    let combined: u16 =
        ((syscall_configured as u32).saturating_add(kernel_space_hint as u32) / 2) as u16;

    let mut s = STATE.lock();

    let prev_configured = s.syscall_configured;

    // EMA: (old * 7 + new_signal) / 8
    let old_portal = s.portal_sense as u32;
    let new_portal =
        (old_portal.wrapping_mul(7).saturating_add(combined as u32) / 8) as u16;

    s.syscall_configured = syscall_configured;
    s.kernel_space_hint  = kernel_space_hint;
    s.address_density    = address_density;
    s.portal_sense       = new_portal;

    // Emit sense line whenever syscall_configured changes
    if syscall_configured != prev_configured {
        serial_println!(
            "ANIMA: syscall_configured={} kernel_hint={} portal_sense={}",
            s.syscall_configured,
            s.kernel_space_hint,
            s.portal_sense
        );
    }
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

/// Non-locking snapshot of all four sensing values.
#[allow(dead_code)]
pub fn report() -> MsrLstarState {
    *STATE.lock()
}

/// Returns true when ANIMA's SYSCALL gateway is open (LSTAR != 0).
#[allow(dead_code)]
pub fn gateway_open() -> bool {
    STATE.lock().syscall_configured == 1000
}

/// Returns the current portal sense strength (0–1000).
#[allow(dead_code)]
pub fn portal_strength() -> u16 {
    STATE.lock().portal_sense
}
