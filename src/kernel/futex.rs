/// futex — fast userspace mutex
///
/// Implements the kernel half of POSIX futex operations:
///   FUTEX_WAIT:    put caller to sleep if *uaddr == expected
///   FUTEX_WAKE:    wake up to N waiters on uaddr
///   FUTEX_REQUEUE: wake N1, requeue rest to uaddr2 (mutex handoff)
///   FUTEX_CMP_REQUEUE: conditional requeue
///   FUTEX_WAKE_OP: wake+atomic-op combo (used by pthread_cond_broadcast)
///
/// Design:
///   - Hash table of 64 wait queues, keyed by addr % 64
///   - Each queue holds up to 16 waiters
///   - Waiter is identified by (pid, addr)
///   - No actual sleeping — in this bare-metal stub, "waiting" is recorded
///     and wake returns a count; the scheduler integration is a TODO
///
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const FUTEX_HASH_SIZE: usize = 64;
pub const FUTEX_BUCKET_DEPTH: usize = 16;

// FUTEX_OP operations for FUTEX_WAKE_OP
pub const FUTEX_OP_SET: u8 = 0;
pub const FUTEX_OP_ADD: u8 = 1;
pub const FUTEX_OP_OR: u8 = 2;
pub const FUTEX_OP_ANDN: u8 = 3;
pub const FUTEX_OP_XOR: u8 = 4;

// FUTEX_OP comparisons
pub const FUTEX_OP_CMP_EQ: u8 = 0;
pub const FUTEX_OP_CMP_NE: u8 = 1;
pub const FUTEX_OP_CMP_LT: u8 = 2;
pub const FUTEX_OP_CMP_LE: u8 = 3;
pub const FUTEX_OP_CMP_GT: u8 = 4;
pub const FUTEX_OP_CMP_GE: u8 = 5;

// Error codes
pub const FUTEX_OK: i32 = 0;
pub const FUTEX_EAGAIN: i32 = -11; // value mismatch — retry
pub const FUTEX_ETIMEDOUT: i32 = -110;
pub const FUTEX_EINVAL: i32 = -22;

// ---------------------------------------------------------------------------
// Waiter entry
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct FutexWaiter {
    pub pid: u32,
    pub addr: u64,   // userspace futex address (kernel-side key)
    pub bitset: u32, // FUTEX_BITSET mask (0xFFFFFFFF = any)
    pub valid: bool,
}

impl FutexWaiter {
    pub const fn empty() -> Self {
        FutexWaiter {
            pid: 0,
            addr: 0,
            bitset: 0xFFFFFFFF,
            valid: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Hash bucket
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct FutexBucket {
    pub waiters: [FutexWaiter; FUTEX_BUCKET_DEPTH],
    pub count: u8,
}

impl FutexBucket {
    pub const fn empty() -> Self {
        const FW: FutexWaiter = FutexWaiter::empty();
        FutexBucket {
            waiters: [FW; FUTEX_BUCKET_DEPTH],
            count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static hash table
// ---------------------------------------------------------------------------

// We can't put an array of non-Copy types in a Mutex with a const initializer,
// but FutexBucket is Copy so this is fine.
static FUTEX_TABLE: Mutex<[FutexBucket; FUTEX_HASH_SIZE]> =
    Mutex::new([FutexBucket::empty(); FUTEX_HASH_SIZE]);

// ---------------------------------------------------------------------------
// Hash function: addr → bucket index
// ---------------------------------------------------------------------------

#[inline(always)]
fn hash(addr: u64) -> usize {
    // Mix address bits and fold into bucket range
    let h = addr.wrapping_mul(0x9e3779b97f4a7c15);
    (h as usize) % FUTEX_HASH_SIZE
}

// ---------------------------------------------------------------------------
// FUTEX_WAIT
// ---------------------------------------------------------------------------

/// Record a waiter for `addr` if `*uaddr_val == expected`. Returns:
///   FUTEX_OK      — waiter enqueued (will be woken by FUTEX_WAKE)
///   FUTEX_EAGAIN  — value mismatch; caller should retry
///   FUTEX_EINVAL  — bucket full
///
/// In a full kernel this would actually block the calling task; here we
/// simply enqueue the waiter record for FUTEX_WAKE to find.
pub fn futex_wait(pid: u32, addr: u64, uaddr_val: u32, expected: u32, bitset: u32) -> i32 {
    if uaddr_val != expected {
        return FUTEX_EAGAIN;
    }
    let bucket = hash(addr);
    let mut table = FUTEX_TABLE.lock();
    let b = &mut table[bucket];
    // Check not already waiting
    let mut i = 0usize;
    while i < FUTEX_BUCKET_DEPTH {
        if b.waiters[i].valid && b.waiters[i].pid == pid && b.waiters[i].addr == addr {
            return FUTEX_OK; // already enqueued
        }
        i = i.saturating_add(1);
    }
    // Find empty slot
    let mut i = 0usize;
    while i < FUTEX_BUCKET_DEPTH {
        if !b.waiters[i].valid {
            b.waiters[i] = FutexWaiter {
                pid,
                addr,
                bitset,
                valid: true,
            };
            b.count = b.count.saturating_add(1);
            return FUTEX_OK;
        }
        i = i.saturating_add(1);
    }
    FUTEX_EINVAL // bucket full
}

// ---------------------------------------------------------------------------
// FUTEX_WAKE
// ---------------------------------------------------------------------------

/// Wake up to `n` waiters on `addr` matching `bitset`. Returns count woken.
pub fn futex_wake(addr: u64, n: u32, bitset: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let bucket = hash(addr);
    let mut table = FUTEX_TABLE.lock();
    let b = &mut table[bucket];
    let mut woken = 0u32;
    let mut i = 0usize;
    while i < FUTEX_BUCKET_DEPTH && woken < n {
        if b.waiters[i].valid && b.waiters[i].addr == addr && (b.waiters[i].bitset & bitset) != 0 {
            b.waiters[i] = FutexWaiter::empty();
            b.count = b.count.saturating_sub(1);
            woken = woken.saturating_add(1);
        }
        i = i.saturating_add(1);
    }
    woken
}

// ---------------------------------------------------------------------------
// FUTEX_REQUEUE
// ---------------------------------------------------------------------------

/// Wake `n_wake` waiters on `addr`, then move up to `n_requeue` remaining
/// waiters from `addr` to `addr2`.  Returns total woken count.
pub fn futex_requeue(addr: u64, addr2: u64, n_wake: u32, n_requeue: u32) -> u32 {
    // First wake n_wake
    let woken = futex_wake(addr, n_wake, 0xFFFFFFFF);

    // Now requeue up to n_requeue from addr → addr2
    if n_requeue == 0 {
        return woken;
    }
    let bk1 = hash(addr);
    let bk2 = hash(addr2);

    // If same bucket, do in-place update
    if bk1 == bk2 {
        let mut table = FUTEX_TABLE.lock();
        let b = &mut table[bk1];
        let mut moved = 0u32;
        let mut i = 0usize;
        while i < FUTEX_BUCKET_DEPTH && moved < n_requeue {
            if b.waiters[i].valid && b.waiters[i].addr == addr {
                b.waiters[i].addr = addr2;
                moved = moved.saturating_add(1);
            }
            i = i.saturating_add(1);
        }
    } else {
        // Different buckets — collect, remove from bk1, insert into bk2
        // To avoid double-lock: copy data out first, then update
        let mut to_move: [FutexWaiter; FUTEX_BUCKET_DEPTH] =
            [FutexWaiter::empty(); FUTEX_BUCKET_DEPTH];
        let mut n_to_move = 0usize;
        {
            let mut table = FUTEX_TABLE.lock();
            let b = &mut table[bk1];
            let mut i = 0usize;
            while i < FUTEX_BUCKET_DEPTH && (n_to_move as u32) < n_requeue {
                if b.waiters[i].valid && b.waiters[i].addr == addr {
                    to_move[n_to_move] = b.waiters[i];
                    to_move[n_to_move].addr = addr2;
                    b.waiters[i] = FutexWaiter::empty();
                    b.count = b.count.saturating_sub(1);
                    n_to_move = n_to_move.saturating_add(1);
                }
                i = i.saturating_add(1);
            }
        }
        {
            let mut table = FUTEX_TABLE.lock();
            let b = &mut table[bk2];
            let mut k = 0usize;
            while k < n_to_move {
                // Find empty slot in target bucket
                let mut j = 0usize;
                while j < FUTEX_BUCKET_DEPTH {
                    if !b.waiters[j].valid {
                        b.waiters[j] = to_move[k];
                        b.count = b.count.saturating_add(1);
                        break;
                    }
                    j = j.saturating_add(1);
                }
                k = k.saturating_add(1);
            }
        }
    }
    woken
}

// ---------------------------------------------------------------------------
// FUTEX_CMP_REQUEUE
// ---------------------------------------------------------------------------

/// Like requeue but only proceeds if `*uaddr_val == expected`.
pub fn futex_cmp_requeue(
    addr: u64,
    addr2: u64,
    n_wake: u32,
    n_requeue: u32,
    uaddr_val: u32,
    expected: u32,
) -> i32 {
    if uaddr_val != expected {
        return FUTEX_EAGAIN;
    }
    futex_requeue(addr, addr2, n_wake, n_requeue) as i32
}

// ---------------------------------------------------------------------------
// FUTEX_WAKE_OP
// ---------------------------------------------------------------------------

/// Wake `n_wake` on `addr`, then apply op to `*uaddr2_val`, then if the
/// comparison holds wake `n_wake2` on `addr2`.
/// Returns total woken count.
pub fn futex_wake_op(
    addr: u64,
    addr2: u64,
    n_wake: u32,
    n_wake2: u32,
    uaddr2_val: u32,
    op: u8,
    op_arg: u32,
    cmp: u8,
    cmp_arg: u32,
) -> u32 {
    let woken = futex_wake(addr, n_wake, 0xFFFFFFFF);

    // Apply op to old_val (simulated — we don't actually touch userspace here)
    let old_val = uaddr2_val;
    let _new_val = match op {
        FUTEX_OP_SET => op_arg,
        FUTEX_OP_ADD => old_val.wrapping_add(op_arg),
        FUTEX_OP_OR => old_val | op_arg,
        FUTEX_OP_ANDN => old_val & !op_arg,
        FUTEX_OP_XOR => old_val ^ op_arg,
        _ => old_val,
    };

    // Check comparison against old_val
    let cond = match cmp {
        FUTEX_OP_CMP_EQ => old_val == cmp_arg,
        FUTEX_OP_CMP_NE => old_val != cmp_arg,
        FUTEX_OP_CMP_LT => old_val < cmp_arg,
        FUTEX_OP_CMP_LE => old_val <= cmp_arg,
        FUTEX_OP_CMP_GT => old_val > cmp_arg,
        FUTEX_OP_CMP_GE => old_val >= cmp_arg,
        _ => false,
    };

    let mut total = woken;
    if cond {
        total = total.saturating_add(futex_wake(addr2, n_wake2, 0xFFFFFFFF));
    }
    total
}

// ---------------------------------------------------------------------------
// Cancel: remove all waiters for a dying process
// ---------------------------------------------------------------------------

pub fn futex_cancel_pid(pid: u32) {
    let mut table = FUTEX_TABLE.lock();
    let mut b = 0usize;
    while b < FUTEX_HASH_SIZE {
        let mut i = 0usize;
        while i < FUTEX_BUCKET_DEPTH {
            if table[b].waiters[i].valid && table[b].waiters[i].pid == pid {
                table[b].waiters[i] = FutexWaiter::empty();
                table[b].count = table[b].count.saturating_sub(1);
            }
            i = i.saturating_add(1);
        }
        b = b.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub fn futex_waiter_count() -> usize {
    let table = FUTEX_TABLE.lock();
    let mut n = 0usize;
    let mut b = 0usize;
    while b < FUTEX_HASH_SIZE {
        n = n.saturating_add(table[b].count as usize);
        b = b.saturating_add(1);
    }
    n
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "[futex] fast userspace mutex subsystem initialized (buckets={})",
        FUTEX_HASH_SIZE
    );
}
