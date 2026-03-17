use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Continuous batching for concurrent requests
///
/// Part of the AIOS LLM layer. Implements a continuous batching scheduler
/// that multiplexes multiple inference requests through the model.
///
/// Unlike static batching where all requests start and end together,
/// continuous batching allows new requests to join the batch as slots
/// free up, and finished requests to leave without blocking others.
///
/// The scheduler maintains:
///   - A waiting queue of pending requests
///   - An active batch of requests currently being processed
///   - Per-request state (position, generated tokens, completion status)
///
/// Each `step()` call advances all active requests by one token,
/// retiring completed ones and promoting queued ones into free slots.
use alloc::vec::Vec;

/// Status of a batch request
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestStatus {
    /// Waiting in queue
    Queued,
    /// Currently in the active batch, prefill phase
    Prefilling,
    /// Currently in the active batch, generating tokens
    Generating,
    /// Generation complete
    Complete,
    /// Cancelled by the caller
    Cancelled,
}

/// A single inference request in the batch
pub struct BatchRequest {
    /// Unique request ID
    pub id: u64,
    /// Input (prompt) tokens
    pub tokens: Vec<u32>,
    /// Maximum new tokens to generate
    pub max_new_tokens: usize,
    /// Current generation position
    pub gen_pos: usize,
    /// Generated output tokens so far
    pub output_tokens: Vec<u32>,
    /// Current status
    pub status: RequestStatus,
    /// Priority (higher = more important)
    pub priority: u32,
    /// Arrival timestamp (monotonic counter)
    pub arrived_at: u64,
    /// Whether this request is in prefill or decode phase
    pub prefill_done: bool,
}

/// Manages continuous batching of inference requests
pub struct BatchScheduler {
    /// Waiting queue (sorted by priority, then arrival order)
    pub queue: Vec<BatchRequest>,
    /// Currently active batch
    pub active_batch: Vec<BatchRequest>,
    /// Maximum batch size (concurrent requests)
    pub max_batch_size: usize,
    /// Monotonic ID counter
    next_id: u64,
    /// Monotonic time counter
    counter: u64,
    /// Total requests processed to completion
    pub total_completed: u64,
    /// Total tokens generated across all requests
    pub total_tokens_generated: u64,
    /// Total steps executed
    pub total_steps: u64,
    /// Maximum sequence length (prompt + generation)
    pub max_seq_len: usize,
    /// Whether preemption is enabled (can swap out low-priority requests)
    pub preemption_enabled: bool,
    /// PRNG state for simulated token generation
    rng_state: u64,
}

impl BatchScheduler {
    /// Create a new batch scheduler.
    ///
    /// `max_batch_size` is the maximum number of requests processed
    /// concurrently in a single step.
    pub fn new(max_batch_size: usize) -> Self {
        serial_println!(
            "    [batch] Creating scheduler: max_batch={}",
            max_batch_size
        );
        BatchScheduler {
            queue: Vec::new(),
            active_batch: Vec::new(),
            max_batch_size,
            next_id: 1,
            counter: 0,
            total_completed: 0,
            total_tokens_generated: 0,
            total_steps: 0,
            max_seq_len: 4096,
            preemption_enabled: true,
            rng_state: 0xBA7C_DEAD_BEEF_0001u64,
        }
    }

    /// Submit a new inference request.
    ///
    /// The request is placed in the waiting queue and will be promoted
    /// to the active batch when a slot is available.
    pub fn submit(&mut self, req: BatchRequest) {
        let id = req.id;
        let n_tokens = req.tokens.len();
        let max_new = req.max_new_tokens;
        self.queue.push(req);

        // Sort queue by priority (descending), then by arrival (ascending)
        self.queue.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.arrived_at.cmp(&b.arrived_at))
        });

        serial_println!(
            "    [batch] Submitted request #{}: {} prompt tokens, max_new={}",
            id,
            n_tokens,
            max_new
        );
    }

    /// Submit a request from just tokens, auto-assigning an ID.
    pub fn submit_tokens(&mut self, tokens: Vec<u32>, max_new_tokens: usize) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.counter = self.counter.saturating_add(1);

        let req = BatchRequest {
            id,
            tokens,
            max_new_tokens,
            gen_pos: 0,
            output_tokens: Vec::new(),
            status: RequestStatus::Queued,
            priority: 0,
            arrived_at: self.counter,
            prefill_done: false,
        };
        self.submit(req);
        id
    }

    /// Execute one generation step.
    ///
    /// This promotes queued requests into the active batch, generates
    /// one token per active request, retires completed requests, and
    /// returns the (request_id, generated_token) pairs for this step.
    pub fn step(&mut self) -> Vec<(u64, u32)> {
        self.total_steps = self.total_steps.saturating_add(1);
        self.counter = self.counter.saturating_add(1);

        // ── Phase 1: Promote from queue to active batch ────────────
        self.promote_from_queue();

        // ── Phase 2: Prefill phase for new requests ────────────────
        for req in self.active_batch.iter_mut() {
            if !req.prefill_done {
                req.prefill_done = true;
                req.status = RequestStatus::Generating;
                // In a real system, we'd run the full prompt through
                // the model here. We mark it as done.
            }
        }

        // ── Phase 3: Generate one token per active request ─────────
        let mut outputs = Vec::new();

        for i in 0..self.active_batch.len() {
            if self.active_batch[i].status != RequestStatus::Generating {
                continue;
            }

            // Simulate token generation using a simple hash of context
            let token = self.generate_token_for(i);
            self.active_batch[i].output_tokens.push(token);
            self.active_batch[i].gen_pos += 1;
            outputs.push((self.active_batch[i].id, token));

            // Check completion
            if self.active_batch[i].gen_pos >= self.active_batch[i].max_new_tokens {
                self.active_batch[i].status = RequestStatus::Complete;
            }

            // Check for EOS token (token 0 or 2 are common EOS)
            if token == 0 || token == 2 {
                self.active_batch[i].status = RequestStatus::Complete;
            }
        }

        self.total_tokens_generated += outputs.len() as u64;

        // ── Phase 4: Retire completed requests ─────────────────────
        let completed_count = self
            .active_batch
            .iter()
            .filter(|r| r.status == RequestStatus::Complete || r.status == RequestStatus::Cancelled)
            .count();
        self.total_completed += completed_count as u64;
        self.active_batch
            .retain(|r| r.status == RequestStatus::Generating);

        // ── Phase 5: Preemption check ──────────────────────────────
        if self.preemption_enabled && !self.queue.is_empty() && !self.active_batch.is_empty() {
            self.maybe_preempt();
        }

        outputs
    }

    /// Promote requests from the queue into the active batch.
    fn promote_from_queue(&mut self) {
        while self.active_batch.len() < self.max_batch_size && !self.queue.is_empty() {
            let mut req = self.queue.remove(0);
            req.status = RequestStatus::Prefilling;
            serial_println!(
                "    [batch] Promoting request #{} to active batch (slot {}/{})",
                req.id,
                self.active_batch.len() + 1,
                self.max_batch_size
            );
            self.active_batch.push(req);
        }
    }

    /// Preempt: if a queued request has higher priority than an active one,
    /// swap them.
    fn maybe_preempt(&mut self) {
        if self.queue.is_empty() || self.active_batch.is_empty() {
            return;
        }

        let highest_queued_priority = self.queue[0].priority;

        // Find the lowest-priority active request
        let mut lowest_active_idx = 0;
        let mut lowest_active_priority = u32::MAX;
        for (i, req) in self.active_batch.iter().enumerate() {
            if req.priority < lowest_active_priority {
                lowest_active_priority = req.priority;
                lowest_active_idx = i;
            }
        }

        if highest_queued_priority > lowest_active_priority + 10 {
            // Preempt: move active request back to queue
            let mut preempted = self.active_batch.remove(lowest_active_idx);
            preempted.status = RequestStatus::Queued;
            serial_println!(
                "    [batch] Preempted request #{} (priority {})",
                preempted.id,
                preempted.priority
            );
            self.queue.push(preempted);
            self.queue.sort_by(|a, b| {
                b.priority
                    .cmp(&a.priority)
                    .then(a.arrived_at.cmp(&b.arrived_at))
            });
        }
    }

    /// Generate a simulated token based on the request context.
    /// In a real system, this calls the transformer forward pass.
    fn generate_token(&mut self, req: &BatchRequest) -> u32 {
        // Simple hash-based "generation" for simulation
        let mut h = req.id.wrapping_mul(0x9E3779B97F4A7C15);
        h ^= req.gen_pos as u64;
        if let Some(&last) = req.output_tokens.last() {
            h = h.wrapping_add(last as u64);
        } else if let Some(&last) = req.tokens.last() {
            h = h.wrapping_add(last as u64);
        }
        h ^= h >> 30;
        h = h.wrapping_mul(0xBF58476D1CE4E5B9);
        h ^= h >> 27;

        // Map to a token in range [1, 32000)
        (h % 31999 + 1) as u32
    }

    /// Generate a simulated token for the request at the given index in active_batch.
    /// This avoids the borrow checker issue of borrowing self and a request simultaneously.
    fn generate_token_for(&self, idx: usize) -> u32 {
        let req = &self.active_batch[idx];
        let mut h = req.id.wrapping_mul(0x9E3779B97F4A7C15);
        h ^= req.gen_pos as u64;
        if let Some(&last) = req.output_tokens.last() {
            h = h.wrapping_add(last as u64);
        } else if let Some(&last) = req.tokens.last() {
            h = h.wrapping_add(last as u64);
        }
        h ^= h >> 30;
        h = h.wrapping_mul(0xBF58476D1CE4E5B9);
        h ^= h >> 27;

        (h % 31999 + 1) as u32
    }

    /// Cancel a request by ID. Works whether queued or active.
    pub fn cancel(&mut self, id: u64) -> bool {
        for req in self.queue.iter_mut() {
            if req.id == id {
                req.status = RequestStatus::Cancelled;
                serial_println!("    [batch] Cancelled queued request #{}", id);
                self.queue.retain(|r| r.status != RequestStatus::Cancelled);
                return true;
            }
        }
        for req in self.active_batch.iter_mut() {
            if req.id == id {
                req.status = RequestStatus::Cancelled;
                serial_println!("    [batch] Cancelled active request #{}", id);
                return true;
            }
        }
        false
    }

    /// Get the current batch utilisation as a percentage.
    pub fn utilisation_pct(&self) -> u32 {
        if self.max_batch_size == 0 {
            return 0;
        }
        ((self.active_batch.len() as u64 * 100) / self.max_batch_size as u64) as u32
    }

    /// Get the queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue.len()
    }

    /// Get average tokens generated per step.
    pub fn avg_tokens_per_step(&self) -> f32 {
        if self.total_steps == 0 {
            return 0.0;
        }
        self.total_tokens_generated as f32 / self.total_steps as f32
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct BatchState {
    scheduler: BatchScheduler,
}

static BATCH: Mutex<Option<BatchState>> = Mutex::new(None);

const DEFAULT_BATCH_SIZE: usize = 8;

pub fn init() {
    let scheduler = BatchScheduler::new(DEFAULT_BATCH_SIZE);
    let mut guard = BATCH.lock();
    *guard = Some(BatchState { scheduler });
    serial_println!(
        "    [batch] Continuous batching subsystem initialised (max_batch={})",
        DEFAULT_BATCH_SIZE
    );
}

/// Submit a request to the global batch scheduler.
pub fn submit_global(tokens: Vec<u32>, max_new_tokens: usize) -> u64 {
    let mut guard = BATCH.lock();
    if let Some(state) = guard.as_mut() {
        state.scheduler.submit_tokens(tokens, max_new_tokens)
    } else {
        0
    }
}

/// Run one generation step on the global scheduler.
pub fn step_global() -> Vec<(u64, u32)> {
    let mut guard = BATCH.lock();
    if let Some(state) = guard.as_mut() {
        state.scheduler.step()
    } else {
        Vec::new()
    }
}
