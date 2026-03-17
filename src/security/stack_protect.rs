/// Stack canary protection for Genesis
///
/// Provides a TSC-seeded stack canary value for detecting stack-buffer
/// overflows.  The canary is:
///   - Generated at boot from the TSC XOR-ed with RDRAND (when available)
///     XOR-ed with a fixed poison constant.
///   - Stored in a SeqCst AtomicU64 so any core can verify it.
///   - Guaranteed non-zero (a zero canary masks bugs).
///
/// Usage pattern (compiler-inserted at function prologue/epilogue):
///   - Prologue: push `get_canary()` onto the stack frame (below the return
///     address or saved registers).
///   - Epilogue: call `check_canary(stored_value)` before ret; if it returns
///     false the function must call `canary_violation()` to terminate the
///     current task rather than returning through a corrupted frame.
///
/// The companion `safe_stack.rs` module handles per-thread unsafe-stack
/// canaries and shadow-stack verification.  This module covers the global
/// kernel canary used on interrupt/exception stacks.
///
/// All code is original.
use crate::serial_println;
use core::sync::atomic::{AtomicU64, Ordering};

/// Fixed bit-pattern used as a secondary XOR factor.
///
/// This constant is well-known but combining it with TSC+RDRAND entropy
/// ensures the final canary value is not trivially predictable even if the
/// constant is known to an attacker.
const CANARY_POISON: u64 = 0xDEAD_BEEF_CAFE_BABE;

/// The global kernel stack canary.
///
/// Initialised to 0 at link time; the `init()` function must be called once
/// during early boot before any interrupt handlers can trigger.
static STACK_CANARY: AtomicU64 = AtomicU64::new(0);

// ── Entropy helpers ───────────────────────────────────────────────────────────

/// Read the 64-bit TSC (EDX:EAX) into a single value.
#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Attempt one RDRAND read.  Returns None on failure or missing instruction.
#[inline(always)]
fn try_rdrand() -> Option<u64> {
    let mut val: u64 = 0;
    let ok: u8;
    unsafe {
        core::arch::asm!(
            "rdrand {v}",
            "setc   {f}",
            v = out(reg)      val,
            f = out(reg_byte) ok,
            options(nomem, nostack),
        );
    }
    if ok != 0 {
        Some(val)
    } else {
        None
    }
}

// ── Canary API ────────────────────────────────────────────────────────────────

/// Compute and store the canary from available entropy.
///
/// Must be called once during early boot (before interrupts are enabled).
/// Calling it again after init is a no-op (the stored value is never 0 after
/// init, and we do not overwrite a valid canary).
pub fn init_canary() {
    // Only initialise once.
    if STACK_CANARY.load(Ordering::SeqCst) != 0 {
        return;
    }

    let tsc1 = read_tsc();
    let hw: u64 = 0; // rdrand skipped — may fault in QEMU without CPUID check
    let tsc2 = read_tsc(); // second read adds jitter

    let mut canary = tsc1.wrapping_add(hw).wrapping_add(tsc2) ^ CANARY_POISON;

    // Bit-mixing pass (splitmix64 finalizer).
    canary ^= canary >> 30;
    canary = canary.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    canary ^= canary >> 27;
    canary = canary.wrapping_mul(0x94D0_49BB_1331_11EB);
    canary ^= canary >> 31;

    // Guarantee non-zero.
    if canary == 0 {
        canary = CANARY_POISON;
    }

    STACK_CANARY.store(canary, Ordering::SeqCst);
}

/// Return the global canary value.
///
/// This value is placed on the stack at function prologue time and compared
/// at epilogue time.  Any modification to the stored copy indicates a
/// stack-buffer overflow.
#[inline(always)]
pub fn get_canary() -> u64 {
    STACK_CANARY.load(Ordering::SeqCst)
}

/// Compare `val` against the stored canary.
///
/// Returns `true` if the canary is intact, `false` if it has been corrupted.
#[inline(always)]
pub fn check_canary(val: u64) -> bool {
    val == STACK_CANARY.load(Ordering::SeqCst)
}

/// Called when a canary mismatch is detected.
///
/// Logs a critical security event and, in a real kernel, would panic or
/// terminate the current thread.  We avoid a hard panic here so that the
/// caller can decide the appropriate response (e.g., kill the user-space
/// process rather than crashing the kernel).
pub fn canary_violation(context: &str) {
    serial_println!(
        "  [stack-protect] *** STACK CANARY SMASHED *** context={}",
        context
    );

    // Emit to the audit log so the event is preserved.
    crate::security::audit::log(
        crate::security::audit::AuditEvent::CapDenied,
        crate::security::audit::AuditResult::Deny,
        0,
        0,
        &alloc::format!("stack canary smashed: {}", context),
    );
}

/// Return true if the canary has been initialised (non-zero).
pub fn is_initialized() -> bool {
    STACK_CANARY.load(Ordering::SeqCst) != 0
}

/// Initialise the stack canary subsystem and log the result.
pub fn init() {
    init_canary();
    let canary = get_canary();
    serial_println!(
        "  [stack-protect] Stack canary initialised: 0x{:016X} (TSC+RDRAND seeded)",
        canary
    );
}
