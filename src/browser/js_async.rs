/// JavaScript async runtime for Genesis browser
///
/// Implements the event loop, promise machinery, async/await
/// state machines, microtask and macrotask queues, and timer
/// scheduling (setTimeout / setInterval). All timing uses
/// integer milliseconds (no floats).

use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

static ASYNC_STATE: Mutex<Option<AsyncRuntime>> = Mutex::new(None);

/// Maximum pending microtasks per drain cycle
const MAX_MICROTASKS: usize = 1024;

/// Maximum pending macrotasks
const MAX_MACROTASKS: usize = 256;

/// Maximum active timers
const MAX_TIMERS: usize = 128;

/// Maximum active promises
const MAX_PROMISES: usize = 2048;

/// Maximum async generators
const MAX_ASYNC_GENERATORS: usize = 64;

/// FNV-1a hash
fn async_hash(s: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in s {
        h ^= b as u64;
        h = h.wrapping_mul(0x00000100000001B3);
    }
    h
}

/// Promise state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseState {
    Pending,
    Fulfilled,
    Rejected,
}

/// A JavaScript Promise
#[derive(Debug, Clone)]
pub struct JsPromise {
    pub id: u32,
    pub state: PromiseState,
    pub result_value: i32,          // Q16 numeric result or 0
    pub result_hash: u64,           // hash of string result
    pub then_callbacks: Vec<u32>,   // callback IDs for .then()
    pub catch_callbacks: Vec<u32>,  // callback IDs for .catch()
    pub finally_callbacks: Vec<u32>,// callback IDs for .finally()
    pub chained_promises: Vec<u32>, // promises chained via .then()
    pub handled: bool,              // whether rejection was handled
}

/// Microtask types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrotaskKind {
    PromiseReaction,
    QueueMicrotask,
    MutationObserver,
    AsyncFunctionResume,
}

/// A microtask in the queue
#[derive(Debug, Clone)]
pub struct Microtask {
    pub id: u32,
    pub kind: MicrotaskKind,
    pub callback_id: u32,
    pub promise_id: Option<u32>,
    pub argument_value: i32,    // Q16 value passed to callback
    pub argument_hash: u64,
}

/// Macrotask types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacrotaskKind {
    Timeout,
    Interval,
    MessageChannel,
    IoCallback,
    UserEvent,
}

/// A macrotask in the queue
#[derive(Debug, Clone)]
pub struct Macrotask {
    pub id: u32,
    pub kind: MacrotaskKind,
    pub callback_id: u32,
    pub argument_value: i32,
    pub argument_hash: u64,
    pub ready: bool,
}

/// A timer (setTimeout or setInterval)
#[derive(Debug, Clone)]
pub struct JsTimer {
    pub id: u32,
    pub callback_id: u32,
    pub delay_ms: u32,
    pub interval: bool,         // true = setInterval, false = setTimeout
    pub start_tick: u64,
    pub next_fire_tick: u64,
    pub active: bool,
    pub repeat_count: u32,      // how many times interval has fired
}

/// Async function execution state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncFnState {
    Created,
    Running,
    Suspended,      // awaiting a promise
    Completed,
    Errored,
}

/// An async function instance (state machine)
#[derive(Debug, Clone)]
pub struct AsyncFunction {
    pub id: u32,
    pub function_id: u32,       // bytecode function ID
    pub state: AsyncFnState,
    pub resume_point: u32,      // instruction pointer to resume at
    pub awaited_promise: Option<u32>,
    pub result_promise: u32,    // the promise returned by the async function
    pub local_stack: Vec<i32>,  // saved local variables (Q16)
}

/// Async generator state
#[derive(Debug, Clone)]
pub struct AsyncGenerator {
    pub id: u32,
    pub function_id: u32,
    pub state: AsyncFnState,
    pub resume_point: u32,
    pub yield_queue: Vec<i32>,  // yielded values (Q16)
    pub awaiting_next: bool,
    pub return_promise: Option<u32>,
}

/// Event loop phase
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopPhase {
    Idle,
    ProcessingMicrotasks,
    ProcessingMacrotask,
    ProcessingTimers,
    Rendering,
}

/// Full async runtime state
pub struct AsyncRuntime {
    pub microtask_queue: Vec<Microtask>,
    pub macrotask_queue: Vec<Macrotask>,
    pub promises: Vec<JsPromise>,
    pub timers: Vec<JsTimer>,
    pub async_functions: Vec<AsyncFunction>,
    pub async_generators: Vec<AsyncGenerator>,
    pub current_tick: u64,
    pub phase: LoopPhase,
    pub next_promise_id: u32,
    pub next_microtask_id: u32,
    pub next_macrotask_id: u32,
    pub next_timer_id: u32,
    pub next_async_fn_id: u32,
    pub next_generator_id: u32,
    pub unhandled_rejections: Vec<u32>,
    pub microtasks_processed: u64,
    pub macrotasks_processed: u64,
}

/// Create a new Promise, returns its ID
pub fn promise_new() -> Option<u32> {
    let mut guard = ASYNC_STATE.lock();
    let state = guard.as_mut()?;

    if state.promises.iter().filter(|p| p.state == PromiseState::Pending).count() >= MAX_PROMISES {
        serial_println!("    async: max promises reached");
        return None;
    }

    let id = state.next_promise_id;
    state.next_promise_id = state.next_promise_id.saturating_add(1);
    state.promises.push(JsPromise {
        id,
        state: PromiseState::Pending,
        result_value: 0,
        result_hash: 0,
        then_callbacks: Vec::new(),
        catch_callbacks: Vec::new(),
        finally_callbacks: Vec::new(),
        chained_promises: Vec::new(),
        handled: false,
    });
    Some(id)
}

/// Resolve a promise with a Q16 value
pub fn promise_resolve(promise_id: u32, value: i32) {
    let mut guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(p) = state.promises.iter_mut().find(|p| p.id == promise_id && p.state == PromiseState::Pending) {
            p.state = PromiseState::Fulfilled;
            p.result_value = value;

            // Enqueue microtasks for .then() callbacks
            let callbacks = p.then_callbacks.clone();
            let finally_cbs = p.finally_callbacks.clone();
            for cb_id in callbacks {
                enqueue_microtask_inner(state, MicrotaskKind::PromiseReaction, cb_id, Some(promise_id), value, 0);
            }
            for cb_id in finally_cbs {
                enqueue_microtask_inner(state, MicrotaskKind::PromiseReaction, cb_id, Some(promise_id), 0, 0);
            }

            // Resume any async function awaiting this promise
            resume_async_awaiting(state, promise_id, value);
        }
    }
}

/// Reject a promise
pub fn promise_reject(promise_id: u32, reason_value: i32, reason_hash: u64) {
    let mut guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(p) = state.promises.iter_mut().find(|p| p.id == promise_id && p.state == PromiseState::Pending) {
            p.state = PromiseState::Rejected;
            p.result_value = reason_value;
            p.result_hash = reason_hash;

            let catch_cbs = p.catch_callbacks.clone();
            let finally_cbs = p.finally_callbacks.clone();
            let handled = !catch_cbs.is_empty();
            p.handled = handled;

            for cb_id in catch_cbs {
                enqueue_microtask_inner(state, MicrotaskKind::PromiseReaction, cb_id, Some(promise_id), reason_value, reason_hash);
            }
            for cb_id in finally_cbs {
                enqueue_microtask_inner(state, MicrotaskKind::PromiseReaction, cb_id, Some(promise_id), 0, 0);
            }

            if !handled {
                state.unhandled_rejections.push(promise_id);
            }
        }
    }
}

/// Register a .then() callback on a promise
pub fn promise_then(promise_id: u32, callback_id: u32) -> Option<u32> {
    let mut guard = ASYNC_STATE.lock();
    let state = guard.as_mut()?;

    // Create chained promise
    let chained_id = state.next_promise_id;
    state.next_promise_id = state.next_promise_id.saturating_add(1);
    state.promises.push(JsPromise {
        id: chained_id,
        state: PromiseState::Pending,
        result_value: 0,
        result_hash: 0,
        then_callbacks: Vec::new(),
        catch_callbacks: Vec::new(),
        finally_callbacks: Vec::new(),
        chained_promises: Vec::new(),
        handled: false,
    });

    if let Some(p) = state.promises.iter_mut().find(|p| p.id == promise_id) {
        match p.state {
            PromiseState::Pending => {
                p.then_callbacks.push(callback_id);
                p.chained_promises.push(chained_id);
            }
            PromiseState::Fulfilled => {
                let val = p.result_value;
                enqueue_microtask_inner(state, MicrotaskKind::PromiseReaction, callback_id, Some(promise_id), val, 0);
            }
            PromiseState::Rejected => {} // .then() doesn't handle rejections
        }
    }
    Some(chained_id)
}

/// Register a .catch() callback
pub fn promise_catch(promise_id: u32, callback_id: u32) {
    let mut guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(p) = state.promises.iter_mut().find(|p| p.id == promise_id) {
            p.handled = true;
            match p.state {
                PromiseState::Pending => {
                    p.catch_callbacks.push(callback_id);
                }
                PromiseState::Rejected => {
                    let val = p.result_value;
                    let hash = p.result_hash;
                    enqueue_microtask_inner(state, MicrotaskKind::PromiseReaction, callback_id, Some(promise_id), val, hash);
                    // Remove from unhandled
                    state.unhandled_rejections.retain(|id| *id != promise_id);
                }
                PromiseState::Fulfilled => {}
            }
        }
    }
}

/// Enqueue a microtask
pub fn enqueue_microtask(kind: MicrotaskKind, callback_id: u32) {
    let mut guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        enqueue_microtask_inner(state, kind, callback_id, None, 0, 0);
    }
}

fn enqueue_microtask_inner(
    state: &mut AsyncRuntime,
    kind: MicrotaskKind,
    callback_id: u32,
    promise_id: Option<u32>,
    arg_value: i32,
    arg_hash: u64,
) {
    if state.microtask_queue.len() >= MAX_MICROTASKS {
        serial_println!("    async: microtask queue overflow");
        return;
    }
    let id = state.next_microtask_id;
    state.next_microtask_id = state.next_microtask_id.saturating_add(1);
    state.microtask_queue.push(Microtask {
        id,
        kind,
        callback_id,
        promise_id,
        argument_value: arg_value,
        argument_hash: arg_hash,
    });
}

/// Enqueue a macrotask
pub fn enqueue_macrotask(kind: MacrotaskKind, callback_id: u32, arg_value: i32) {
    let mut guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if state.macrotask_queue.len() >= MAX_MACROTASKS {
            serial_println!("    async: macrotask queue overflow");
            return;
        }
        let id = state.next_macrotask_id;
        state.next_macrotask_id += 1;
        state.macrotask_queue.push(Macrotask {
            id,
            kind,
            callback_id,
            argument_value: arg_value,
            argument_hash: 0,
            ready: true,
        });
    }
}

/// Set a timeout (setTimeout), returns timer ID
pub fn set_timeout(callback_id: u32, delay_ms: u32) -> Option<u32> {
    let mut guard = ASYNC_STATE.lock();
    let state = guard.as_mut()?;

    if state.timers.iter().filter(|t| t.active).count() >= MAX_TIMERS {
        serial_println!("    async: max timers reached");
        return None;
    }

    let id = state.next_timer_id;
    state.next_timer_id = state.next_timer_id.saturating_add(1);
    let fire_tick = state.current_tick + delay_ms as u64;
    state.timers.push(JsTimer {
        id,
        callback_id,
        delay_ms,
        interval: false,
        start_tick: state.current_tick,
        next_fire_tick: fire_tick,
        active: true,
        repeat_count: 0,
    });
    Some(id)
}

/// Set an interval (setInterval), returns timer ID
pub fn set_interval(callback_id: u32, delay_ms: u32) -> Option<u32> {
    let mut guard = ASYNC_STATE.lock();
    let state = guard.as_mut()?;

    if state.timers.iter().filter(|t| t.active).count() >= MAX_TIMERS {
        return None;
    }

    let id = state.next_timer_id;
    state.next_timer_id = state.next_timer_id.saturating_add(1);
    let fire_tick = state.current_tick + delay_ms as u64;
    state.timers.push(JsTimer {
        id,
        callback_id,
        delay_ms,
        interval: true,
        start_tick: state.current_tick,
        next_fire_tick: fire_tick,
        active: true,
        repeat_count: 0,
    });
    Some(id)
}

/// Clear a timer (clearTimeout / clearInterval)
pub fn clear_timer(timer_id: u32) {
    let mut guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(t) = state.timers.iter_mut().find(|t| t.id == timer_id) {
            t.active = false;
        }
    }
}

/// Create an async function instance
pub fn async_fn_create(function_id: u32, result_promise: u32) -> Option<u32> {
    let mut guard = ASYNC_STATE.lock();
    let state = guard.as_mut()?;

    let id = state.next_async_fn_id;
    state.next_async_fn_id = state.next_async_fn_id.saturating_add(1);
    state.async_functions.push(AsyncFunction {
        id,
        function_id,
        state: AsyncFnState::Created,
        resume_point: 0,
        awaited_promise: None,
        result_promise,
        local_stack: Vec::new(),
    });
    Some(id)
}

/// Suspend an async function on an await expression
pub fn async_fn_await(async_fn_id: u32, promise_id: u32, resume_point: u32) {
    let mut guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(af) = state.async_functions.iter_mut().find(|a| a.id == async_fn_id) {
            af.state = AsyncFnState::Suspended;
            af.awaited_promise = Some(promise_id);
            af.resume_point = resume_point;
        }
    }
}

/// Resume async functions awaiting a resolved promise
fn resume_async_awaiting(state: &mut AsyncRuntime, promise_id: u32, value: i32) {
    let to_resume: Vec<u32> = state.async_functions.iter()
        .filter(|a| a.state == AsyncFnState::Suspended && a.awaited_promise == Some(promise_id))
        .map(|a| a.id)
        .collect();

    for af_id in to_resume {
        if let Some(af) = state.async_functions.iter_mut().find(|a| a.id == af_id) {
            af.state = AsyncFnState::Running;
            af.awaited_promise = None;
            // Enqueue microtask to resume execution
            enqueue_microtask_inner(
                state,
                MicrotaskKind::AsyncFunctionResume,
                af.function_id,
                None,
                value,
                af.resume_point as u64,
            );
        }
    }
}

/// Create an async generator
pub fn async_generator_create(function_id: u32) -> Option<u32> {
    let mut guard = ASYNC_STATE.lock();
    let state = guard.as_mut()?;

    if state.async_generators.len() >= MAX_ASYNC_GENERATORS {
        return None;
    }

    let id = state.next_generator_id;
    state.next_generator_id = state.next_generator_id.saturating_add(1);
    state.async_generators.push(AsyncGenerator {
        id,
        function_id,
        state: AsyncFnState::Created,
        resume_point: 0,
        yield_queue: Vec::new(),
        awaiting_next: false,
        return_promise: None,
    });
    Some(id)
}

/// Yield a value from an async generator
pub fn async_generator_yield(generator_id: u32, value: i32) {
    let mut guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        if let Some(gen) = state.async_generators.iter_mut().find(|g| g.id == generator_id) {
            gen.yield_queue.push(value);
            gen.state = AsyncFnState::Suspended;
        }
    }
}

/// Tick the event loop: process timers, drain microtasks, run one macrotask
pub fn event_loop_tick(elapsed_ms: u32) {
    let mut guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_mut() {
        state.current_tick += elapsed_ms as u64;

        // Phase 1: Fire ready timers -> enqueue macrotasks
        state.phase = LoopPhase::ProcessingTimers;
        let current = state.current_tick;
        let mut fired_timers: Vec<(u32, u32)> = Vec::new(); // (callback_id, timer_id)

        for timer in state.timers.iter_mut() {
            if !timer.active {
                continue;
            }
            if current >= timer.next_fire_tick {
                fired_timers.push((timer.callback_id, timer.id));
                if timer.interval {
                    timer.next_fire_tick = current + timer.delay_ms as u64;
                    timer.repeat_count = timer.repeat_count.saturating_add(1);
                } else {
                    timer.active = false;
                }
            }
        }

        for (cb_id, _timer_id) in fired_timers {
            let task_id = state.next_macrotask_id;
            state.next_macrotask_id = state.next_macrotask_id.saturating_add(1);
            if state.macrotask_queue.len() < MAX_MACROTASKS {
                state.macrotask_queue.push(Macrotask {
                    id: task_id,
                    kind: MacrotaskKind::Timeout,
                    callback_id: cb_id,
                    argument_value: 0,
                    argument_hash: 0,
                    ready: true,
                });
            }
        }

        // Phase 2: Drain all microtasks
        state.phase = LoopPhase::ProcessingMicrotasks;
        let mut processed = 0usize;
        while !state.microtask_queue.is_empty() && processed < MAX_MICROTASKS {
            let task = state.microtask_queue.remove(0);
            state.microtasks_processed = state.microtasks_processed.saturating_add(1);
            processed += 1;
            // In a real implementation, this would invoke the callback
            // via the JS interpreter. Here we record it was processed.
            let _ = task;
        }

        // Phase 3: Process one macrotask
        state.phase = LoopPhase::ProcessingMacrotask;
        if let Some(idx) = state.macrotask_queue.iter().position(|t| t.ready) {
            let task = state.macrotask_queue.remove(idx);
            state.macrotasks_processed = state.macrotasks_processed.saturating_add(1);
            let _ = task;

            // After macrotask, drain microtasks again
            state.phase = LoopPhase::ProcessingMicrotasks;
            let mut processed2 = 0usize;
            while !state.microtask_queue.is_empty() && processed2 < MAX_MICROTASKS {
                let task = state.microtask_queue.remove(0);
                state.microtasks_processed = state.microtasks_processed.saturating_add(1);
                processed2 += 1;
                let _ = task;
            }
        }

        state.phase = LoopPhase::Idle;
    }
}

/// Check if the event loop has pending work
pub fn event_loop_has_work() -> bool {
    let guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_ref() {
        !state.microtask_queue.is_empty()
            || !state.macrotask_queue.is_empty()
            || state.timers.iter().any(|t| t.active)
            || state.async_functions.iter().any(|a| a.state == AsyncFnState::Suspended)
    } else {
        false
    }
}

/// Get promise state by ID
pub fn promise_get_state(promise_id: u32) -> Option<PromiseState> {
    let guard = ASYNC_STATE.lock();
    let state = guard.as_ref()?;
    state.promises.iter().find(|p| p.id == promise_id).map(|p| p.state)
}

/// Promise.all: creates a promise that resolves when all input promises resolve
pub fn promise_all(promise_ids: &[u32]) -> Option<u32> {
    let mut guard = ASYNC_STATE.lock();
    let state = guard.as_mut()?;

    let combined_id = state.next_promise_id;
    state.next_promise_id = state.next_promise_id.saturating_add(1);
    state.promises.push(JsPromise {
        id: combined_id,
        state: PromiseState::Pending,
        result_value: 0,
        result_hash: 0,
        then_callbacks: Vec::new(),
        catch_callbacks: Vec::new(),
        finally_callbacks: Vec::new(),
        chained_promises: Vec::new(),
        handled: false,
    });

    // Check if all are already fulfilled
    let mut all_fulfilled = true;
    let mut sum_value: i32 = 0;
    for &pid in promise_ids {
        if let Some(p) = state.promises.iter().find(|p| p.id == pid) {
            match p.state {
                PromiseState::Fulfilled => { sum_value = sum_value.wrapping_add(p.result_value); }
                PromiseState::Rejected => {
                    // Reject the combined promise immediately
                    if let Some(cp) = state.promises.iter_mut().find(|p| p.id == combined_id) {
                        cp.state = PromiseState::Rejected;
                        cp.result_value = p.result_value;
                        cp.result_hash = p.result_hash;
                    }
                    return Some(combined_id);
                }
                PromiseState::Pending => { all_fulfilled = false; }
            }
        }
    }

    if all_fulfilled {
        if let Some(cp) = state.promises.iter_mut().find(|p| p.id == combined_id) {
            cp.state = PromiseState::Fulfilled;
            cp.result_value = sum_value;
        }
    }

    Some(combined_id)
}

/// Get runtime statistics
pub fn async_stats() -> (u64, u64, usize, usize) {
    let guard = ASYNC_STATE.lock();
    if let Some(state) = guard.as_ref() {
        (
            state.microtasks_processed,
            state.macrotasks_processed,
            state.timers.iter().filter(|t| t.active).count(),
            state.promises.iter().filter(|p| p.state == PromiseState::Pending).count(),
        )
    } else {
        (0, 0, 0, 0)
    }
}

/// Initialize the async runtime
pub fn init() {
    let mut guard = ASYNC_STATE.lock();
    *guard = Some(AsyncRuntime {
        microtask_queue: Vec::new(),
        macrotask_queue: Vec::new(),
        promises: Vec::new(),
        timers: Vec::new(),
        async_functions: Vec::new(),
        async_generators: Vec::new(),
        current_tick: 0,
        phase: LoopPhase::Idle,
        next_promise_id: 1,
        next_microtask_id: 1,
        next_macrotask_id: 1,
        next_timer_id: 1,
        next_async_fn_id: 1,
        next_generator_id: 1,
        unhandled_rejections: Vec::new(),
        microtasks_processed: 0,
        macrotasks_processed: 0,
    });
    serial_println!("    browser::js_async initialized");
}
