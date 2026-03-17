use crate::serial_println;
/// Static tracepoints for Genesis AIOS.
///
/// Provides a fixed table of named tracepoints, each capable of holding up to
/// `MAX_TP_PROBES` callback pointers. Tracepoints are registered once at boot
/// and then fired at call sites throughout the kernel.
///
/// ## Design constraints (bare-metal #![no_std])
/// - NO heap: no Vec / Box / String / alloc::* — all storage is fixed-size
///   static arrays.
/// - NO floats: no `as f64` / `as f32` anywhere.
/// - NO panics: no unwrap() / expect() / panic!() — functions return bool /
///   Option to signal failures.
/// - All counters use saturating_add.
/// - Structs stored in static Mutex must be Copy + have `const fn empty()`.
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tracepoints in the static table.
pub const MAX_TRACEPOINTS: usize = 64;

/// Maximum number of probe callbacks per tracepoint.
pub const MAX_TP_PROBES: usize = 8;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Probe callback signature: `fn(arg1: u64, arg2: u64, arg3: u64)`.
///
/// Plain function pointers are `Copy + Clone`, which is required for storage
/// inside a `Copy` struct placed in a static Mutex.
pub type TpCallback = fn(u64, u64, u64);

/// A single static tracepoint descriptor.
#[derive(Copy, Clone)]
pub struct Tracepoint {
    /// Human-readable name (UTF-8, zero-padded), e.g. b"sched:sched_switch".
    pub name: [u8; 32],
    /// Number of valid bytes in `name`.
    pub name_len: u8,
    /// Registered probe callbacks.
    pub callbacks: [Option<TpCallback>; MAX_TP_PROBES],
    /// Number of currently registered probes.
    pub nprobes: u8,
    /// Whether this tracepoint is currently enabled.
    pub enabled: bool,
    /// Number of times `tp_fire` has been called for this tracepoint.
    pub hit_count: u64,
    /// Whether this slot is occupied (false = free).
    pub active: bool,
}

impl Tracepoint {
    /// Return an all-zero descriptor suitable for static initialisation.
    pub const fn empty() -> Self {
        Tracepoint {
            name: [0u8; 32],
            name_len: 0,
            callbacks: [None; MAX_TP_PROBES],
            nprobes: 0,
            enabled: false,
            hit_count: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static state
// ---------------------------------------------------------------------------

static TRACEPOINTS: Mutex<[Tracepoint; MAX_TRACEPOINTS]> =
    Mutex::new([Tracepoint::empty(); MAX_TRACEPOINTS]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to 32 bytes of `src` into the 32-byte array `dst` and return the
/// number of bytes copied.
#[inline]
fn copy_name(dst: &mut [u8; 32], src: &[u8]) -> u8 {
    let n = if src.len() < 32 { src.len() } else { 32 };
    for i in 0..n {
        dst[i] = src[i];
    }
    // Zero remainder.
    for i in n..32 {
        dst[i] = 0;
    }
    n as u8
}

/// Compare a tracepoint's stored name against `name`.
#[inline]
fn name_eq(tp: &Tracepoint, name: &[u8]) -> bool {
    let n = tp.name_len as usize;
    if n != name.len() {
        return false;
    }
    for i in 0..n {
        if tp.name[i] != name[i] {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new tracepoint with the given name.
///
/// Returns `Some(id)` where `id` is the slot index, or `None` if the table is
/// full or a tracepoint with the same name already exists.
pub fn tp_register(name: &[u8]) -> Option<u32> {
    let mut tps = TRACEPOINTS.lock();
    // Reject duplicate names.
    for i in 0..MAX_TRACEPOINTS {
        if tps[i].active && name_eq(&tps[i], name) {
            return None;
        }
    }
    // Find a free slot.
    for i in 0..MAX_TRACEPOINTS {
        if !tps[i].active {
            let mut tp = Tracepoint::empty();
            tp.name_len = copy_name(&mut tp.name, name);
            tp.active = true;
            tp.enabled = false;
            tps[i] = tp;
            return Some(i as u32);
        }
    }
    None
}

/// Attach a probe callback to the tracepoint identified by `id`.
///
/// Returns `true` on success, `false` if `id` is invalid, the tracepoint is
/// inactive, or the probe table is full.
pub fn tp_attach(id: u32, cb: TpCallback) -> bool {
    if id as usize >= MAX_TRACEPOINTS {
        return false;
    }
    let mut tps = TRACEPOINTS.lock();
    if !tps[id as usize].active {
        return false;
    }
    let tp = &mut tps[id as usize];
    let nprobes = tp.nprobes as usize;
    if nprobes >= MAX_TP_PROBES {
        return false;
    }
    tp.callbacks[nprobes] = Some(cb);
    tp.nprobes = tp.nprobes.saturating_add(1);
    true
}

/// Detach a previously attached probe callback from tracepoint `id`.
///
/// Compares function pointers directly. Removes the first matching callback
/// and compacts the array. Returns `true` if a callback was removed.
pub fn tp_detach(id: u32, cb: TpCallback) -> bool {
    if id as usize >= MAX_TRACEPOINTS {
        return false;
    }
    let mut tps = TRACEPOINTS.lock();
    if !tps[id as usize].active {
        return false;
    }
    let tp = &mut tps[id as usize];
    let nprobes = tp.nprobes as usize;
    let mut found = false;
    let mut new_n = 0usize;
    let mut new_cbs: [Option<TpCallback>; MAX_TP_PROBES] = [None; MAX_TP_PROBES];
    for i in 0..nprobes {
        if let Some(stored) = tp.callbacks[i] {
            // Compare fn pointers by casting to usize.
            if !found && (stored as usize) == (cb as usize) {
                found = true;
                // Skip this one — do not copy it.
                continue;
            }
            new_cbs[new_n] = Some(stored);
            new_n = new_n.saturating_add(1);
        }
    }
    if found {
        tp.callbacks = new_cbs;
        tp.nprobes = new_n as u8;
    }
    found
}

/// Enable firing of tracepoint `id`.
pub fn tp_enable(id: u32) -> bool {
    if id as usize >= MAX_TRACEPOINTS {
        return false;
    }
    let mut tps = TRACEPOINTS.lock();
    if !tps[id as usize].active {
        return false;
    }
    tps[id as usize].enabled = true;
    true
}

/// Disable firing of tracepoint `id`.
pub fn tp_disable(id: u32) -> bool {
    if id as usize >= MAX_TRACEPOINTS {
        return false;
    }
    let mut tps = TRACEPOINTS.lock();
    if !tps[id as usize].active {
        return false;
    }
    tps[id as usize].enabled = false;
    true
}

/// Fire tracepoint `id` with three arguments.
///
/// If the tracepoint is enabled, `hit_count` is incremented with
/// `saturating_add` and each registered callback is invoked with the three
/// arguments.
///
/// This function takes the lock, reads the callbacks into a local array, then
/// releases the lock before invoking callbacks, so callbacks may themselves
/// call non-tracepoint kernel functions without re-entering the lock.
pub fn tp_fire(id: u32, arg1: u64, arg2: u64, arg3: u64) {
    if id as usize >= MAX_TRACEPOINTS {
        return;
    }

    // Read what we need under the lock, then release it before calling back.
    let mut local_cbs: [Option<TpCallback>; MAX_TP_PROBES] = [None; MAX_TP_PROBES];
    let mut local_n: usize = 0;

    {
        let mut tps = TRACEPOINTS.lock();
        let tp = &mut tps[id as usize];
        if !tp.active || !tp.enabled {
            return;
        }
        tp.hit_count = tp.hit_count.saturating_add(1);
        local_n = tp.nprobes as usize;
        for i in 0..local_n {
            local_cbs[i] = tp.callbacks[i];
        }
    }

    // Invoke callbacks with lock released.
    for i in 0..local_n {
        if let Some(cb) = local_cbs[i] {
            cb(arg1, arg2, arg3);
        }
    }
}

/// Find a tracepoint by name.
///
/// Returns `Some(id)` if found, `None` otherwise.
pub fn tp_find(name: &[u8]) -> Option<u32> {
    let tps = TRACEPOINTS.lock();
    for i in 0..MAX_TRACEPOINTS {
        if tps[i].active && name_eq(&tps[i], name) {
            return Some(i as u32);
        }
    }
    None
}

/// Return the hit count for tracepoint `id`, or `None` if `id` is invalid or
/// the slot is not active.
pub fn tp_get_stats(id: u32) -> Option<u64> {
    if id as usize >= MAX_TRACEPOINTS {
        return None;
    }
    let tps = TRACEPOINTS.lock();
    if !tps[id as usize].active {
        return None;
    }
    Some(tps[id as usize].hit_count)
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the static tracepoint subsystem and register standard kernel
/// tracepoints.
pub fn init() {
    // Register standard tracepoints. We ignore the returned ids here; callers
    // that need them can use tp_find() by name.
    let _ = tp_register(b"sched:sched_switch");
    let _ = tp_register(b"irq:irq_handler_entry");
    let _ = tp_register(b"syscalls:sys_enter_read");
    let _ = tp_register(b"syscalls:sys_enter_write");
    let _ = tp_register(b"mm:page_fault_user");
    serial_println!("[tracepoint] static tracepoints initialized");
}
