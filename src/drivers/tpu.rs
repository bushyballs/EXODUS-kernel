use crate::sync::Mutex;
/// Tensor Processing Unit driver for Genesis
///
/// Provides hardware-accelerated matrix/tensor operations via a PCI-mapped TPU:
///   - Compute queue with job scheduling and priority levels
///   - Matrix multiply dispatch (FP16/FP32/INT8 precision)
///   - DMA buffer management for input/output tensors
///   - Inference job lifecycle (submit, poll, cancel)
///   - Power state management (active, idle, sleep, off)
///   - Register-level control of systolic array and accumulators
///
/// Inspired by: Google Edge TPU programming model, NVIDIA NVDLA open-source
/// accelerator spec, ARM Ethos-N NPU register interface. All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// MMIO register offsets (from BAR0)
// ---------------------------------------------------------------------------

const TPU_BASE: u16 = 0xD000;
const REG_CHIP_ID: u16 = TPU_BASE + 0x00;
const REG_STATUS: u16 = TPU_BASE + 0x04;
const REG_CONTROL: u16 = TPU_BASE + 0x08;
const REG_POWER: u16 = TPU_BASE + 0x0C;
const REG_QUEUE_HEAD: u16 = TPU_BASE + 0x10;
const REG_QUEUE_TAIL: u16 = TPU_BASE + 0x14;
const REG_DMA_SRC_LO: u16 = TPU_BASE + 0x20;
const REG_DMA_SRC_HI: u16 = TPU_BASE + 0x24;
const REG_DMA_DST_LO: u16 = TPU_BASE + 0x28;
const REG_DMA_DST_HI: u16 = TPU_BASE + 0x2C;
const REG_DMA_LEN: u16 = TPU_BASE + 0x30;
const REG_DMA_CTRL: u16 = TPU_BASE + 0x34;
const REG_MAT_ROWS_A: u16 = TPU_BASE + 0x40;
const REG_MAT_COLS_A: u16 = TPU_BASE + 0x44;
const REG_MAT_COLS_B: u16 = TPU_BASE + 0x48;
const REG_PRECISION: u16 = TPU_BASE + 0x4C;
const REG_JOB_ID: u16 = TPU_BASE + 0x50;
const REG_JOB_STATUS: u16 = TPU_BASE + 0x54;
const REG_INTR_STATUS: u16 = TPU_BASE + 0x60;
const REG_INTR_ENABLE: u16 = TPU_BASE + 0x64;
const REG_COMPUTE_UNITS: u16 = TPU_BASE + 0x70;
const REG_CLOCK_MHZ: u16 = TPU_BASE + 0x74;

// Status register bits
const STATUS_READY: u32 = 1 << 0;
const STATUS_BUSY: u32 = 1 << 1;
const STATUS_DMA_ACTIVE: u32 = 1 << 2;
const STATUS_ERROR: u32 = 1 << 7;

// Control register bits
const CTRL_RESET: u32 = 1 << 0;
const CTRL_START_COMPUTE: u32 = 1 << 1;
const CTRL_ABORT: u32 = 1 << 2;
const CTRL_DMA_START: u32 = 1 << 4;

// Power state values
const POWER_ACTIVE: u32 = 0x00;
const POWER_IDLE: u32 = 0x01;
const POWER_SLEEP: u32 = 0x02;
const POWER_OFF: u32 = 0x03;

// Max hardware queue depth
const MAX_QUEUE_DEPTH: usize = 64;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Precision for matrix/tensor operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    /// 32-bit floating point
    Fp32 = 0,
    /// 16-bit floating point (half)
    Fp16 = 1,
    /// 8-bit integer (quantized)
    Int8 = 2,
    /// 4-bit integer (quantized, packed)
    Int4 = 3,
}

/// TPU power state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Active,
    Idle,
    Sleep,
    Off,
}

/// Job status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Complete,
    Error,
    Cancelled,
}

/// A compute job descriptor
#[derive(Debug, Clone)]
pub struct ComputeJob {
    /// Unique job identifier
    pub job_id: u32,
    /// Input data buffer
    pub input: Vec<u8>,
    /// Output data buffer (filled on completion)
    pub output: Vec<u8>,
    /// Operation descriptor (matrix dims, precision, etc.)
    pub rows_a: u32,
    pub cols_a: u32,
    pub cols_b: u32,
    pub precision: Precision,
    /// Priority (0 = highest, 3 = lowest)
    pub priority: u8,
    /// Current status
    pub status: JobStatus,
}

/// DMA buffer descriptor
struct DmaBuffer {
    /// Physical address (simulated as offset)
    phys_addr: u64,
    /// Length in bytes
    length: u32,
    /// Whether this slot is in use
    in_use: bool,
}

// ---------------------------------------------------------------------------
// Inner driver state
// ---------------------------------------------------------------------------

struct TpuInner {
    /// Whether the TPU has been detected and initialized
    initialized: bool,
    /// Detected chip ID
    chip_id: u32,
    /// Number of systolic array compute units
    compute_units: u32,
    /// Clock frequency in MHz
    clock_mhz: u32,
    /// Current power state
    power_state: PowerState,
    /// Job queue (priority-sorted)
    job_queue: VecDeque<ComputeJob>,
    /// Completed jobs awaiting collection
    completed: VecDeque<ComputeJob>,
    /// Next job ID counter
    next_job_id: u32,
    /// DMA buffer pool (fixed slots)
    dma_buffers: [DmaBuffer; 8],
    /// Currently executing job ID (0 = none)
    active_job_id: u32,
    /// Total jobs processed
    jobs_processed: u64,
}

impl TpuInner {
    const fn new() -> Self {
        const EMPTY_BUF: DmaBuffer = DmaBuffer {
            phys_addr: 0,
            length: 0,
            in_use: false,
        };
        TpuInner {
            initialized: false,
            chip_id: 0,
            compute_units: 0,
            clock_mhz: 0,
            power_state: PowerState::Off,
            job_queue: VecDeque::new(),
            completed: VecDeque::new(),
            next_job_id: 1,
            dma_buffers: [EMPTY_BUF; 8],
            active_job_id: 0,
            jobs_processed: 0,
        }
    }

    /// Read a 32-bit TPU register
    fn read_reg(&self, reg: u16) -> u32 {
        crate::io::inl(reg)
    }

    /// Write a 32-bit TPU register
    fn write_reg(&self, reg: u16, val: u32) {
        crate::io::outl(reg, val);
    }

    /// Perform a hardware reset of the TPU
    fn hw_reset(&self) {
        self.write_reg(REG_CONTROL, CTRL_RESET);
        for _ in 0..10000 {
            core::hint::spin_loop();
        }
        // Wait for ready bit
        for _ in 0..50000 {
            if self.read_reg(REG_STATUS) & STATUS_READY != 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Set the hardware power state
    fn set_power_state(&mut self, state: PowerState) {
        let val = match state {
            PowerState::Active => POWER_ACTIVE,
            PowerState::Idle => POWER_IDLE,
            PowerState::Sleep => POWER_SLEEP,
            PowerState::Off => POWER_OFF,
        };
        self.write_reg(REG_POWER, val);
        self.power_state = state;
    }

    /// Allocate a DMA buffer slot, returns index
    fn alloc_dma_buffer(&mut self, phys_addr: u64, length: u32) -> Option<usize> {
        for (i, buf) in self.dma_buffers.iter_mut().enumerate() {
            if !buf.in_use {
                buf.phys_addr = phys_addr;
                buf.length = length;
                buf.in_use = true;
                return Some(i);
            }
        }
        None
    }

    /// Free a DMA buffer slot
    fn free_dma_buffer(&mut self, index: usize) {
        if index < self.dma_buffers.len() {
            self.dma_buffers[index].in_use = false;
            self.dma_buffers[index].phys_addr = 0;
            self.dma_buffers[index].length = 0;
        }
    }

    /// Program the DMA engine for a transfer
    fn start_dma(&self, src: u64, dst: u64, len: u32) {
        self.write_reg(REG_DMA_SRC_LO, src as u32);
        self.write_reg(REG_DMA_SRC_HI, (src >> 32) as u32);
        self.write_reg(REG_DMA_DST_LO, dst as u32);
        self.write_reg(REG_DMA_DST_HI, (dst >> 32) as u32);
        self.write_reg(REG_DMA_LEN, len);
        self.write_reg(REG_DMA_CTRL, CTRL_DMA_START);
    }

    /// Wait for DMA to complete
    fn wait_dma(&self) -> bool {
        for _ in 0..100_000 {
            let status = self.read_reg(REG_STATUS);
            if status & STATUS_DMA_ACTIVE == 0 {
                return true;
            }
            if status & STATUS_ERROR != 0 {
                return false;
            }
            core::hint::spin_loop();
        }
        false
    }

    /// Dispatch the next queued job to hardware
    fn dispatch_next(&mut self) {
        if self.active_job_id != 0 {
            return;
        }
        let job = match self.job_queue.pop_front() {
            Some(j) => j,
            None => {
                // No work — transition to idle
                if self.power_state == PowerState::Active {
                    self.set_power_state(PowerState::Idle);
                }
                return;
            }
        };

        // Ensure active power
        if self.power_state != PowerState::Active {
            self.set_power_state(PowerState::Active);
        }

        // Program matrix dimensions
        self.write_reg(REG_MAT_ROWS_A, job.rows_a);
        self.write_reg(REG_MAT_COLS_A, job.cols_a);
        self.write_reg(REG_MAT_COLS_B, job.cols_b);
        self.write_reg(REG_PRECISION, job.precision as u32);
        self.write_reg(REG_JOB_ID, job.job_id);

        // DMA input data to device memory (simulated addresses)
        let input_addr = 0x1000_0000u64.saturating_add((job.job_id as u64).saturating_mul(0x10000));
        self.start_dma(input_addr, 0x2000_0000, job.input.len() as u32);
        self.wait_dma();

        // Start compute
        self.write_reg(REG_CONTROL, CTRL_START_COMPUTE);
        self.active_job_id = job.job_id;

        // Store the job back so we can retrieve it on completion
        let mut running_job = job;
        running_job.status = JobStatus::Running;
        self.job_queue.push_front(running_job);
    }

    /// Poll hardware for job completion
    fn poll_completion(&mut self) {
        if self.active_job_id == 0 {
            return;
        }

        let status = self.read_reg(REG_STATUS);
        if status & STATUS_BUSY != 0 {
            return;
        }

        let hw_job_status = self.read_reg(REG_JOB_STATUS);
        let completed_id = self.active_job_id;
        self.active_job_id = 0;

        // Find the job in the queue front
        if let Some(mut job) = self.job_queue.pop_front() {
            if job.job_id == completed_id {
                if status & STATUS_ERROR != 0 || hw_job_status != 0 {
                    job.status = JobStatus::Error;
                } else {
                    job.status = JobStatus::Complete;
                    // DMA output data back (fill output buffer with result size)
                    let out_size =
                        (job.rows_a.saturating_mul(job.cols_b).saturating_mul(4)) as usize;
                    job.output = alloc::vec![0u8; out_size];
                    // Read result from device output region
                    let out_addr = 0x3000_0000u64;
                    self.start_dma(out_addr, out_addr + 0x1000_0000, out_size as u32);
                    self.wait_dma();
                }
                self.completed.push_back(job);
                self.jobs_processed = self.jobs_processed.saturating_add(1);
            }
        }

        // Clear interrupt status
        self.write_reg(REG_INTR_STATUS, 0xFFFF_FFFF);

        // Dispatch next job
        self.dispatch_next();
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static TPU: Mutex<TpuInner> = Mutex::new(TpuInner::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the TPU driver (detect via PCI BAR probing)
pub fn init() {
    let mut tpu = TPU.lock();

    // Probe chip ID register
    let chip_id = tpu.read_reg(REG_CHIP_ID);
    if chip_id == 0 || chip_id == 0xFFFF_FFFF {
        serial_println!("  TPU: no tensor processing unit detected");
        return;
    }

    tpu.chip_id = chip_id;
    tpu.hw_reset();

    // Read hardware capabilities
    tpu.compute_units = tpu.read_reg(REG_COMPUTE_UNITS);
    if tpu.compute_units == 0 {
        tpu.compute_units = 1;
    }
    tpu.clock_mhz = tpu.read_reg(REG_CLOCK_MHZ);
    if tpu.clock_mhz == 0 {
        tpu.clock_mhz = 500;
    }

    // Enable interrupts for job completion
    tpu.write_reg(REG_INTR_ENABLE, 0x01);

    // Bring to idle (low-power until work arrives)
    tpu.set_power_state(PowerState::Idle);
    tpu.initialized = true;

    serial_println!(
        "  TPU: chip={:#010X}, {} CUs @ {} MHz",
        tpu.chip_id,
        tpu.compute_units,
        tpu.clock_mhz
    );
    drop(tpu);
    super::register("tpu", super::DeviceType::Other);
}

/// Submit a matrix multiply job (A[rows_a x cols_a] * B[cols_a x cols_b])
pub fn submit_matmul(
    input: &[u8],
    rows_a: u32,
    cols_a: u32,
    cols_b: u32,
    precision: Precision,
    priority: u8,
) -> Result<u32, &'static str> {
    let mut tpu = TPU.lock();
    if !tpu.initialized {
        return Err("TPU not initialized");
    }
    if tpu.job_queue.len() >= MAX_QUEUE_DEPTH {
        return Err("job queue full");
    }

    let job_id = tpu.next_job_id;
    tpu.next_job_id = tpu.next_job_id.saturating_add(1);

    let job = ComputeJob {
        job_id,
        input: Vec::from(input),
        output: Vec::new(),
        rows_a,
        cols_a,
        cols_b,
        precision,
        priority: priority.min(3),
        status: JobStatus::Queued,
    };

    // Insert by priority (lower number = higher priority)
    let pos = tpu.job_queue.iter().position(|j| j.priority > job.priority);
    match pos {
        Some(idx) => tpu.job_queue.insert(idx, job),
        None => tpu.job_queue.push_back(job),
    }

    // Kick the dispatch loop
    tpu.dispatch_next();

    Ok(job_id)
}

/// Submit a generic tensor operation
pub fn submit_tensor_op(op: &[u8], data: &[u8]) -> Result<u32, &'static str> {
    // Parse op descriptor: first 4 bytes = rows_a, next 4 = cols_a, next 4 = cols_b
    if op.len() < 12 {
        return Err("op descriptor too short");
    }
    let rows_a = u32::from_le_bytes([op[0], op[1], op[2], op[3]]);
    let cols_a = u32::from_le_bytes([op[4], op[5], op[6], op[7]]);
    let cols_b = u32::from_le_bytes([op[8], op[9], op[10], op[11]]);
    submit_matmul(data, rows_a, cols_a, cols_b, Precision::Fp32, 1)
}

/// Poll for completed jobs (call from timer interrupt or worker thread)
pub fn poll() {
    let mut tpu = TPU.lock();
    if !tpu.initialized {
        return;
    }
    tpu.poll_completion();
}

/// Check the status of a submitted job
pub fn job_status(job_id: u32) -> JobStatus {
    let tpu = TPU.lock();
    // Check completed queue
    for job in tpu.completed.iter() {
        if job.job_id == job_id {
            return job.status;
        }
    }
    // Check active queue
    for job in tpu.job_queue.iter() {
        if job.job_id == job_id {
            return job.status;
        }
    }
    JobStatus::Error
}

/// Collect the output of a completed job (removes from completed queue)
pub fn collect_result(job_id: u32) -> Option<Vec<u8>> {
    let mut tpu = TPU.lock();
    if let Some(pos) = tpu.completed.iter().position(|j| j.job_id == job_id) {
        if let Some(job) = tpu.completed.remove(pos) {
            if job.status == JobStatus::Complete {
                return Some(job.output);
            }
        }
    }
    None
}

/// Cancel a queued (not yet running) job
pub fn cancel_job(job_id: u32) -> bool {
    let mut tpu = TPU.lock();
    if let Some(pos) = tpu
        .job_queue
        .iter()
        .position(|j| j.job_id == job_id && j.status == JobStatus::Queued)
    {
        tpu.job_queue.remove(pos);
        return true;
    }
    false
}

/// Get current power state
pub fn power_state() -> PowerState {
    TPU.lock().power_state
}

/// Set power state (Active, Idle, Sleep, Off)
pub fn set_power_state(state: PowerState) {
    let mut tpu = TPU.lock();
    if !tpu.initialized {
        return;
    }
    tpu.set_power_state(state);
}

/// Get number of pending jobs in the queue
pub fn queue_depth() -> usize {
    TPU.lock().job_queue.len()
}

/// Get total jobs processed since init
pub fn jobs_processed() -> u64 {
    TPU.lock().jobs_processed
}
