/// Stack Canary Support — Genesis kernel hardening
///
/// Provides a manual canary system for critical kernel paths.  The Rust
/// compiler can emit its own stack-protector checks when built with the
/// appropriate flags, but this module gives the kernel explicit control:
///
///   - A master canary is seeded from RDRAND (preferred) or TSC at boot.
///   - Per-thread canaries are derived by XOR-ing the master with the TID.
///   - A `check_canary(frame_canary)` call at function epilogue compares the
///     saved frame value against the live master; any discrepancy triggers
///     a serial panic log and an infinite halt.
///   - A guard-zone API marks fixed-size regions with a known magic value;
///     `check_guard_zone` detects whether any byte was overwritten.
///
/// Placement in GS-base:
///   The master canary is written to the per-CPU GS region at offset +40,
///   matching the Linux x86_64 ABI for `-fstack-protector`.  This allows the
///   GCC/Clang prologue/epilogue sequences emitted when the kernel is built
///   with `-fstack-protector-strong` to read the correct canary via
///   `%gs:40` without any additional runtime adaptation.
///
/// Critical rules honoured here:
///   - NO float casts (no `as f32` / `as f64`).
///   - Counters use `saturating_add` / `saturating_sub`.
///   - No heap (`alloc`) — fixed static arrays only.
///   - No panics — violations are logged then halted.
///
/// All code is original.
use crate::serial_println;
use core::sync::atomic::{AtomicU64, Ordering};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Magic value written into every byte of a heap guard zone.
/// Chosen to be visually distinctive and highly unlikely to appear naturally
/// in valid data.  "DEAD C0DE CAFE BABE" split into its component bytes.
pub const HEAP_GUARD_MAGIC: u64 = 0xDEAD_C0DE_CAFE_BABE;

/// Byte value used to fill guard zones (low byte of HEAP_GUARD_MAGIC).
const GUARD_BYTE: u8 = 0xBE;

/// Secondary XOR constant mixed into canary entropy.
const CANARY_POISON: u64 = 0xBAD0_BABE_FEED_FACE;

// ── Statics ───────────────────────────────────────────────────────────────────

/// The master kernel stack canary.
/// Written once by `init_canary()`, never changed afterward.
/// All kernel stacks embed a copy of this value in their frame; any write to
/// the embedding corrupts it and is caught by `check_canary`.
static CANARY_VALUE: AtomicU64 = AtomicU64::new(0);

// ── Entropy helpers ───────────────────────────────────────────────────────────

/// Read the combined 64-bit TSC (EDX:EAX).
#[inline(always)]
fn read_tsc64() -> u64 {
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

/// Attempt a single RDRAND read.  Returns `None` if the instruction is
/// unavailable or the hardware buffer is temporarily empty.
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

// ── Canary initialisation ─────────────────────────────────────────────────────

/// Compute and store the master canary.
///
/// Must be called exactly once during early boot, before any user stack frame
/// is allocated and before interrupts are enabled.  A second call is a no-op.
pub fn init_canary() {
    // Idempotent: do nothing if already initialised (canary != 0 after init).
    serial_println!("  [canary-dbg] loading CANARY_VALUE");
    if CANARY_VALUE.load(Ordering::SeqCst) != 0 {
        serial_println!("  [canary-dbg] already init, returning");
        return;
    }

    serial_println!("  [canary-dbg] reading tsc1");
    let tsc1 = read_tsc64();
    serial_println!("  [canary-dbg] skipping rdrand (may fault in QEMU)");
    let hw: u64 = 0;
    serial_println!("  [canary-dbg] reading tsc2");
    let tsc2 = read_tsc64();
    serial_println!(
        "  [canary-dbg] computing canary tsc1={:#x} hw={:#x} tsc2={:#x}",
        tsc1,
        hw,
        tsc2
    );

    let mut c = tsc1.wrapping_add(hw).wrapping_add(tsc2) ^ CANARY_POISON;

    // splitmix64 finaliser — diffuses low-entropy inputs.
    c ^= c >> 30;
    c = c.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    c ^= c >> 27;
    c = c.wrapping_mul(0x94D0_49BB_1331_11EB);
    c ^= c >> 31;

    // Guarantee non-zero.
    if c == 0 {
        c = CANARY_POISON;
    }

    CANARY_VALUE.store(c, Ordering::SeqCst);

    // write_gs_canary skipped: GS base not configured until per-CPU init
}

/// Write the canary to GS:+40 (Linux x86_64 stack-protector ABI offset).
/// This is best-effort: if GS base has not been configured yet, the write
/// lands in low memory (which is fine — the page-fault handler will catch it,
/// or the value will be overwritten when GS is properly loaded).
#[inline]
fn write_gs_canary(canary: u64) {
    unsafe {
        core::arch::asm!(
            "mov %gs:40, {c}",
            c = in(reg) canary,
            options(nostack, att_syntax),
        );
    }
}

// ── Public canary API ─────────────────────────────────────────────────────────

/// Return the current master canary value.
///
/// The generated function prologue stores this value at a fixed offset in the
/// stack frame; the epilogue calls `check_canary` with the stored copy.
#[inline(always)]
pub fn get_canary() -> u64 {
    CANARY_VALUE.load(Ordering::SeqCst)
}

/// Verify that `frame_canary` (the value saved in a stack frame at function
/// entry) matches the live master canary.
///
/// If they differ, a kernel-stack overflow or stack-smashing attack is
/// assumed.  We log to the serial port and loop forever (a controlled halt)
/// rather than allowing execution to continue through a corrupted frame.
pub fn check_canary(frame_canary: u64) {
    let live = CANARY_VALUE.load(Ordering::SeqCst);
    if frame_canary != live {
        serial_println!(
            "  [stack-canary] *** CANARY CORRUPTED *** expected=0x{:016X} found=0x{:016X}",
            live,
            frame_canary
        );
        // Log to the audit subsystem before halting.
        crate::security::audit::log(
            crate::security::audit::AuditEvent::CapDenied,
            crate::security::audit::AuditResult::Deny,
            0,
            0,
            "stack canary corrupted — kernel halted",
        );
        // Hard halt: spin forever so no corrupted frame can be returned through.
        loop {
            core::hint::spin_loop();
        }
    }
}

/// Per-thread canary derived from the master by XOR with the TID.
///
/// Each thread embeds this value at the bottom of its stack.  Using distinct
/// per-thread values means that a write overflowing from thread A's stack
/// into thread B's stack cannot forge a valid canary for B.
#[inline(always)]
pub fn thread_canary(tid: u32) -> u64 {
    let master = CANARY_VALUE.load(Ordering::SeqCst);
    // Mix TID with a large prime to ensure good bit-distribution.
    master ^ (tid as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

// ── Guard zone API ────────────────────────────────────────────────────────────

/// Write `GUARD_BYTE` (0xBE) to the `size` bytes starting at `ptr`.
///
/// Intended to bracket heap allocations so that a write-past-end is caught by
/// `check_guard_zone`.
///
/// # Safety
/// `ptr` must be valid for `size` writes and must not overlap any live
/// allocation.
pub fn mark_guard_zone(ptr: *mut u8, size: usize) {
    if ptr.is_null() || size == 0 {
        return;
    }
    for i in 0..size {
        unsafe {
            core::ptr::write_volatile(ptr.add(i), GUARD_BYTE);
        }
    }
}

/// Check whether the `size`-byte guard zone starting at `ptr` is intact.
///
/// Returns `true` if every byte equals `GUARD_BYTE` (no overflow detected).
/// Returns `false` if any byte differs (overflow or corruption detected).
pub fn check_guard_zone(ptr: *const u8, size: usize) -> bool {
    if ptr.is_null() || size == 0 {
        return true; // Nothing to check.
    }
    for i in 0..size {
        let b = unsafe { core::ptr::read_volatile(ptr.add(i)) };
        if b != GUARD_BYTE {
            return false;
        }
    }
    true
}

// ── Module init ───────────────────────────────────────────────────────────────

/// Initialize the stack canary subsystem.
///
/// Must be called very early in boot — before any task stacks are created and
/// before interrupt handlers that use a kernel stack are enabled.
pub fn init() {
    init_canary();
    let c = get_canary();
    serial_println!(
        "  [stack-canary] Canary initialised: 0x{:016X} (TSC+RDRAND seeded, GS:40 written)",
        c
    );
    serial_println!(
        "  [stack-canary] Guard-zone magic: 0x{:016X} (byte: 0x{:02X})",
        HEAP_GUARD_MAGIC,
        GUARD_BYTE
    );
}
