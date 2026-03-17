/// Bottom-half interrupt processing (software IRQ handlers).
///
/// Part of the AIOS kernel.
use core::sync::atomic::{AtomicU32, Ordering};

/// Maximum number of softirq vectors.
pub const NR_SOFTIRQS: usize = 10;

/// Well-known softirq vector indices.
pub const TIMER_SOFTIRQ: usize = 0;
pub const NET_TX_SOFTIRQ: usize = 1;
pub const NET_RX_SOFTIRQ: usize = 2;
pub const SCHED_SOFTIRQ: usize = 7;

/// Per-CPU pending softirq bitmask.
pub struct SoftirqState {
    pub pending: AtomicU32,
}

impl SoftirqState {
    pub fn new() -> Self {
        SoftirqState {
            pending: AtomicU32::new(0),
        }
    }
}

/// Global softirq handler table. Index = softirq vector number.
/// Handlers are plain function pointers to avoid Box/alloc at this layer.
static mut SOFTIRQ_HANDLERS: [Option<fn()>; NR_SOFTIRQS] = [None; NR_SOFTIRQS];

/// Global per-CPU pending bitmask (single-CPU for now).
static SOFTIRQ_PENDING: AtomicU32 = AtomicU32::new(0);

/// Raise a softirq (mark it pending for later execution).
pub fn raise_softirq(nr: usize) {
    if nr >= NR_SOFTIRQS {
        return;
    }
    SOFTIRQ_PENDING.fetch_or(1u32 << nr, Ordering::Release);
}

/// Process all pending softirqs (called after hardirq or from ksoftirqd).
pub fn do_softirq() {
    // Snapshot and clear the pending mask atomically.
    let pending = SOFTIRQ_PENDING.swap(0, Ordering::AcqRel);
    for i in 0..NR_SOFTIRQS {
        if pending & (1u32 << i) != 0 {
            // Safety: SOFTIRQ_HANDLERS is only mutated during init, before
            // SMP is running, so a single read here is safe.
            let handler = unsafe { SOFTIRQ_HANDLERS[i] };
            if let Some(h) = handler {
                h();
            }
        }
    }
}

/// Initialize the softirq subsystem.
pub fn init() {
    // TODO: Register default softirq handlers
}
