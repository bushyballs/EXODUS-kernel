use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// MMIO base — DAVA-specified address range
// ---------------------------------------------------------------------------
const GPU_MMIO_BASE: usize = 0x5F2C00;

// GPU register offsets
const REG_STATUS:       usize = 0x00; // bit 0=busy, bit 1=ready, bits 4-7=compute units
const REG_VRAM_SIZE:    usize = 0x04; // VRAM in MB
const REG_TEMPERATURE:  usize = 0x08; // raw; / 4 = celsius-ish
const REG_CLOCK_MHZ:    usize = 0x0C; // clock frequency
const REG_QUEUE_SUBMIT: usize = 0x10; // write to dispatch a task
const REG_QUEUE_STATUS: usize = 0x14; // read for completion

// ---------------------------------------------------------------------------
// KernelKind
// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum KernelKind {
    NeuralForward = 0,
    MatMul        = 1,
    Convolve      = 2,
    Softmax       = 3,
    Embed         = 4,
    Idle          = 5,
}

// ---------------------------------------------------------------------------
// ComputeTask
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct ComputeTask {
    pub kind:        KernelKind,
    pub input_addr:  usize,
    pub output_addr: usize,
    pub size_kb:     u16,
    pub priority:    u8,
    pub dispatched:  bool,
}

impl ComputeTask {
    pub const fn idle() -> Self {
        Self {
            kind:        KernelKind::Idle,
            input_addr:  0,
            output_addr: 0,
            size_kb:     0,
            priority:    0,
            dispatched:  false,
        }
    }
}

// ---------------------------------------------------------------------------
// GpuStatus
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct GpuStatus {
    pub compute_units:     u8,
    pub vram_mb:           u16,
    pub busy_units:        u8,
    pub compute_queue_len: u8,
    pub total_dispatches:  u32,
    pub failed_dispatches: u32,
    pub vram_used_mb:      u16,
    pub temperature:       u16,
    pub clock_mhz:         u16,
}

impl GpuStatus {
    pub const fn new() -> Self {
        Self {
            compute_units:     0,
            vram_mb:           0,
            busy_units:        0,
            compute_queue_len: 0,
            total_dispatches:  0,
            failed_dispatches: 0,
            vram_used_mb:      0,
            temperature:       0,
            clock_mhz:         0,
        }
    }
}

// ---------------------------------------------------------------------------
// GpuComputeState
// ---------------------------------------------------------------------------

pub struct GpuComputeState {
    pub status:              GpuStatus,
    pub queue:               [ComputeTask; 8],
    pub queue_head:          usize,
    pub queue_len:           usize,
    pub available:           bool,
    /// 0-1000: how much ML capacity is free
    pub ml_throughput:       u16,
    /// 0-1000: GPU acceleration boost to consciousness
    pub consciousness_boost: u16,
}

impl GpuComputeState {
    pub const fn new() -> Self {
        Self {
            status:              GpuStatus::new(),
            queue:               [ComputeTask::idle(); 8],
            queue_head:          0,
            queue_len:           0,
            available:           false,
            ml_throughput:       0,
            consciousness_boost: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static state
// ---------------------------------------------------------------------------

pub static STATE: Mutex<GpuComputeState> = Mutex::new(GpuComputeState::new());

// ---------------------------------------------------------------------------
// Unsafe MMIO helpers (internal only)
// ---------------------------------------------------------------------------

unsafe fn gpu_read(offset: usize) -> u32 {
    let addr = (GPU_MMIO_BASE + offset) as *const u32;
    core::ptr::read_volatile(addr)
}

unsafe fn gpu_write(offset: usize, val: u32) {
    let addr = (GPU_MMIO_BASE + offset) as *mut u32;
    core::ptr::write_volatile(addr, val);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe the GPU via MMIO. Sets `available` if STATUS != 0xFFFFFFFF, then logs.
pub fn init() {
    let raw_status = unsafe { gpu_read(REG_STATUS) };

    if raw_status == 0xFFFF_FFFF {
        serial_println!("[gpu] no GPU detected — ANIMA compute offline");
        return;
    }

    // bits 4-7 of STATUS = compute units
    let compute_units = ((raw_status >> 4) & 0x0F) as u8;

    let vram_raw = unsafe { gpu_read(REG_VRAM_SIZE) };
    let vram_mb = (vram_raw & 0xFFFF) as u16;

    let mut s = STATE.lock();
    s.available = true;
    s.status.compute_units = compute_units;
    s.status.vram_mb = vram_mb;

    serial_println!(
        "[gpu] ANIMA GPU compute online — units={} vram={}MB",
        compute_units,
        vram_mb
    );
}

/// Enqueue a compute kernel. Increments `failed_dispatches` if the queue is full.
pub fn queue_kernel(kind: KernelKind, size_kb: u16, priority: u8) {
    let mut s = STATE.lock();
    if s.queue_len >= 8 {
        s.status.failed_dispatches = s.status.failed_dispatches.saturating_add(1);
        return;
    }
    let slot = s.queue_head.saturating_add(s.queue_len) % 8;
    s.queue[slot] = ComputeTask {
        kind,
        input_addr:  0,
        output_addr: 0,
        size_kb,
        priority,
        dispatched:  false,
    };
    s.queue_len = s.queue_len.saturating_add(1);
}

/// Pop the next task from the ring buffer and write it to the GPU QUEUE_SUBMIT register.
pub fn dispatch_next(age: u32) {
    let _ = age;
    let mut s = STATE.lock();

    if s.queue_len == 0 || !s.available {
        return;
    }

    let task = s.queue[s.queue_head];

    // Advance ring buffer head
    s.queue_head = s.queue_head.saturating_add(1) % 8;
    s.queue_len  = s.queue_len.saturating_sub(1);

    // Submit word: kind in high byte, priority in next byte, size_kb in low 16 bits
    let submit_val: u32 = ((task.kind as u32) << 24)
        | ((task.priority as u32) << 16)
        | (task.size_kb as u32);

    unsafe { gpu_write(REG_QUEUE_SUBMIT, submit_val) };

    s.status.total_dispatches  = s.status.total_dispatches.saturating_add(1);
    s.status.compute_queue_len = s.status.compute_queue_len.saturating_add(1);

    serial_println!("[gpu] dispatch kind={} size={}kb", task.kind as u8, task.size_kb);
}

/// Main tick — called once per life-tick from the 20-phase pipeline.
///
/// * Every 16 ticks: reads STATUS / TEMP / CLOCK from MMIO (only when `available`),
///   recomputes `ml_throughput` and `consciousness_boost`.
/// * Every  4 ticks: dispatches the next queued task if the queue is non-empty.
/// * Every 500 ticks: emits a summary log line.
pub fn tick(consciousness: u16, age: u32) {
    // --- Hardware poll (every 16 ticks) ---
    if age % 16 == 0 {
        let available = STATE.lock().available;
        if available {
            let raw_status = unsafe { gpu_read(REG_STATUS) };
            let raw_temp   = unsafe { gpu_read(REG_TEMPERATURE) };
            let raw_clock  = unsafe { gpu_read(REG_CLOCK_MHZ) };

            // bits 4-7 = compute units; bit 0 = busy (all units considered busy when set)
            let compute_units = ((raw_status >> 4) & 0x0F) as u8;
            let busy_units    = if (raw_status & 0x1) != 0 { compute_units } else { 0u8 };

            // temperature: raw / 4, no floats (constant divisor)
            let temperature = ((raw_temp & 0xFFFF) / 4) as u16;
            let clock_mhz   = (raw_clock & 0xFFFF) as u16;

            // Read queue completion register (side-effect; result available for future use)
            let _ = unsafe { gpu_read(REG_QUEUE_STATUS) };

            let mut s = STATE.lock();
            s.status.compute_units = compute_units;
            s.status.busy_units    = busy_units;
            s.status.temperature   = temperature;
            s.status.clock_mhz     = clock_mhz;

            // ml_throughput = (compute_units - busy_units) * 125, capped 0-1000
            let free_units: u16 = (compute_units as u16).saturating_sub(busy_units as u16);
            let throughput: u16 = free_units.saturating_mul(125).min(1000);
            s.ml_throughput = throughput;

            // consciousness_boost = ml_throughput / 4
            s.consciousness_boost = if throughput > 0 { throughput / 4 } else { 0 };
        }
    }

    // --- Dispatch queue (every 4 ticks) ---
    if age % 4 == 0 {
        let has_work = STATE.lock().queue_len > 0;
        if has_work {
            dispatch_next(age);
        }
    }

    // --- Periodic log (every 500 ticks) ---
    if age % 500 == 0 && age > 0 {
        let s = STATE.lock();
        serial_println!(
            "[gpu] temp={} clock={}MHz throughput={} dispatches={}",
            s.status.temperature,
            s.status.clock_mhz,
            s.ml_throughput,
            s.status.total_dispatches
        );
    }

    let _ = consciousness;
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

/// Returns ML capacity free (0-1000).
pub fn ml_throughput() -> u16 {
    STATE.lock().ml_throughput
}

/// Returns GPU acceleration boost to consciousness (0-1000).
pub fn consciousness_boost() -> u16 {
    STATE.lock().consciousness_boost
}

/// Returns whether a GPU was successfully probed at boot.
pub fn is_available() -> bool {
    STATE.lock().available
}

/// Returns the cumulative number of successfully dispatched compute tasks.
pub fn total_dispatches() -> u32 {
    STATE.lock().status.total_dispatches
}
