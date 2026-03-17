use super::{NeuralSignal, Q16};
use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Priority queue for neural signals
///
/// Part of the AIOS neural bus layer. Implements a max-heap priority
/// queue for NeuralSignal objects ordered by their Q16 strength field.
/// Higher strength signals are dequeued first.
///
/// The queue also implements:
///   - Aging: signals that have been waiting too long get their
///     effective priority boosted to prevent starvation
///   - Capacity limiting: when the queue is full, the lowest-priority
///     signal is dropped
///   - Bulk drain: efficiently extract the top-N signals
use alloc::vec::Vec;

/// How much effective priority increases per aging tick (Q16 units)
const AGING_BOOST: Q16 = 655; // ~0.01 per tick

/// Priority-ordered signal queue (higher Q16 = higher priority)
pub struct PriorityQueue {
    /// Binary max-heap of signals
    pub heap: Vec<NeuralSignal>,
    /// Maximum number of signals the queue can hold
    pub max_size: usize,
    /// Number of signals dropped due to capacity overflow
    pub dropped_count: u64,
    /// Global aging tick counter
    pub aging_ticks: u64,
    /// Aging boost per tick per signal (Q16)
    pub aging_rate: Q16,
    /// Total signals ever enqueued
    pub total_enqueued: u64,
    /// Total signals ever dequeued
    pub total_dequeued: u64,
}

impl PriorityQueue {
    /// Create a new priority queue with the given capacity.
    pub fn new(max_size: usize) -> Self {
        PriorityQueue {
            heap: Vec::with_capacity(max_size),
            max_size,
            dropped_count: 0,
            aging_ticks: 0,
            aging_rate: AGING_BOOST,
            total_enqueued: 0,
            total_dequeued: 0,
        }
    }

    /// Push a signal into the priority queue.
    ///
    /// If the queue is full, compares the new signal's strength with
    /// the minimum in the heap. If the new signal is stronger, it
    /// replaces the weakest; otherwise it is dropped.
    pub fn push(&mut self, signal: NeuralSignal) {
        self.total_enqueued = self.total_enqueued.saturating_add(1);

        if self.heap.len() < self.max_size {
            self.heap.push(signal);
            self.sift_up(self.heap.len() - 1);
        } else {
            // Find the minimum element (it's somewhere in the leaves)
            let min_idx = self.find_min_idx();
            if signal.strength > self.heap[min_idx].strength {
                // Replace the minimum with the new signal
                self.heap[min_idx] = signal;
                // The replaced element might need to move up or down
                self.sift_up(min_idx);
                self.sift_down(min_idx);
                self.dropped_count = self.dropped_count.saturating_add(1);
            } else {
                // New signal is weaker; drop it
                self.dropped_count = self.dropped_count.saturating_add(1);
            }
        }
    }

    /// Pop the highest-priority signal from the queue.
    pub fn pop_highest(&mut self) -> Option<NeuralSignal> {
        if self.heap.is_empty() {
            return None;
        }

        self.total_dequeued = self.total_dequeued.saturating_add(1);
        let last_idx = self.heap.len() - 1;

        if last_idx == 0 {
            return self.heap.pop();
        }

        // Swap root with last element, then remove and sift down
        self.heap.swap(0, last_idx);
        let result = self.heap.pop();
        if !self.heap.is_empty() {
            self.sift_down(0);
        }
        result
    }

    /// Peek at the highest-priority signal without removing it.
    pub fn peek(&self) -> Option<&NeuralSignal> {
        self.heap.first()
    }

    /// Number of signals in the queue.
    pub fn len(&self) -> usize {
        self.heap.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }

    /// Whether the queue is at capacity.
    pub fn is_full(&self) -> bool {
        self.heap.len() >= self.max_size
    }

    /// Drain up to `n` highest-priority signals.
    pub fn drain_top(&mut self, n: usize) -> Vec<NeuralSignal> {
        let count = n.min(self.heap.len());
        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            if let Some(sig) = self.pop_highest() {
                result.push(sig);
            } else {
                break;
            }
        }
        result
    }

    /// Apply aging: boost the effective strength of all waiting signals.
    /// This prevents low-priority signals from being starved forever.
    pub fn age_tick(&mut self) {
        self.aging_ticks = self.aging_ticks.saturating_add(1);
        let boost = self.aging_rate;

        for signal in self.heap.iter_mut() {
            // Boost strength but cap at Q16_ONE
            signal.strength = (signal.strength + boost).min(super::Q16_ONE);
        }

        // Rebuild heap after modifying priorities
        self.rebuild_heap();
    }

    /// Remove all signals from a specific source node.
    pub fn remove_source(&mut self, source_node: u16) -> usize {
        let before = self.heap.len();
        self.heap.retain(|s| s.source_node != source_node);
        let removed = before - self.heap.len();
        if removed > 0 {
            self.rebuild_heap();
        }
        removed
    }

    /// Remove all signals of a specific kind.
    pub fn remove_kind(&mut self, kind: super::SignalKind) -> usize {
        let before = self.heap.len();
        self.heap.retain(|s| s.kind != kind);
        let removed = before - self.heap.len();
        if removed > 0 {
            self.rebuild_heap();
        }
        removed
    }

    /// Clear the entire queue.
    pub fn clear(&mut self) {
        self.heap.clear();
    }

    // ── Heap operations ─────────────────────────────────────────────

    /// Sift element at `idx` up towards the root.
    fn sift_up(&mut self, mut idx: usize) {
        while idx > 0 {
            let parent = (idx - 1) / 2;
            if self.heap[idx].strength > self.heap[parent].strength {
                self.heap.swap(idx, parent);
                idx = parent;
            } else {
                break;
            }
        }
    }

    /// Sift element at `idx` down towards the leaves.
    fn sift_down(&mut self, mut idx: usize) {
        let len = self.heap.len();
        loop {
            let left = 2 * idx + 1;
            let right = 2 * idx + 2;
            let mut largest = idx;

            if left < len && self.heap[left].strength > self.heap[largest].strength {
                largest = left;
            }
            if right < len && self.heap[right].strength > self.heap[largest].strength {
                largest = right;
            }

            if largest != idx {
                self.heap.swap(idx, largest);
                idx = largest;
            } else {
                break;
            }
        }
    }

    /// Find the index of the minimum element.
    /// In a max-heap, the minimum is among the leaves (last half).
    fn find_min_idx(&self) -> usize {
        if self.heap.is_empty() {
            return 0;
        }
        let start = self.heap.len() / 2;
        let mut min_idx = start;
        let mut min_val = self.heap[start].strength;
        for i in (start + 1)..self.heap.len() {
            if self.heap[i].strength < min_val {
                min_val = self.heap[i].strength;
                min_idx = i;
            }
        }
        min_idx
    }

    /// Rebuild the heap from scratch (O(n)).
    fn rebuild_heap(&mut self) {
        let len = self.heap.len();
        if len <= 1 {
            return;
        }
        // Start from the last internal node and sift down
        let mut i = len / 2;
        loop {
            self.sift_down(i);
            if i == 0 {
                break;
            }
            i -= 1;
        }
    }

    /// Get the average strength of all signals in the queue.
    pub fn avg_strength(&self) -> Q16 {
        if self.heap.is_empty() {
            return 0;
        }
        let sum: i64 = self.heap.iter().map(|s| s.strength as i64).sum();
        (sum / self.heap.len() as i64) as Q16
    }

    /// Get the spread (max - min) of signal strengths.
    pub fn strength_spread(&self) -> Q16 {
        if self.heap.is_empty() {
            return 0;
        }
        let max_s = self.heap.iter().map(|s| s.strength).max().unwrap_or(0);
        let min_s = self.heap.iter().map(|s| s.strength).min().unwrap_or(0);
        max_s - min_s
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct PriorityState {
    queue: PriorityQueue,
}

static PRIORITY_QUEUE: Mutex<Option<PriorityState>> = Mutex::new(None);

const DEFAULT_MAX_SIZE: usize = 512;

pub fn init() {
    let queue = PriorityQueue::new(DEFAULT_MAX_SIZE);
    let mut guard = PRIORITY_QUEUE.lock();
    *guard = Some(PriorityState { queue });
    serial_println!(
        "    [priority] Priority queue subsystem initialised (max={})",
        DEFAULT_MAX_SIZE
    );
}

/// Push a signal into the global priority queue.
pub fn push_global(signal: NeuralSignal) {
    let mut guard = PRIORITY_QUEUE.lock();
    if let Some(state) = guard.as_mut() {
        state.queue.push(signal);
    }
}

/// Pop the highest-priority signal from the global queue.
pub fn pop_global() -> Option<NeuralSignal> {
    let mut guard = PRIORITY_QUEUE.lock();
    if let Some(state) = guard.as_mut() {
        state.queue.pop_highest()
    } else {
        None
    }
}
