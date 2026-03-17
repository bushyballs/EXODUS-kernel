/// Kernel Pointer Exposure Prevention — Genesis hardening
///
/// Controls whether kernel virtual addresses may appear in any output visible
/// to user space (diagnostic interfaces, /proc-equivalents, debug serial
/// output filtered before forwarding, etc.).
///
/// Three restriction levels mirror the Linux `kptr_restrict` sysctl:
///
///   0 — Unrestricted: all kernel pointers are exposed.
///       Only suitable for kernel developers on isolated machines.
///
///   1 — Restricted (DEFAULT): addresses are hidden unless the caller has
///       been explicitly granted permission (e.g. CAP_SYSLOG equivalent).
///       This is the safe out-of-box default.
///
///   2 — Locked: addresses are always hidden regardless of caller privilege.
///       Appropriate for production / hardened deployments.
///
/// Usage:
///   - Call `hide_kptr(addr)` before printing any kernel address.
///   - It returns 0 when restriction is active, the original address otherwise.
///   - `kptr_ok()` is a fast boolean check for callers that want to skip
///     formatting altogether.
///
/// Critical rules honoured here:
///   - NO float casts.
///   - No heap (`alloc`).
///   - No panics.
///   - Saturating arithmetic where applicable.
///
/// All code is original.
use core::sync::atomic::{AtomicU8, Ordering};

// ── State ─────────────────────────────────────────────────────────────────────

/// Active kptr_restrict level.
/// Default: 1 (hide unless privileged).
static KPTR_RESTRICT: AtomicU8 = AtomicU8::new(1);

// ── Public API ────────────────────────────────────────────────────────────────

/// Set the kptr_restrict level (0, 1, or 2).
///
/// Values above 2 are clamped to 2 so the field stays meaningful.
pub fn set_kptr_restrict(level: u8) {
    let clamped = if level > 2 { 2 } else { level };
    KPTR_RESTRICT.store(clamped, Ordering::SeqCst);
}

/// Return the current kptr_restrict level.
pub fn get_kptr_restrict() -> u8 {
    KPTR_RESTRICT.load(Ordering::SeqCst)
}

/// Return `true` if the caller is allowed to expose kernel pointers.
///
/// Level 0: always allowed.
/// Level 1: allowed only for privileged callers — this module does not track
///          privilege itself; callers must check their own capability and only
///          call `kptr_ok()` when they hold CAP_SYSLOG or equivalent.
///          At this level `kptr_ok()` returns `false` as a conservative
///          default (callers that know they are privileged bypass this check).
/// Level 2: never allowed.
pub fn kptr_ok() -> bool {
    KPTR_RESTRICT.load(Ordering::SeqCst) == 0
}

/// Return `0` if kernel pointer restriction is active, otherwise return `addr`.
///
/// This is the single call-site helper that should wrap every kernel pointer
/// before it is passed to a formatted output function.
///
/// Example:
/// ```
/// serial_println!("  kmalloc slab at {:#x}", kptr::hide_kptr(slab_addr));
/// ```
pub fn hide_kptr(addr: u64) -> u64 {
    if kptr_ok() {
        addr
    } else {
        0
    }
}

/// Initialize the kptr subsystem.
///
/// Sets the default restriction level to 1 (hidden unless privileged) and
/// logs the active policy.
pub fn init() {
    // Default is already 1 from the static initialiser, but set explicitly
    // here to make the boot sequence self-documenting.
    set_kptr_restrict(1);
    crate::serial_println!(
        "  [kptr] Kernel pointer restriction initialised: level={} (1=hide unless privileged)",
        get_kptr_restrict()
    );
}
