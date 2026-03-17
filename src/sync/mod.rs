/// Synchronization primitives for Genesis — built from scratch
///
/// Provides:
///   - Mutex<T>: spinlock-based mutual exclusion
///   - MutexGuard<T>: RAII lock guard with Deref/DerefMut
///   - Once<T>: one-time initialization cell
///   - Futex: fast userspace mutex (wait/wake)
///   - RCU: read-copy-update for lock-free reads
///   - Workqueue: deferred work execution
///
/// No external crates. Pure core::sync::atomic.
pub mod futex;
pub mod rcu;
pub mod workqueue;

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Ticketed spinlock — fair FIFO ordering under contention
// ---------------------------------------------------------------------------

/// A ticket-based spinlock that guarantees FIFO acquisition order.
///
/// Unlike a plain test-and-set spinlock where any waiter can steal the lock,
/// a ticket lock assigns each caller a sequential ticket and serves them in
/// order.  This prevents starvation when many CPUs contend simultaneously,
/// e.g. the process-table or run-queue locks during SMP scheduling.
///
/// Usage matches `Mutex<T>` but uses `TicketGuard` instead of `MutexGuard`.
// hot struct: process table and run-queue both use this; acquired ~1K/s each
#[repr(C, align(64))] // one cache line — next_ticket and now_serving on same line
pub struct TicketLock<T: ?Sized> {
    /// Next ticket to hand out; incremented atomically on every lock() call.
    next_ticket: AtomicU32,
    /// The ticket currently being served; incremented on every unlock.
    now_serving: AtomicU32,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Sync for TicketLock<T> {}
unsafe impl<T: ?Sized + Send> Send for TicketLock<T> {}

impl<T> TicketLock<T> {
    pub const fn new(value: T) -> Self {
        TicketLock {
            next_ticket: AtomicU32::new(0),
            now_serving: AtomicU32::new(0),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquire the lock.  Blocks (spin) until this caller's ticket is served.
    // hot path: called on every PROCESS_TABLE and RUN_QUEUE access
    #[inline]
    pub fn lock(&self) -> TicketGuard<'_, T> {
        // Atomically claim the next ticket.
        let ticket = self.next_ticket.fetch_add(1, Ordering::Relaxed);
        // Spin until our ticket is called.
        while self.now_serving.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
        }
        TicketGuard { lock: self }
    }

    /// Try to acquire the lock without spinning.
    /// Returns None if the lock is already held.
    #[inline]
    pub fn try_lock(&self) -> Option<TicketGuard<'_, T>> {
        let serving = self.now_serving.load(Ordering::Acquire);
        let next = self.next_ticket.load(Ordering::Relaxed);
        if serving == next {
            // Lock is free — try to claim the next ticket atomically.
            if self
                .next_ticket
                .compare_exchange(
                    next,
                    next.wrapping_add(1),
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                return Some(TicketGuard { lock: self });
            }
        }
        None
    }
}

/// RAII guard for `TicketLock`.  Releases the lock (advances `now_serving`) on drop.
pub struct TicketGuard<'a, T: ?Sized + 'a> {
    lock: &'a TicketLock<T>,
}

impl<T: ?Sized> Deref for TicketGuard<'_, T> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for TicketGuard<'_, T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for TicketGuard<'_, T> {
    // hot path: called on every lock release — must be inlined to avoid call overhead
    #[inline(always)]
    fn drop(&mut self) {
        // Advance now_serving to wake the next waiter.
        // Wrapping add: ticket numbers wrap after 2^32 acquisitions; that is
        // safe because no more than 2^32 waiters can exist simultaneously.
        self.lock.now_serving.fetch_add(1, Ordering::Release);
    }
}

/// A spinlock-based mutex.
///
/// Spins in a tight loop until the lock is acquired.
/// Suitable for short critical sections in a kernel where
/// we can't block/sleep.
pub struct Mutex<T: ?Sized> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// Safety: Mutex provides synchronization via atomic spinlock
unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}

impl<T> Mutex<T> {
    /// Create a new unlocked mutex.
    pub const fn new(value: T) -> Self {
        Mutex {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquire the lock, spinning until available.
    /// Returns a guard that releases the lock on drop.
    pub fn lock(&self) -> MutexGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Spin — hint to CPU that we're in a spin-wait loop
            core::hint::spin_loop();
        }
        MutexGuard { mutex: self }
    }
}

/// RAII guard — holds the lock and releases it on drop.
pub struct MutexGuard<'a, T: ?Sized + 'a> {
    mutex: &'a Mutex<T>,
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}

/// One-time initialization cell.
///
/// Stores a value that is computed exactly once, on first access.
/// Thread-safe via spinlock.
pub struct Once<T> {
    done: AtomicBool,
    data: UnsafeCell<Option<T>>,
    lock: AtomicBool,
}

unsafe impl<T: Send + Sync> Sync for Once<T> {}
unsafe impl<T: Send> Send for Once<T> {}

impl<T> Once<T> {
    pub const fn new() -> Self {
        Once {
            done: AtomicBool::new(false),
            data: UnsafeCell::new(None),
            lock: AtomicBool::new(false),
        }
    }

    /// Initialize the cell if not already done, then return a reference.
    pub fn call_once<F: FnOnce() -> T>(&self, f: F) -> &T {
        if !self.done.load(Ordering::Acquire) {
            // Acquire init lock
            while self
                .lock
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                core::hint::spin_loop();
            }

            // Double-check after acquiring lock
            if !self.done.load(Ordering::Relaxed) {
                unsafe {
                    *self.data.get() = Some(f());
                }
                self.done.store(true, Ordering::Release);
            }

            self.lock.store(false, Ordering::Release);
        }

        // Safety: data is guaranteed to be Some after done is set to true above
        match unsafe { (*self.data.get()).as_ref() } {
            Some(v) => v,
            None => {
                // This should never happen: done=true implies data=Some
                // If it does, we cannot return a valid reference; loop forever
                // to signal the bug rather than producing UB
                loop {
                    core::hint::spin_loop();
                }
            }
        }
    }

    /// Get a reference if already initialized.
    pub fn get(&self) -> Option<&T> {
        if self.done.load(Ordering::Acquire) {
            unsafe { (*self.data.get()).as_ref() }
        } else {
            None
        }
    }
}
