use crate::sync::Mutex;
/// Combined POSIX named semaphores + System V semaphore arrays
///
/// POSIX semaphores (sem_open/sem_wait/sem_post/sem_close/sem_unlink):
///   Named semaphores identified by a byte-string name (leading '/' stripped).
///   Each has a 32-bit counter, a waiter list, and a reference count.
///   Semaphore fds are synthetic: SEM_FD_BASE + slot_index.
///
/// System V semaphore arrays (semget/semop/semctl):
///   Each semaphore set holds up to SEMMSL individual 16-bit counters.
///   Atomic multi-operation semop sequences (up to SEMOPM ops).
///   GETALL / SETALL copy values through a user-space pointer.
///
/// Rules:
///   - NO heap: Vec, Box, String, alloc::* are forbidden.
///   - NO float casts: no `as f64` / `as f32`.
///   - NO panics: no unwrap(), expect(), panic!().
///   - All counters: saturating_add / saturating_sub.
///   - Structs in static Mutex: Copy + const fn empty().
///   - MMIO: read_volatile / write_volatile only (not needed here).
///
/// All code is original.
use crate::{serial_print, serial_println};
use core::sync::atomic::{AtomicI32, Ordering};

// ---------------------------------------------------------------------------
// POSIX semaphore constants
// ---------------------------------------------------------------------------

/// Lowest synthetic fd issued for POSIX semaphores.
pub const SEM_FD_BASE: i32 = 8000;

/// Maximum value a semaphore counter may hold.
pub const SEM_VALUE_MAX: u32 = 32767;

/// O_CREAT flag — create semaphore if it does not already exist.
pub const O_CREAT: i32 = 0x40;

/// O_EXCL flag — fail if semaphore already exists (used with O_CREAT).
pub const O_EXCL: i32 = 0x80;

// ---------------------------------------------------------------------------
// System V constants
// ---------------------------------------------------------------------------

/// Maximum semaphores per set.
pub const SEMMSL: usize = 32;

/// Maximum semaphores system-wide (across all sets; enforced via set count).
pub const SEMMNS: usize = 128;

/// Maximum operations per semop call.
pub const SEMOPM: usize = 32;

/// IPC_PRIVATE key — always create a new (anonymous) set.
pub const IPC_PRIVATE: i32 = 0;

/// IPC_CREAT flag.
pub const IPC_CREAT: i32 = 0o1000;

/// IPC_EXCL flag.
pub const IPC_EXCL: i32 = 0o2000;

/// IPC_RMID command — remove the semaphore set.
pub const IPC_RMID: i32 = 0;

/// IPC_SET command — update permissions.
pub const IPC_SET: i32 = 1;

/// IPC_STAT command — read semaphore set metadata.
pub const IPC_STAT: i32 = 2;

/// GETVAL — return semval of semaphore semnum.
pub const GETVAL: i32 = 12;

/// SETVAL — set semval of semaphore semnum.
pub const SETVAL: i32 = 16;

/// GETALL — copy all semvals to user buffer.
pub const GETALL: i32 = 13;

/// SETALL — set all semvals from user buffer.
pub const SETALL: i32 = 17;

/// GETPID — return PID of last semop caller on semnum.
pub const GETPID: i32 = 11;

/// GETNCNT — number of waiters for semval to increase.
pub const GETNCNT: i32 = 14;

/// GETZCNT — number of waiters for semval to reach zero.
pub const GETZCNT: i32 = 15;

/// IPC_NOWAIT semop flag — return EAGAIN instead of blocking.
pub const IPC_NOWAIT: i16 = 0x800;

/// SEM_UNDO semop flag — undo adjustment on process exit.
pub const SEM_UNDO: i16 = 0x1000u16 as i16;

// ---------------------------------------------------------------------------
// Capacity limits
// ---------------------------------------------------------------------------

/// Maximum concurrent POSIX named semaphores.
const MAX_POSIX_SEMS: usize = 64;

/// Maximum SysV semaphore sets.
const MAX_SEM_SETS: usize = 32;

/// Maximum PIDs blocked on a single POSIX semaphore.
const MAX_POSIX_WAITERS: usize = 16;

/// Spin iterations representing one millisecond (rough approximation).
const ITERS_PER_MS: u64 = 1000;

// ---------------------------------------------------------------------------
// POSIX semaphore data structures
// ---------------------------------------------------------------------------

/// A single POSIX named semaphore.
#[derive(Copy, Clone)]
pub struct PosixSem {
    /// Semaphore name bytes (without leading '/').
    pub name: [u8; 32],
    /// Length of the name in bytes.
    pub name_len: u8,
    /// Current semaphore value.
    pub value: u32,
    /// Total number of blocked waiters (informational).
    pub waiters: u32,
    /// PIDs currently blocked on this semaphore.
    pub waiter_pids: [u32; MAX_POSIX_WAITERS],
    /// Number of valid entries in waiter_pids.
    pub waiter_count: u8,
    /// Synthetic fd for this semaphore (SEM_FD_BASE + slot index).
    pub fd: i32,
    /// Number of open handles (sem_open callers minus sem_close callers).
    pub ref_count: u32,
    /// True while the semaphore is usable; false means it has been unlinked
    /// and will be destroyed when ref_count drops to 0.
    pub active: bool,
    /// Slot is in use.
    pub in_use: bool,
}

impl PosixSem {
    pub const fn empty() -> Self {
        PosixSem {
            name: [0u8; 32],
            name_len: 0,
            value: 0,
            waiters: 0,
            waiter_pids: [0u32; MAX_POSIX_WAITERS],
            waiter_count: 0,
            fd: 0,
            ref_count: 0,
            active: false,
            in_use: false,
        }
    }
}

// ---------------------------------------------------------------------------
// System V semaphore data structures
// ---------------------------------------------------------------------------

/// One semaphore within a SysV set.
#[derive(Copy, Clone)]
pub struct Semaphore {
    /// Current semaphore value.
    pub semval: u16,
    /// PID of the process that performed the last semop.
    pub sempid: u32,
    /// Number of processes waiting for semval to increase.
    pub semncnt: u16,
    /// Number of processes waiting for semval to reach zero.
    pub semzcnt: u16,
}

impl Semaphore {
    pub const fn empty() -> Self {
        Semaphore {
            semval: 0,
            sempid: 0,
            semncnt: 0,
            semzcnt: 0,
        }
    }
}

/// A SysV semaphore set.
#[derive(Copy, Clone)]
pub struct SemSet {
    /// Semaphore set identifier.
    pub semid: i32,
    /// IPC key used to look up this set.
    pub key: i32,
    /// Number of semaphores in this set.
    pub nsems: u16,
    /// The semaphore array.
    pub sems: [Semaphore; SEMMSL],
    /// Owner UID.
    pub uid: u32,
    /// Owner GID.
    pub gid: u32,
    /// Creator UID.
    pub cuid: u32,
    /// Creator GID.
    pub cgid: u32,
    /// Permission bits.
    pub mode: u16,
    /// Slot is occupied.
    pub active: bool,
}

impl SemSet {
    pub const fn empty() -> Self {
        SemSet {
            semid: 0,
            key: 0,
            nsems: 0,
            sems: [Semaphore::empty(); SEMMSL],
            uid: 0,
            gid: 0,
            cuid: 0,
            cgid: 0,
            mode: 0,
            active: false,
        }
    }
}

/// One operation in a semop call.
#[derive(Copy, Clone)]
pub struct Sembuf {
    /// Index of the semaphore within the set.
    pub sem_num: u16,
    /// Operation:  > 0 release,  < 0 acquire,  == 0 wait-for-zero.
    pub sem_op: i16,
    /// Flags: IPC_NOWAIT and/or SEM_UNDO.
    pub sem_flg: i16,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Table of all POSIX named semaphores.
static POSIX_SEMS: Mutex<[PosixSem; MAX_POSIX_SEMS]> =
    Mutex::new([PosixSem::empty(); MAX_POSIX_SEMS]);

/// Table of all SysV semaphore sets.
static SEM_SETS: Mutex<[SemSet; MAX_SEM_SETS]> = Mutex::new([SemSet::empty(); MAX_SEM_SETS]);

/// Monotonically increasing SysV semaphore ID counter.
static NEXT_SEMID: AtomicI32 = AtomicI32::new(1);

// ---------------------------------------------------------------------------
// Internal helpers — POSIX
// ---------------------------------------------------------------------------

/// Compare a name slice against the stored name of a slot.
#[inline]
fn name_matches(slot: &PosixSem, name: &[u8]) -> bool {
    if slot.name_len as usize != name.len() {
        return false;
    }
    let len = slot.name_len as usize;
    slot.name[..len] == name[..len]
}

/// Strip a leading '/' from name, if present.
fn strip_slash(name: &[u8]) -> &[u8] {
    if !name.is_empty() && name[0] == b'/' {
        &name[1..]
    } else {
        name
    }
}

/// Add PID to the waiter list of a POSIX semaphore slot.
fn posix_add_waiter(slot: &mut PosixSem, pid: u32) {
    if (slot.waiter_count as usize) < MAX_POSIX_WAITERS {
        slot.waiter_pids[slot.waiter_count as usize] = pid;
        slot.waiter_count = slot.waiter_count.saturating_add(1);
    }
    slot.waiters = slot.waiters.saturating_add(1);
}

/// Remove the first PID entry from the waiter list.  Returns the PID if found.
fn posix_pop_waiter(slot: &mut PosixSem) -> Option<u32> {
    if slot.waiter_count == 0 {
        return None;
    }
    let pid = slot.waiter_pids[0];
    // Shift remaining entries left.
    let remaining = (slot.waiter_count as usize).saturating_sub(1);
    for i in 0..remaining {
        slot.waiter_pids[i] = slot.waiter_pids[i.saturating_add(1)];
    }
    slot.waiter_pids[remaining] = 0;
    slot.waiter_count = slot.waiter_count.saturating_sub(1);
    if slot.waiters > 0 {
        slot.waiters = slot.waiters.saturating_sub(1);
    }
    Some(pid)
}

// ---------------------------------------------------------------------------
// POSIX named semaphore public API
// ---------------------------------------------------------------------------

/// sem_open — open or create a POSIX named semaphore.
///
/// Returns a synthetic fd (SEM_FD_BASE + slot_index) on success, or a
/// negative errno on failure:
///   -17 (EEXIST)  — O_CREAT | O_EXCL and semaphore already exists
///   -2  (ENOENT)  — semaphore not found and O_CREAT not set
///   -12 (ENOMEM)  — table full
///   -22 (EINVAL)  — name too long / empty
pub fn sem_open(name: &[u8], flags: i32, value: u32) -> i32 {
    let name = strip_slash(name);
    if name.is_empty() || name.len() > 32 {
        return -22; // EINVAL
    }

    let mut table = POSIX_SEMS.lock();

    // Search for an existing slot with this name.
    let mut found_idx: Option<usize> = None;
    for i in 0..MAX_POSIX_SEMS {
        if table[i].in_use && name_matches(&table[i], name) {
            found_idx = Some(i);
            break;
        }
    }

    if let Some(idx) = found_idx {
        // Semaphore already exists.
        if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
            return -17; // EEXIST
        }
        // Re-use existing slot — bump ref_count.
        table[idx].ref_count = table[idx].ref_count.saturating_add(1);
        return table[idx].fd;
    }

    // Semaphore does not exist.
    if flags & O_CREAT == 0 {
        return -2; // ENOENT
    }

    // Find a free slot.
    let mut free_idx: Option<usize> = None;
    for i in 0..MAX_POSIX_SEMS {
        if !table[i].in_use {
            free_idx = Some(i);
            break;
        }
    }
    let idx = match free_idx {
        Some(i) => i,
        None => return -12, // ENOMEM — table full
    };

    // Populate the new slot.
    let slot = &mut table[idx];
    *slot = PosixSem::empty();
    let copy_len = if name.len() < 32 { name.len() } else { 32 };
    slot.name[..copy_len].copy_from_slice(&name[..copy_len]);
    slot.name_len = copy_len as u8;
    slot.value = if value > SEM_VALUE_MAX {
        SEM_VALUE_MAX
    } else {
        value
    };
    slot.fd = SEM_FD_BASE.saturating_add(idx as i32);
    slot.ref_count = 1;
    slot.active = true;
    slot.in_use = true;

    slot.fd
}

/// sem_close — close one handle to a POSIX semaphore.
///
/// Decrements ref_count.  When ref_count reaches 0 and active==false
/// (the semaphore has been unlinked), the slot is freed.
/// Returns 0 on success, -9 (EBADF) on an invalid fd.
pub fn sem_close(fd: i32) -> i32 {
    if fd < SEM_FD_BASE {
        return -9; // EBADF
    }
    let idx = (fd - SEM_FD_BASE) as usize;
    if idx >= MAX_POSIX_SEMS {
        return -9; // EBADF
    }
    let mut table = POSIX_SEMS.lock();
    let slot = &mut table[idx];
    if !slot.in_use {
        return -9; // EBADF
    }
    slot.ref_count = slot.ref_count.saturating_sub(1);
    // If the semaphore was unlinked and all handles are closed, free the slot.
    if !slot.active && slot.ref_count == 0 {
        *slot = PosixSem::empty();
    }
    0
}

/// sem_unlink — remove a named semaphore.
///
/// Marks the semaphore as inactive so no new sem_open calls will find it.
/// The slot is freed when the last handle is closed.
/// Returns 0 on success, -2 (ENOENT) if not found.
pub fn sem_unlink(name: &[u8]) -> i32 {
    let name = strip_slash(name);
    let mut table = POSIX_SEMS.lock();
    for i in 0..MAX_POSIX_SEMS {
        if table[i].in_use && name_matches(&table[i], name) {
            table[i].active = false;
            // Clear the name so future lookups won't find it.
            table[i].name_len = 0;
            // If there are no open handles, free the slot immediately.
            if table[i].ref_count == 0 {
                table[i] = PosixSem::empty();
            }
            return 0;
        }
    }
    -2 // ENOENT
}

/// sem_wait — decrement (acquire) semaphore, blocking until available.
///
/// If value > 0, decrements value and returns 0 immediately.
/// If value == 0, registers PID 1 as a stub waiter and spins ~100 000
/// iterations; returns -11 (EAGAIN) if the value never becomes > 0.
pub fn sem_wait(fd: i32) -> i32 {
    if fd < SEM_FD_BASE {
        return -9;
    }
    let idx = (fd - SEM_FD_BASE) as usize;
    if idx >= MAX_POSIX_SEMS {
        return -9;
    }

    // Fast path: try to decrement without blocking.
    {
        let mut table = POSIX_SEMS.lock();
        let slot = &mut table[idx];
        if !slot.in_use {
            return -9;
        }
        if slot.value > 0 {
            slot.value = slot.value.saturating_sub(1);
            return 0;
        }
        // Register stub waiter (PID 1).
        posix_add_waiter(slot, 1);
    }

    // Spin-wait loop.
    let mut spins = 0u32;
    loop {
        core::hint::spin_loop();
        spins = spins.saturating_add(1);

        let mut table = POSIX_SEMS.lock();
        let slot = &mut table[idx];
        if !slot.in_use {
            return -9;
        }
        if slot.value > 0 {
            slot.value = slot.value.saturating_sub(1);
            // Remove our stub waiter entry.
            posix_pop_waiter(slot);
            return 0;
        }
        if spins >= 100_000 {
            // Timeout — remove our stub waiter entry and give up.
            posix_pop_waiter(slot);
            return -11; // EAGAIN
        }
    }
}

/// sem_trywait — attempt to decrement semaphore without blocking.
///
/// Returns 0 on success, -11 (EAGAIN) if value == 0.
pub fn sem_trywait(fd: i32) -> i32 {
    if fd < SEM_FD_BASE {
        return -9;
    }
    let idx = (fd - SEM_FD_BASE) as usize;
    if idx >= MAX_POSIX_SEMS {
        return -9;
    }
    let mut table = POSIX_SEMS.lock();
    let slot = &mut table[idx];
    if !slot.in_use {
        return -9;
    }
    if slot.value > 0 {
        slot.value = slot.value.saturating_sub(1);
        0
    } else {
        -11 // EAGAIN
    }
}

/// sem_timedwait — decrement semaphore with a millisecond timeout.
///
/// Spins up to `timeout_ms * ITERS_PER_MS` iterations.
/// Returns 0 on success, -110 (ETIMEDOUT) if the timeout expired, or
/// -9 (EBADF) for an invalid fd.
pub fn sem_timedwait(fd: i32, timeout_ms: u64) -> i32 {
    if fd < SEM_FD_BASE {
        return -9;
    }
    let idx = (fd - SEM_FD_BASE) as usize;
    if idx >= MAX_POSIX_SEMS {
        return -9;
    }

    // Fast path.
    {
        let mut table = POSIX_SEMS.lock();
        let slot = &mut table[idx];
        if !slot.in_use {
            return -9;
        }
        if slot.value > 0 {
            slot.value = slot.value.saturating_sub(1);
            return 0;
        }
        posix_add_waiter(slot, 1);
    }

    let max_iters: u64 = timeout_ms.saturating_mul(ITERS_PER_MS);
    let mut i: u64 = 0;
    loop {
        core::hint::spin_loop();
        i = i.saturating_add(1);

        let mut table = POSIX_SEMS.lock();
        let slot = &mut table[idx];
        if !slot.in_use {
            return -9;
        }
        if slot.value > 0 {
            slot.value = slot.value.saturating_sub(1);
            posix_pop_waiter(slot);
            return 0;
        }
        if i >= max_iters {
            posix_pop_waiter(slot);
            return -110; // ETIMEDOUT
        }
    }
}

/// sem_post — increment (release) semaphore.
///
/// Increments value (capped at SEM_VALUE_MAX).  If there is a waiter,
/// it is removed from the list (the next sem_wait spin will observe the
/// incremented value and succeed).
/// Returns 0 on success, -9 (EBADF) for an invalid fd.
pub fn sem_post(fd: i32) -> i32 {
    if fd < SEM_FD_BASE {
        return -9;
    }
    let idx = (fd - SEM_FD_BASE) as usize;
    if idx >= MAX_POSIX_SEMS {
        return -9;
    }
    let mut table = POSIX_SEMS.lock();
    let slot = &mut table[idx];
    if !slot.in_use {
        return -9;
    }
    if slot.value < SEM_VALUE_MAX {
        slot.value = slot.value.saturating_add(1);
    }
    // Wake the first waiter (stub: remove from list; spin loop will see value).
    if slot.waiter_count > 0 {
        posix_pop_waiter(slot);
    }
    0
}

/// sem_getvalue — read the current semaphore value.
///
/// Writes the value to `*out`.  Per POSIX, if there are waiters the value
/// may be reported as a negative number equal to -(waiter_count).
/// Returns 0 on success, -9 (EBADF) for an invalid fd.
pub fn sem_getvalue(fd: i32, out: &mut i32) -> i32 {
    if fd < SEM_FD_BASE {
        return -9;
    }
    let idx = (fd - SEM_FD_BASE) as usize;
    if idx >= MAX_POSIX_SEMS {
        return -9;
    }
    let table = POSIX_SEMS.lock();
    let slot = &table[idx];
    if !slot.in_use {
        return -9;
    }
    if slot.waiter_count > 0 {
        // POSIX extension: negative value indicates queued waiters.
        *out = 0i32.wrapping_sub(slot.waiter_count as i32);
    } else {
        *out = slot.value as i32;
    }
    0
}

/// Return true if `fd` is a POSIX semaphore fd issued by this subsystem.
pub fn sem_is_fd(fd: i32) -> bool {
    if fd < SEM_FD_BASE {
        return false;
    }
    let idx = (fd - SEM_FD_BASE) as usize;
    if idx >= MAX_POSIX_SEMS {
        return false;
    }
    let table = POSIX_SEMS.lock();
    table[idx].in_use
}

// ---------------------------------------------------------------------------
// System V semaphore public API
// ---------------------------------------------------------------------------

/// semget — get or create a SysV semaphore set.
///
/// `key == IPC_PRIVATE` (0): always creates a new anonymous set.
/// Otherwise: searches for an existing set with the given key; creates one
/// if IPC_CREAT is set.
///
/// Returns a semid (>= 1) on success, or a negative errno:
///   -17 (EEXIST)   — IPC_CREAT|IPC_EXCL and set already exists
///   -2  (ENOENT)   — key not found and IPC_CREAT not set
///   -22 (EINVAL)   — nsems == 0 or > SEMMSL
///   -12 (ENOMEM)   — table full
pub fn semget(key: i32, nsems: i32, flags: i32) -> i32 {
    if nsems < 0 || nsems as usize > SEMMSL {
        return -22; // EINVAL
    }

    let mut table = SEM_SETS.lock();

    // Search for an existing set with this key (skip IPC_PRIVATE).
    if key != IPC_PRIVATE {
        for i in 0..MAX_SEM_SETS {
            if table[i].active && table[i].key == key {
                // Found.
                if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                    return -17; // EEXIST
                }
                return table[i].semid;
            }
        }
        // Not found.
        if flags & IPC_CREAT == 0 {
            return -2; // ENOENT
        }
    }

    // nsems must be > 0 when creating.
    if nsems == 0 {
        return -22; // EINVAL
    }

    // Find a free slot.
    let mut free_idx: Option<usize> = None;
    for i in 0..MAX_SEM_SETS {
        if !table[i].active {
            free_idx = Some(i);
            break;
        }
    }
    let idx = match free_idx {
        Some(i) => i,
        None => return -12, // ENOMEM — table full
    };

    let semid = NEXT_SEMID.fetch_add(1, Ordering::Relaxed);

    let slot = &mut table[idx];
    *slot = SemSet::empty();
    slot.semid = semid;
    slot.key = key;
    slot.nsems = nsems as u16;
    slot.mode = (flags & 0o777) as u16;
    slot.active = true;

    semid
}

/// semop — perform semaphore operations atomically.
///
/// Processes up to `SEMOPM` operations from `ops`.  If any operation would
/// block and IPC_NOWAIT is not set, this function spins until the operation
/// can complete (simple busy-wait; up to 10 000 iterations per blocked op).
///
/// Returns 0 on success or a negative errno:
///   -22 (EINVAL)  — semid invalid or sem_num out of range
///   -11 (EAGAIN)  — IPC_NOWAIT set and operation would block
pub fn semop(semid: i32, ops: &[Sembuf]) -> i32 {
    if ops.is_empty() {
        return 0;
    }

    let op_count = if ops.len() > SEMOPM {
        SEMOPM
    } else {
        ops.len()
    };
    let pid = crate::process::getpid();

    // Validate semid and sem_num values up front.
    {
        let table = SEM_SETS.lock();
        let set = match find_set(&table, semid) {
            Some(s) => s,
            None => return -22, // EINVAL
        };
        for op in &ops[..op_count] {
            if op.sem_num as u16 >= set.nsems {
                return -22; // EINVAL
            }
        }
    }

    // Attempt the full atomic batch — spin until all ops can proceed together.
    let mut attempts = 0u32;
    loop {
        attempts = attempts.saturating_add(1);

        let mut table = SEM_SETS.lock();
        let set = match find_set_mut(&mut table, semid) {
            Some(s) => s,
            None => return -22,
        };

        // Phase 1: check whether all ops can proceed.
        let mut can_proceed = true;
        for op in &ops[..op_count] {
            let idx = op.sem_num as usize;
            let val = set.sems[idx].semval as i32;
            if op.sem_op < 0 {
                // Acquire: need val >= |sem_op|.
                if val < (-(op.sem_op as i32)) {
                    if op.sem_flg & IPC_NOWAIT != 0 {
                        return -11; // EAGAIN
                    }
                    can_proceed = false;
                    break;
                }
            } else if op.sem_op == 0 {
                // Wait for zero.
                if val != 0 {
                    if op.sem_flg & IPC_NOWAIT != 0 {
                        return -11; // EAGAIN
                    }
                    can_proceed = false;
                    break;
                }
            }
            // Positive sem_op always proceeds.
        }

        if !can_proceed {
            // Give up after 10 000 attempts to avoid infinite spinning.
            if attempts >= 10_000 {
                return -11; // EAGAIN (timeout equivalent)
            }
            drop(table);
            core::hint::spin_loop();
            continue;
        }

        // Phase 2: apply all operations.
        for op in &ops[..op_count] {
            let idx = op.sem_num as usize;
            let sem = &mut set.sems[idx];
            if op.sem_op > 0 {
                let new_val = (sem.semval as u32).saturating_add(op.sem_op as u32);
                sem.semval = if new_val > SEM_VALUE_MAX as u32 {
                    SEM_VALUE_MAX as u16
                } else {
                    new_val as u16
                };
            } else if op.sem_op < 0 {
                let dec = (-(op.sem_op as i32)) as u16;
                sem.semval = sem.semval.saturating_sub(dec);
            }
            // sem_op == 0: no change to value, just a check.
            sem.sempid = pid;
        }

        return 0;
    }
}

/// semctl — control operations on a SysV semaphore set.
///
/// Returns a value >= 0 on success, or a negative errno:
///   -22 (EINVAL)  — invalid semid, semnum, or cmd
///   -14 (EFAULT)  — arg pointer invalid (for GETALL/SETALL)
pub fn semctl(semid: i32, semnum: i32, cmd: i32, arg: u64) -> i64 {
    let mut table = SEM_SETS.lock();

    match cmd {
        IPC_RMID => {
            // Remove the semaphore set.
            for i in 0..MAX_SEM_SETS {
                if table[i].active && table[i].semid == semid {
                    table[i] = SemSet::empty();
                    return 0;
                }
            }
            -22 // EINVAL — not found
        }

        IPC_STAT => {
            // Just return 0; a real implementation would copy a semid_ds struct.
            match find_set(&table, semid) {
                Some(_) => 0,
                None => -22,
            }
        }

        GETVAL => {
            let set = match find_set(&table, semid) {
                Some(s) => s,
                None => return -22,
            };
            if semnum < 0 || semnum as u16 >= set.nsems {
                return -22;
            }
            set.sems[semnum as usize].semval as i64
        }

        SETVAL => {
            let new_val = arg as u32;
            if new_val > SEM_VALUE_MAX {
                return -22; // EINVAL — value too large
            }
            let set = match find_set_mut(&mut table, semid) {
                Some(s) => s,
                None => return -22,
            };
            if semnum < 0 || semnum as u16 >= set.nsems {
                return -22;
            }
            set.sems[semnum as usize].semval = new_val as u16;
            0
        }

        GETPID => {
            let set = match find_set(&table, semid) {
                Some(s) => s,
                None => return -22,
            };
            if semnum < 0 || semnum as u16 >= set.nsems {
                return -22;
            }
            set.sems[semnum as usize].sempid as i64
        }

        GETNCNT => {
            let set = match find_set(&table, semid) {
                Some(s) => s,
                None => return -22,
            };
            if semnum < 0 || semnum as u16 >= set.nsems {
                return -22;
            }
            set.sems[semnum as usize].semncnt as i64
        }

        GETZCNT => {
            let set = match find_set(&table, semid) {
                Some(s) => s,
                None => return -22,
            };
            if semnum < 0 || semnum as u16 >= set.nsems {
                return -22;
            }
            set.sems[semnum as usize].semzcnt as i64
        }

        GETALL => {
            // Copy all semvals into user buffer at address `arg`.
            // Each value is written as u16 (2 bytes).
            let set = match find_set(&table, semid) {
                Some(s) => s,
                None => return -22,
            };
            if arg == 0 {
                return -14; // EFAULT
            }
            let nsems = set.nsems as usize;
            let buf = arg as *mut u16;
            for i in 0..nsems {
                // Safety: caller guarantees the buffer is valid for nsems u16 values.
                unsafe {
                    core::ptr::write_volatile(buf.add(i), set.sems[i].semval);
                }
            }
            0
        }

        SETALL => {
            // Read all semvals from user buffer at address `arg`.
            if arg == 0 {
                return -14; // EFAULT
            }
            let set = match find_set_mut(&mut table, semid) {
                Some(s) => s,
                None => return -22,
            };
            let nsems = set.nsems as usize;
            let buf = arg as *const u16;
            for i in 0..nsems {
                let val = unsafe { core::ptr::read_volatile(buf.add(i)) };
                set.sems[i].semval = if val as u32 > SEM_VALUE_MAX {
                    SEM_VALUE_MAX as u16
                } else {
                    val
                };
            }
            0
        }

        _ => -22, // EINVAL — unknown command
    }
}

// ---------------------------------------------------------------------------
// Internal table helpers
// ---------------------------------------------------------------------------

/// Find an immutable reference to a SemSet by semid.
fn find_set(table: &[SemSet; MAX_SEM_SETS], semid: i32) -> Option<&SemSet> {
    for i in 0..MAX_SEM_SETS {
        if table[i].active && table[i].semid == semid {
            return Some(&table[i]);
        }
    }
    None
}

/// Find a mutable reference to a SemSet by semid.
fn find_set_mut(table: &mut [SemSet; MAX_SEM_SETS], semid: i32) -> Option<&mut SemSet> {
    for i in 0..MAX_SEM_SETS {
        if table[i].active && table[i].semid == semid {
            return Some(&mut table[i]);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Syscall-facing integer wrappers
// ---------------------------------------------------------------------------
//
// These thin wrappers convert raw u64 syscall arguments into typed calls and
// translate errors into the negative-errno convention used by syscall_dispatch.
//
// Syscall numbers assigned (Genesis-custom; original Linux numbers 58/59/66
// are already taken by SYS_RENAME/SYS_CHMOD/SYS_GETHOSTNAME):
//
//   sys_semget        = 320
//   sys_semop         = 321
//   sys_semctl        = 322
//   sys_sem_open      = 323
//   sys_sem_close     = 324
//   sys_sem_wait      = 325
//   sys_sem_post      = 326
//   sys_sem_trywait   = 327
//   sys_sem_timedwait = 328
//   sys_sem_getvalue  = 329
//   sys_sem_unlink    = 330

/// sys_semget — SYS_SEMGET (320).
///
/// `arg1` = key (i32), `arg2` = nsems (i32), `arg3` = flags (i32).
/// Returns semid as u64, or 0usize.wrapping_sub(errno) on error.
pub fn sys_semget(key: i32, nsems: i32, flags: i32) -> u64 {
    let r = semget(key, nsems, flags);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        r as u64
    }
}

/// sys_semop — SYS_SEMOP (321).
///
/// `semid` = semaphore set id, `ops_ptr` = pointer to `[Sembuf]` array,
/// `nops` = number of operations.
/// Returns 0 on success or 0usize.wrapping_sub(errno).
pub fn sys_semop(semid: i32, ops_ptr: *const Sembuf, nops: usize) -> u64 {
    if ops_ptr.is_null() || nops == 0 {
        return 0u64.wrapping_sub(22); // EINVAL
    }
    let capped = if nops > SEMOPM { SEMOPM } else { nops };
    // Safety: caller (syscall_dispatch) must validate the pointer before calling.
    let ops = unsafe { core::slice::from_raw_parts(ops_ptr, capped) };
    let r = semop(semid, ops);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        0
    }
}

/// sys_semctl — SYS_SEMCTL (322).
///
/// `semid`, `semnum`, `cmd`, `arg` map directly to `semctl()`.
/// Returns the semctl result (>=0) or 0usize.wrapping_sub(errno).
pub fn sys_semctl(semid: i32, semnum: i32, cmd: i32, arg: u64) -> u64 {
    let r = semctl(semid, semnum, cmd, arg);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        r as u64
    }
}

/// sys_sem_open — SYS_SEM_OPEN (323).
///
/// `name_ptr` / `name_len` describe the semaphore name in user space.
/// `flags` and `value` correspond to sem_open parameters.
/// Returns the fd or 0usize.wrapping_sub(errno).
pub fn sys_sem_open(name_ptr: *const u8, name_len: usize, flags: i32, value: u32) -> u64 {
    if name_ptr.is_null() || name_len == 0 || name_len > 64 {
        return 0u64.wrapping_sub(22); // EINVAL
    }
    // Safety: validated by check_user_ptr! in syscall_dispatch before reaching here.
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
    let r = sem_open(name, flags, value);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        r as u64
    }
}

/// sys_sem_close — SYS_SEM_CLOSE (324).
///
/// Returns 0 or 0usize.wrapping_sub(errno).
pub fn sys_sem_close(fd: i32) -> u64 {
    let r = sem_close(fd);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        0
    }
}

/// sys_sem_wait — SYS_SEM_WAIT (325).
///
/// Returns 0 or 0usize.wrapping_sub(errno).
pub fn sys_sem_wait(fd: i32) -> u64 {
    let r = sem_wait(fd);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        0
    }
}

/// sys_sem_post — SYS_SEM_POST (326).
///
/// Returns 0 or 0usize.wrapping_sub(errno).
pub fn sys_sem_post(fd: i32) -> u64 {
    let r = sem_post(fd);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        0
    }
}

/// sys_sem_trywait — SYS_SEM_TRYWAIT (327).
///
/// Returns 0 or 0usize.wrapping_sub(errno).
pub fn sys_sem_trywait(fd: i32) -> u64 {
    let r = sem_trywait(fd);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        0
    }
}

/// sys_sem_timedwait — SYS_SEM_TIMEDWAIT (328).
///
/// `timeout_ms` is the deadline in milliseconds.
/// Returns 0 or 0usize.wrapping_sub(errno).
pub fn sys_sem_timedwait(fd: i32, timeout_ms: u64) -> u64 {
    let r = sem_timedwait(fd, timeout_ms);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        0
    }
}

/// sys_sem_getvalue — SYS_SEM_GETVALUE (329).
///
/// `out_ptr` is a user-space pointer to an i32 that receives the value.
/// Returns 0 or 0usize.wrapping_sub(errno).
pub fn sys_sem_getvalue(fd: i32, out_ptr: *mut i32) -> u64 {
    if out_ptr.is_null() {
        return 0u64.wrapping_sub(14); // EFAULT
    }
    let mut val: i32 = 0;
    let r = sem_getvalue(fd, &mut val);
    if r < 0 {
        return 0u64.wrapping_sub((-r) as u64);
    }
    // Safety: validated by check_user_ptr! in syscall_dispatch before reaching here.
    unsafe { core::ptr::write_volatile(out_ptr, val) };
    0
}

/// sys_sem_unlink — SYS_SEM_UNLINK (330).
///
/// `name_ptr` / `name_len` describe the semaphore name in user space.
/// Returns 0 or 0usize.wrapping_sub(errno).
pub fn sys_sem_unlink(name_ptr: *const u8, name_len: usize) -> u64 {
    if name_ptr.is_null() || name_len == 0 || name_len > 64 {
        return 0u64.wrapping_sub(22); // EINVAL
    }
    // Safety: validated by check_user_ptr! in syscall_dispatch before reaching here.
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
    let r = sem_unlink(name);
    if r < 0 {
        0u64.wrapping_sub((-r) as u64)
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the semaphore subsystem.
///
/// Both tables are const-initialised; this function only emits a startup
/// log message.
pub fn init() {
    serial_println!(
        "    [sem] semaphore subsystem ready: {} POSIX slots, {} SysV sets, {} sems/set",
        MAX_POSIX_SEMS,
        MAX_SEM_SETS,
        SEMMSL
    );
}
