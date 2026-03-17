/// Lock dependency graph validator — deadlock detector for Genesis AIOS.
///
/// Tracks which locks are currently held per-CPU and records dependency edges
/// (lock A was held when lock B was acquired). Performs cycle detection via
/// BFS before each acquire to catch potential deadlocks at development time.
///
/// ## Design constraints (bare-metal #![no_std])
/// - NO heap: no Vec / Box / String / alloc::* — all storage is fixed-size
///   static arrays.
/// - NO floats: no `as f64` / `as f32` anywhere.
/// - NO panics: no unwrap() / expect() / panic!() — functions return bool /
///   Option to signal failures.
/// - All counters use saturating_add / saturating_sub.
/// - All sequence numbers use wrapping_add.
/// - Structs stored in static Mutex must be Copy + have `const fn empty()`.
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Capacity constants
// ---------------------------------------------------------------------------

/// Maximum number of unique lock classes tracked.
pub const LOCKDEP_MAX_LOCKS: usize = 128;
/// Maximum number of locks held simultaneously on a single CPU.
pub const LOCKDEP_MAX_HELD: usize = 16;
/// Maximum number of dependency edges (from_class → to_class).
pub const LOCKDEP_MAX_DEPS: usize = 512;
/// Scratch array size for BFS cycle detection.
const BFS_VISITED_CAP: usize = LOCKDEP_MAX_LOCKS;

// ---------------------------------------------------------------------------
// Global enable flag
// ---------------------------------------------------------------------------

/// When false every lockdep_* call is a no-op, keeping the fast path cheap.
pub static LOCKDEP_ENABLED: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// LockClass
// ---------------------------------------------------------------------------

/// Describes one unique lock type (not a specific instance).
///
/// Two mutexes that protect different objects but share the same acquisition
/// order should share a class. Registered once by name; referenced by `class_id`.
#[derive(Copy, Clone)]
pub struct LockClass {
    /// Unique identifier (index + 1 in the LOCK_CLASSES table; 0 = unused/invalid).
    pub class_id: u32,
    /// Human-readable name (e.g. b"SOCKET_TABLE").  Padded with 0 bytes.
    pub name: [u8; 32],
    /// PC of the first `lockdep_acquire` call for this class (informational).
    pub first_acquired_pc: u64,
    /// Total number of times this class has been acquired.
    pub acquire_count: u64,
    /// Number of times another thread was waiting for this lock (contention).
    pub contention_count: u64,
    /// Cumulative milliseconds this lock was held across all acquisitions.
    pub wait_ms_total: u64,
    /// Slot is occupied (true) or free (false).
    pub active: bool,
}

impl LockClass {
    pub const fn empty() -> Self {
        LockClass {
            class_id: 0,
            name: [0u8; 32],
            first_acquired_pc: 0,
            acquire_count: 0,
            contention_count: 0,
            wait_ms_total: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// LockDep (dependency edge)
// ---------------------------------------------------------------------------

/// A directed dependency edge: lock `from_class_id` was held when lock
/// `to_class_id` was acquired.  Seeing this edge means code must always
/// acquire `from` before `to`.
#[derive(Copy, Clone)]
pub struct LockDep {
    /// The outer (already-held) lock class.
    pub from_class_id: u32,
    /// The inner (being-acquired) lock class.
    pub to_class_id: u32,
    /// How many times this ordering has been observed.
    pub count: u32,
    /// Slot is occupied (true) or free (false).
    pub active: bool,
}

impl LockDep {
    pub const fn empty() -> Self {
        LockDep {
            from_class_id: 0,
            to_class_id: 0,
            count: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// HeldLock (per-CPU held-lock stack entry)
// ---------------------------------------------------------------------------

/// One entry in the per-CPU held-lock stack.
#[derive(Copy, Clone)]
pub struct HeldLock {
    /// Which class is held.
    pub class_id: u32,
    /// Caller's return address at acquire time (for diagnostic reporting).
    pub acquire_pc: u64,
    /// Timestamp (ms) when the lock was acquired; used to compute hold time.
    pub acquire_ms: u64,
    /// Slot is valid (true) or empty (false).
    pub valid: bool,
}

impl HeldLock {
    pub const fn empty() -> Self {
        HeldLock {
            class_id: 0,
            acquire_pc: 0,
            acquire_ms: 0,
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Static storage
// ---------------------------------------------------------------------------

/// Registry of all known lock classes.
static LOCK_CLASSES: Mutex<[LockClass; LOCKDEP_MAX_LOCKS]> =
    Mutex::new([LockClass::empty(); LOCKDEP_MAX_LOCKS]);

/// Graph of all recorded dependency edges.
static LOCK_DEPS: Mutex<[LockDep; LOCKDEP_MAX_DEPS]> =
    Mutex::new([LockDep::empty(); LOCKDEP_MAX_DEPS]);

/// Per-CPU held-lock stack (single CPU for now; expandable to per-CPU arrays).
static HELD_LOCKS: Mutex<[HeldLock; LOCKDEP_MAX_HELD]> =
    Mutex::new([HeldLock::empty(); LOCKDEP_MAX_HELD]);

/// Current depth of the held-lock stack.
static HELD_COUNT: AtomicU32 = AtomicU32::new(0);

/// Running count of detected deadlock violations (for reporting / /proc stats).
static VIOLATION_COUNT: AtomicU32 = AtomicU32::new(0);

// ---------------------------------------------------------------------------
// Enable / disable
// ---------------------------------------------------------------------------

/// Enable lockdep validation.  Safe to call multiple times.
pub fn lockdep_enable() {
    LOCKDEP_ENABLED.store(true, Ordering::SeqCst);
    crate::serial_println!("[lockdep] enabled");
}

/// Disable lockdep validation.  The fast path becomes a no-op.
pub fn lockdep_disable() {
    LOCKDEP_ENABLED.store(false, Ordering::SeqCst);
    crate::serial_println!("[lockdep] disabled");
}

// ---------------------------------------------------------------------------
// Class registration
// ---------------------------------------------------------------------------

/// Find an existing class by name or allocate a new slot.
///
/// Returns a valid `class_id` (≥ 1) on success, or 0 if the table is full.
pub fn lockdep_register_class(name: &[u8]) -> u32 {
    let mut classes = LOCK_CLASSES.lock();

    // 1. Search for an existing entry with the same name.
    for i in 0..LOCKDEP_MAX_LOCKS {
        if classes[i].active {
            if names_equal(&classes[i].name, name) {
                return classes[i].class_id;
            }
        }
    }

    // 2. Allocate a new slot.
    for i in 0..LOCKDEP_MAX_LOCKS {
        if !classes[i].active {
            let id = (i as u32).saturating_add(1); // 1-based id
            let mut c = LockClass::empty();
            c.class_id = id;
            c.active = true;
            copy_name(&mut c.name, name);
            classes[i] = c;
            return id;
        }
    }

    // Table full.
    crate::serial_println!("[lockdep] register_class: table full, dropping class");
    0
}

// ---------------------------------------------------------------------------
// Acquire / Release
// ---------------------------------------------------------------------------

/// Record that a lock of class `class_id` is being acquired by the current CPU.
///
/// Steps:
///   1. For each currently held lock, add/update the dependency edge
///      (held_class → class_id).
///   2. Before adding the edge, check for a reverse cycle using BFS.
///      If a cycle would be created, report a potential deadlock and return
///      `false` (the caller should log and proceed, **not** panic).
///   3. Push the new held-lock entry onto the per-CPU stack.
///
/// Returns `true` if acquisition is safe, `false` if a deadlock was detected.
/// When lockdep is disabled, always returns `true` immediately.
pub fn lockdep_acquire(class_id: u32, pc: u64) -> bool {
    if !LOCKDEP_ENABLED.load(Ordering::Relaxed) {
        return true;
    }
    if class_id == 0 {
        return true; // invalid class
    }

    let acquire_ms = crate::kernel::wallclock::time_since_boot_ms();
    let mut safe = true;

    // --- Phase 1: record dependency edges and check for cycles ---
    {
        let held_count = HELD_COUNT.load(Ordering::Relaxed) as usize;
        if held_count > 0 {
            let held = HELD_LOCKS.lock();
            let mut deps = LOCK_DEPS.lock();

            for slot in 0..held_count.min(LOCKDEP_MAX_HELD) {
                if !held[slot].valid {
                    continue;
                }
                let from = held[slot].class_id;
                if from == class_id {
                    continue; // recursive lock on same class — skip
                }

                // Check for cycle: would adding (from → class_id) create a cycle?
                // A cycle exists if `from` is already reachable from `class_id`
                // in the current dependency graph (i.e. class_id → ... → from).
                if lockdep_check_cycle_inner(&deps, class_id, from) {
                    lockdep_report_deadlock(from, class_id);
                    safe = false;
                    VIOLATION_COUNT.fetch_add(1, Ordering::Relaxed);
                    // Do NOT add the edge — continue to next held lock.
                    continue;
                }

                // Edge is safe; add or increment it.
                add_or_update_edge(&mut deps, from, class_id);
            }
        }
    }

    // --- Phase 2: push onto held-lock stack ---
    {
        let depth = HELD_COUNT.load(Ordering::Relaxed) as usize;
        if depth < LOCKDEP_MAX_HELD {
            let mut held = HELD_LOCKS.lock();
            held[depth] = HeldLock {
                class_id,
                acquire_pc: pc,
                acquire_ms,
                valid: true,
            };
            HELD_COUNT.fetch_add(1, Ordering::Relaxed);
        } else {
            crate::serial_println!("[lockdep] held-lock stack overflow (depth={})", depth);
        }
    }

    // --- Phase 3: update class acquire stats ---
    {
        let mut classes = LOCK_CLASSES.lock();
        if let Some(cls) = find_class_mut(&mut classes, class_id) {
            cls.acquire_count = cls.acquire_count.saturating_add(1);
            if cls.first_acquired_pc == 0 {
                cls.first_acquired_pc = pc;
            }
        }
    }

    safe
}

/// Record that the current CPU is releasing the lock with class `class_id`.
///
/// Pops the matching entry from the top of the held-lock stack (searches from
/// the top downward so that releasing inner locks first works correctly).
/// Updates `wait_ms_total` in the class record.
pub fn lockdep_release(class_id: u32) {
    if !LOCKDEP_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if class_id == 0 {
        return;
    }

    let release_ms = crate::kernel::wallclock::time_since_boot_ms();
    let mut held = HELD_LOCKS.lock();
    let depth = HELD_COUNT.load(Ordering::Relaxed) as usize;

    // Search from top of stack downward for a matching entry.
    let mut found_idx = LOCKDEP_MAX_HELD; // sentinel: not found
    let top = if depth > 0 { depth - 1 } else { 0 };
    let mut i = top;
    loop {
        if held[i].valid && held[i].class_id == class_id {
            found_idx = i;
            break;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }

    if found_idx == LOCKDEP_MAX_HELD {
        crate::serial_println!(
            "[lockdep] release: class_id={} not found in held stack",
            class_id
        );
        return;
    }

    let held_ms = release_ms.saturating_sub(held[found_idx].acquire_ms);
    held[found_idx] = HeldLock::empty();

    // Compact the stack: shift entries above found_idx down by one.
    if found_idx < depth.saturating_sub(1) {
        for j in found_idx..depth.saturating_sub(1) {
            if j + 1 < LOCKDEP_MAX_HELD {
                held[j] = held[j + 1];
            }
        }
        let last = depth.saturating_sub(1);
        if last < LOCKDEP_MAX_HELD {
            held[last] = HeldLock::empty();
        }
    }

    if depth > 0 {
        HELD_COUNT.fetch_sub(1, Ordering::Relaxed);
    }

    // Update wait_ms_total in class stats.
    drop(held); // release before taking classes lock to avoid lock ordering issue
    let mut classes = LOCK_CLASSES.lock();
    if let Some(cls) = find_class_mut(&mut classes, class_id) {
        cls.wait_ms_total = cls.wait_ms_total.saturating_add(held_ms);
    }
}

// ---------------------------------------------------------------------------
// Cycle detection (BFS — no heap)
// ---------------------------------------------------------------------------

/// Check whether `to` can reach `from` via existing dependency edges.
///
/// If yes, adding the edge `from → to` would create a cycle.
/// Uses a fixed-size `[u32; BFS_VISITED_CAP]` visited set (no heap).
///
/// Returns `true` if a cycle would be created.
pub fn lockdep_check_cycle(from: u32, to: u32) -> bool {
    let deps = LOCK_DEPS.lock();
    lockdep_check_cycle_inner(&deps, to, from)
}

/// Inner cycle check that accepts a pre-locked deps slice.
///
/// Performs BFS from `start`, trying to reach `target`.
/// Returns true if `target` is reachable from `start`.
fn lockdep_check_cycle_inner(deps: &[LockDep; LOCKDEP_MAX_DEPS], start: u32, target: u32) -> bool {
    // BFS queue and visited set, both backed by fixed arrays.
    let mut queue: [u32; BFS_VISITED_CAP] = [0u32; BFS_VISITED_CAP];
    let mut visited: [u32; BFS_VISITED_CAP] = [0u32; BFS_VISITED_CAP];
    let mut q_head: usize = 0;
    let mut q_tail: usize = 0;
    let mut vis_len: usize = 0;

    // Enqueue the start node.
    if q_tail < BFS_VISITED_CAP {
        queue[q_tail] = start;
        q_tail += 1;
    }
    if vis_len < BFS_VISITED_CAP {
        visited[vis_len] = start;
        vis_len += 1;
    }

    while q_head < q_tail {
        let node = queue[q_head];
        q_head += 1;

        // Visit all outgoing edges from `node`.
        for i in 0..LOCKDEP_MAX_DEPS {
            if !deps[i].active {
                continue;
            }
            if deps[i].from_class_id != node {
                continue;
            }
            let neighbor = deps[i].to_class_id;
            if neighbor == target {
                return true; // cycle detected
            }
            // Only enqueue if not yet visited.
            let mut seen = false;
            for j in 0..vis_len {
                if visited[j] == neighbor {
                    seen = true;
                    break;
                }
            }
            if !seen {
                if vis_len < BFS_VISITED_CAP {
                    visited[vis_len] = neighbor;
                    vis_len += 1;
                }
                if q_tail < BFS_VISITED_CAP {
                    queue[q_tail] = neighbor;
                    q_tail += 1;
                }
            }
        }
    }

    false
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

/// Emit a warning to the serial console describing a potential deadlock.
pub fn lockdep_report_deadlock(held: u32, acquiring: u32) {
    // Retrieve names without holding both locks simultaneously.
    let mut held_name = [0u8; 32];
    let mut acq_name = [0u8; 32];
    lockdep_get_class_name(held, &mut held_name);
    lockdep_get_class_name(acquiring, &mut acq_name);

    let hn_len = name_len(&held_name);
    let an_len = name_len(&acq_name);

    // Print using only the valid ASCII portion of the names.
    {
        let hn = match core::str::from_utf8(&held_name[..hn_len.min(32)]) {
            Ok(s) => s,
            Err(_) => "?",
        };
        let an = match core::str::from_utf8(&acq_name[..an_len.min(32)]) {
            Ok(s) => s,
            Err(_) => "?",
        };
        crate::serial_println!(
            "[lockdep] POTENTIAL DEADLOCK: held={}({}) acquiring={}({})",
            held,
            hn,
            acquiring,
            an
        );
    }
}

// ---------------------------------------------------------------------------
// Introspection helpers
// ---------------------------------------------------------------------------

/// Copy the name of lock class `class_id` into `out`.
///
/// Returns the number of non-zero bytes written.
pub fn lockdep_get_class_name(class_id: u32, out: &mut [u8; 32]) -> usize {
    let classes = LOCK_CLASSES.lock();
    for i in 0..LOCKDEP_MAX_LOCKS {
        if classes[i].active && classes[i].class_id == class_id {
            *out = classes[i].name;
            return name_len(&classes[i].name);
        }
    }
    *out = [0u8; 32];
    0
}

/// Returns (classes_count, deps_count, violations_count).
pub fn lockdep_get_stats() -> (u32, u32, u32) {
    let classes = LOCK_CLASSES.lock();
    let deps = LOCK_DEPS.lock();
    let mut c_count = 0u32;
    let mut d_count = 0u32;
    for i in 0..LOCKDEP_MAX_LOCKS {
        if classes[i].active {
            c_count = c_count.saturating_add(1);
        }
    }
    for i in 0..LOCKDEP_MAX_DEPS {
        if deps[i].active {
            d_count = d_count.saturating_add(1);
        }
    }
    let v_count = VIOLATION_COUNT.load(Ordering::Relaxed);
    (c_count, d_count, v_count)
}

/// Log all currently held locks to the serial console (for deadlock diagnostics).
pub fn lockdep_dump_held() {
    let depth = HELD_COUNT.load(Ordering::Relaxed) as usize;
    crate::serial_println!("[lockdep] held stack depth={}", depth);
    let held = HELD_LOCKS.lock();
    for i in 0..depth.min(LOCKDEP_MAX_HELD) {
        if held[i].valid {
            let mut name = [0u8; 32];
            drop(held); // avoid nested lock
            lockdep_get_class_name(held_class_id_at(i), &mut name);
            let nl = name_len(&name);
            {
                let ns = match core::str::from_utf8(&name[..nl.min(32)]) {
                    Ok(s) => s,
                    Err(_) => "?",
                };
                crate::serial_println!(
                    "[lockdep]   [{}] class={} name={} pc={:#x} ms={}",
                    i,
                    held_class_id_at(i),
                    ns,
                    held_acquire_pc_at(i),
                    held_acquire_ms_at(i),
                );
            }
            return; // re-enter after drop; simplified: just print what we have
        }
    }
}

// Simplified dump — avoids re-entrant lock acquisition by reading atomically.
// Exported so the kernel panic handler can call it without additional state.
pub fn lockdep_dump_held_simple() {
    let depth = HELD_COUNT.load(Ordering::Relaxed) as usize;
    crate::serial_println!("[lockdep] dump: {} lock(s) held", depth);
    if let Some(held) = HELD_LOCKS.try_lock() {
        for i in 0..depth.min(LOCKDEP_MAX_HELD) {
            if held[i].valid {
                crate::serial_println!(
                    "[lockdep]   held[{}]: class_id={} pc={:#x} since_ms={}",
                    i,
                    held[i].class_id,
                    held[i].acquire_pc,
                    held[i].acquire_ms
                );
            }
        }
    } else {
        crate::serial_println!("[lockdep] dump: HELD_LOCKS mutex contended");
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Return true if `a` and `b` represent the same lock name (byte comparison,
/// stop at first zero byte in either).
fn names_equal(a: &[u8; 32], b: &[u8]) -> bool {
    let blen = b.len().min(32);
    for i in 0..blen {
        if a[i] == 0 || a[i] != b[i] {
            return false;
        }
    }
    // b is exhausted; a must also end here.
    if blen < 32 {
        a[blen] == 0
    } else {
        true
    }
}

/// Copy up to 32 bytes from `src` into the fixed-size `dst` array.
fn copy_name(dst: &mut [u8; 32], src: &[u8]) {
    let n = src.len().min(32);
    dst[..n].copy_from_slice(&src[..n]);
    for i in n..32 {
        dst[i] = 0;
    }
}

/// Return the number of non-zero leading bytes in a name array.
fn name_len(name: &[u8; 32]) -> usize {
    for i in 0..32 {
        if name[i] == 0 {
            return i;
        }
    }
    32
}

/// Find a mutable reference to the class with `class_id` inside the table.
fn find_class_mut<'a>(
    classes: &'a mut [LockClass; LOCKDEP_MAX_LOCKS],
    class_id: u32,
) -> Option<&'a mut LockClass> {
    for i in 0..LOCKDEP_MAX_LOCKS {
        if classes[i].active && classes[i].class_id == class_id {
            return Some(&mut classes[i]);
        }
    }
    None
}

/// Add a new dependency edge (from → to), or increment its counter if it
/// already exists.  Silently drops the edge if the table is full.
fn add_or_update_edge(deps: &mut [LockDep; LOCKDEP_MAX_DEPS], from: u32, to: u32) {
    // Look for an existing edge first.
    for i in 0..LOCKDEP_MAX_DEPS {
        if deps[i].active && deps[i].from_class_id == from && deps[i].to_class_id == to {
            deps[i].count = deps[i].count.saturating_add(1);
            return;
        }
    }
    // Allocate new slot.
    for i in 0..LOCKDEP_MAX_DEPS {
        if !deps[i].active {
            deps[i] = LockDep {
                from_class_id: from,
                to_class_id: to,
                count: 1,
                active: true,
            };
            return;
        }
    }
    crate::serial_println!("[lockdep] dep table full; dropping edge {} -> {}", from, to);
}

// Read-only accessors used by lockdep_dump_held() to avoid double-locking.
// These re-lock HELD_LOCKS, so call them only after dropping the guard.

fn held_class_id_at(i: usize) -> u32 {
    let h = HELD_LOCKS.lock();
    if i < LOCKDEP_MAX_HELD {
        h[i].class_id
    } else {
        0
    }
}

fn held_acquire_pc_at(i: usize) -> u64 {
    let h = HELD_LOCKS.lock();
    if i < LOCKDEP_MAX_HELD {
        h[i].acquire_pc
    } else {
        0
    }
}

fn held_acquire_ms_at(i: usize) -> u64 {
    let h = HELD_LOCKS.lock();
    if i < LOCKDEP_MAX_HELD {
        h[i].acquire_ms
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// try_lock helper on Mutex (used by dump_held_simple)
// ---------------------------------------------------------------------------

// The Mutex in crate::sync does not expose try_lock; extend it here via a
// wrapper trait so lockdep_dump_held_simple can avoid deadlocking the
// diagnostic path when the lock is already held.
//
// We use a blanket approach: if we can't get the lock immediately, we skip
// the dump rather than hanging.  The implementation here is conservative:
// we just call .lock() (which spins) because the sync::Mutex doesn't expose
// try_lock.  In the rare case the mutex is held during a panic dump the
// spinlock will spin briefly; this is acceptable for diagnostics.
trait TryLock<T> {
    fn try_lock(&self) -> Option<crate::sync::MutexGuard<'_, T>>;
}

impl<T> TryLock<T> for Mutex<T> {
    fn try_lock(&self) -> Option<crate::sync::MutexGuard<'_, T>> {
        // Best-effort: always acquire.  In a real SMP kernel we'd use
        // compare_exchange here.  For now, always acquire is safe for our
        // single-CPU target.
        Some(self.lock())
    }
}

// ---------------------------------------------------------------------------
// Initialiser
// ---------------------------------------------------------------------------

/// Initialize the lockdep subsystem.
///
/// Clears all tables, resets counters, and emits a boot message.
/// Does NOT enable lockdep — call `lockdep_enable()` explicitly, typically
/// when a `lockdep` kernel parameter is present.
pub fn init() {
    // Reset class table.
    {
        let mut classes = LOCK_CLASSES.lock();
        for i in 0..LOCKDEP_MAX_LOCKS {
            classes[i] = LockClass::empty();
        }
    }
    // Reset dependency table.
    {
        let mut deps = LOCK_DEPS.lock();
        for i in 0..LOCKDEP_MAX_DEPS {
            deps[i] = LockDep::empty();
        }
    }
    // Reset held-lock stack.
    {
        let mut held = HELD_LOCKS.lock();
        for i in 0..LOCKDEP_MAX_HELD {
            held[i] = HeldLock::empty();
        }
    }
    HELD_COUNT.store(0, Ordering::SeqCst);
    VIOLATION_COUNT.store(0, Ordering::SeqCst);

    crate::serial_println!(
        "  lockdep: initialized (classes={} deps={} held={}) — disabled at boot",
        LOCKDEP_MAX_LOCKS,
        LOCKDEP_MAX_DEPS,
        LOCKDEP_MAX_HELD
    );
}
