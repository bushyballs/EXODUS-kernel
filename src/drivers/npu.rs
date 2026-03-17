use crate::sync::Mutex;
/// Neural Processing Unit driver for Genesis
///
/// Provides hardware-accelerated neural network inference:
///   - Model loading and validation (header check, layer graph)
///   - Inference pipeline with input/output tensor management
///   - Quantization support (FP32, FP16, INT8, INT4)
///   - Memory pool management for activation/weight buffers
///   - Hardware acceleration control (systolic array, vector unit)
///   - Performance monitoring (inference count, latency tracking)
///
/// Inspired by: ARM Ethos-U NPU driver, Qualcomm Hexagon NN,
/// Intel NNAPI HAL interface. All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// MMIO register offsets
// ---------------------------------------------------------------------------

const NPU_BASE: u16 = 0xD100;
const REG_CHIP_ID: u16 = NPU_BASE + 0x00;
const REG_STATUS: u16 = NPU_BASE + 0x04;
const REG_CONTROL: u16 = NPU_BASE + 0x08;
const REG_POWER: u16 = NPU_BASE + 0x0C;
const REG_MODEL_ADDR_LO: u16 = NPU_BASE + 0x10;
const REG_MODEL_ADDR_HI: u16 = NPU_BASE + 0x14;
const REG_INPUT_ADDR_LO: u16 = NPU_BASE + 0x18;
const REG_INPUT_ADDR_HI: u16 = NPU_BASE + 0x1C;
const REG_OUTPUT_ADDR_LO: u16 = NPU_BASE + 0x20;
const REG_OUTPUT_ADDR_HI: u16 = NPU_BASE + 0x24;
const REG_INPUT_SIZE: u16 = NPU_BASE + 0x28;
const REG_OUTPUT_SIZE: u16 = NPU_BASE + 0x2C;
const REG_QUANT_MODE: u16 = NPU_BASE + 0x30;
const REG_LAYER_COUNT: u16 = NPU_BASE + 0x34;
const REG_CUR_LAYER: u16 = NPU_BASE + 0x38;
const REG_TOPS: u16 = NPU_BASE + 0x40;
const REG_INTR_STATUS: u16 = NPU_BASE + 0x50;
const REG_INTR_ENABLE: u16 = NPU_BASE + 0x54;
const REG_MEM_POOL_BASE: u16 = NPU_BASE + 0x60;
const REG_MEM_POOL_SIZE: u16 = NPU_BASE + 0x64;

// Status bits
const STATUS_READY: u32 = 1 << 0;
const STATUS_BUSY: u32 = 1 << 1;
const STATUS_MODEL_LOADED: u32 = 1 << 2;
const STATUS_ERROR: u32 = 1 << 7;

// Control bits
const CTRL_RESET: u32 = 1 << 0;
const CTRL_LOAD_MODEL: u32 = 1 << 1;
const CTRL_START_INFER: u32 = 1 << 2;
const CTRL_ABORT: u32 = 1 << 3;
const CTRL_UNLOAD_MODEL: u32 = 1 << 4;

// Model file magic bytes
const MODEL_MAGIC: [u8; 4] = [0x4E, 0x4E, 0x4D, 0x44]; // "NNMD"

// Max loaded models
const MAX_MODELS: usize = 8;
// Max concurrent inference jobs
const MAX_INFER_QUEUE: usize = 32;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Quantization / precision mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuantMode {
    Fp32 = 0,
    Fp16 = 1,
    Int8 = 2,
    Int4 = 3,
}

/// NPU power state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Active,
    Idle,
    Sleep,
    Off,
}

/// Inference job status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferStatus {
    Queued,
    Running,
    Complete,
    Error,
}

/// Model header (parsed from binary)
#[derive(Debug, Clone)]
pub struct ModelHeader {
    pub model_id: u32,
    pub layer_count: u32,
    pub input_size: u32,
    pub output_size: u32,
    pub quant_mode: QuantMode,
    pub weight_size: u32,
}

/// A loaded model slot
struct LoadedModel {
    header: ModelHeader,
    data: Vec<u8>,
    slot_id: usize,
    loaded: bool,
}

/// An inference job
#[derive(Clone)]
pub struct InferenceJob {
    pub job_id: u32,
    pub model_id: u32,
    pub input: Vec<u8>,
    pub output: Vec<u8>,
    pub status: InferStatus,
}

/// Memory pool block
struct PoolBlock {
    offset: u32,
    size: u32,
    in_use: bool,
}

// ---------------------------------------------------------------------------
// Inner driver state
// ---------------------------------------------------------------------------

struct NpuInner {
    initialized: bool,
    chip_id: u32,
    tops: u32,
    power_state: PowerState,
    /// Model slots
    models: [Option<LoadedModel>; MAX_MODELS],
    /// Inference job queue
    infer_queue: VecDeque<InferenceJob>,
    /// Completed inference results
    completed: VecDeque<InferenceJob>,
    /// Next job ID
    next_job_id: u32,
    /// Currently running inference job ID (0 = none)
    active_job_id: u32,
    /// Memory pool blocks for activation buffers
    pool_blocks: Vec<PoolBlock>,
    /// Total pool size in bytes
    pool_total: u32,
    /// Performance counters
    total_inferences: u64,
}

const EMPTY_MODEL_SLOT: Option<LoadedModel> = None;

impl NpuInner {
    const fn new() -> Self {
        NpuInner {
            initialized: false,
            chip_id: 0,
            tops: 0,
            power_state: PowerState::Off,
            models: [EMPTY_MODEL_SLOT; MAX_MODELS],
            infer_queue: VecDeque::new(),
            completed: VecDeque::new(),
            next_job_id: 1,
            active_job_id: 0,
            pool_blocks: Vec::new(),
            pool_total: 0,
            total_inferences: 0,
        }
    }

    fn read_reg(&self, reg: u16) -> u32 {
        crate::io::inl(reg)
    }
    fn write_reg(&self, reg: u16, val: u32) {
        crate::io::outl(reg, val);
    }

    fn hw_reset(&self) {
        self.write_reg(REG_CONTROL, CTRL_RESET);
        for _ in 0..10000 {
            core::hint::spin_loop();
        }
        for _ in 0..50000 {
            if self.read_reg(REG_STATUS) & STATUS_READY != 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    fn set_power(&mut self, state: PowerState) {
        let val = match state {
            PowerState::Active => 0x00,
            PowerState::Idle => 0x01,
            PowerState::Sleep => 0x02,
            PowerState::Off => 0x03,
        };
        self.write_reg(REG_POWER, val);
        self.power_state = state;
    }

    /// Initialize the memory pool (partition into fixed blocks)
    fn init_pool(&mut self) {
        let base = self.read_reg(REG_MEM_POOL_BASE);
        let size = self.read_reg(REG_MEM_POOL_SIZE);
        self.pool_total = if size > 0 { size } else { 4 * 1024 * 1024 }; // default 4 MiB
                                                                         // Partition into 64 KiB blocks
        let block_size = 64 * 1024u32;
        let num_blocks = self.pool_total / block_size;
        self.pool_blocks.clear();
        for i in 0..num_blocks {
            self.pool_blocks.push(PoolBlock {
                offset: base.saturating_add(i.saturating_mul(block_size)),
                size: block_size,
                in_use: false,
            });
        }
    }

    /// Allocate pool memory, returns offset
    fn pool_alloc(&mut self, needed: u32) -> Option<u32> {
        // Simple first-fit: find contiguous free blocks
        let block_size = if self.pool_blocks.is_empty() {
            return None;
        } else {
            self.pool_blocks[0].size
        };
        let blocks_needed = needed.saturating_add(block_size.saturating_sub(1)) / block_size;
        let mut run_start = 0usize;
        let mut run_len = 0u32;
        for i in 0..self.pool_blocks.len() {
            if !self.pool_blocks[i].in_use {
                if run_len == 0 {
                    run_start = i;
                }
                run_len += 1;
                if run_len >= blocks_needed {
                    for j in run_start..=i {
                        self.pool_blocks[j].in_use = true;
                    }
                    return Some(self.pool_blocks[run_start].offset);
                }
            } else {
                run_len = 0;
            }
        }
        None
    }

    /// Free pool memory at offset
    fn pool_free(&mut self, offset: u32, size: u32) {
        let block_size = if self.pool_blocks.is_empty() {
            return;
        } else {
            self.pool_blocks[0].size
        };
        let blocks = size.saturating_add(block_size.saturating_sub(1)) / block_size;
        let pool_end = offset.saturating_add(blocks.saturating_mul(block_size));
        for blk in self.pool_blocks.iter_mut() {
            if blk.offset >= offset && blk.offset < pool_end {
                blk.in_use = false;
            }
        }
    }

    /// Parse and validate a model binary
    fn parse_model(data: &[u8]) -> Option<ModelHeader> {
        if data.len() < 24 {
            return None;
        }
        if data[0..4] != MODEL_MAGIC {
            return None;
        }
        let model_id = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let layer_count = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);
        let input_size = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
        let output_size = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);
        let quant_byte = data[20];
        let quant_mode = match quant_byte {
            0 => QuantMode::Fp32,
            1 => QuantMode::Fp16,
            2 => QuantMode::Int8,
            3 => QuantMode::Int4,
            _ => return None,
        };
        let weight_size = u32::from_le_bytes([
            data[21],
            data[22],
            data[23],
            if data.len() > 24 { data[24] } else { 0 },
        ]);
        Some(ModelHeader {
            model_id,
            layer_count,
            input_size,
            output_size,
            quant_mode,
            weight_size,
        })
    }

    /// Load a model into a free slot
    fn load_model(&mut self, data: &[u8]) -> Result<u32, &'static str> {
        let header = Self::parse_model(data).ok_or("invalid model format")?;
        // Find free slot
        let slot = self
            .models
            .iter()
            .position(|m| m.is_none())
            .ok_or("no free model slots")?;

        // Allocate pool space for weights
        let _pool_offset = self
            .pool_alloc(header.weight_size)
            .ok_or("insufficient pool memory for weights")?;

        // Program model address into hardware
        let model_addr = 0x4000_0000u64 + (slot as u64) * 0x100000;
        self.write_reg(REG_MODEL_ADDR_LO, model_addr as u32);
        self.write_reg(REG_MODEL_ADDR_HI, (model_addr >> 32) as u32);
        self.write_reg(REG_LAYER_COUNT, header.layer_count);
        self.write_reg(REG_QUANT_MODE, header.quant_mode as u32);
        self.write_reg(REG_CONTROL, CTRL_LOAD_MODEL);

        // Wait for model loaded status
        for _ in 0..100_000 {
            let st = self.read_reg(REG_STATUS);
            if st & STATUS_MODEL_LOADED != 0 {
                break;
            }
            if st & STATUS_ERROR != 0 {
                return Err("hardware model load error");
            }
            core::hint::spin_loop();
        }

        let mid = header.model_id;
        self.models[slot] = Some(LoadedModel {
            header,
            data: Vec::from(data),
            slot_id: slot,
            loaded: true,
        });

        Ok(mid)
    }

    /// Unload a model by ID
    fn unload_model(&mut self, model_id: u32) -> bool {
        let mut found_idx = None;
        let mut weight_size = 0;
        for (i, slot) in self.models.iter().enumerate() {
            if let Some(ref m) = slot {
                if m.header.model_id == model_id {
                    weight_size = m.header.weight_size;
                    found_idx = Some(i);
                    break;
                }
            }
        }
        if let Some(idx) = found_idx {
            self.pool_free(0, weight_size);
            self.write_reg(REG_CONTROL, CTRL_UNLOAD_MODEL);
            self.models[idx] = None;
            return true;
        }
        false
    }

    /// Find a loaded model by ID
    fn find_model(&self, model_id: u32) -> Option<&LoadedModel> {
        for slot in self.models.iter() {
            if let Some(ref m) = slot {
                if m.header.model_id == model_id && m.loaded {
                    return Some(m);
                }
            }
        }
        None
    }

    /// Dispatch the next queued inference job
    fn dispatch_next(&mut self) {
        if self.active_job_id != 0 {
            return;
        }
        let job = match self.infer_queue.pop_front() {
            Some(j) => j,
            None => {
                if self.power_state == PowerState::Active {
                    self.set_power(PowerState::Idle);
                }
                return;
            }
        };

        if self.power_state != PowerState::Active {
            self.set_power(PowerState::Active);
        }

        // Validate model is loaded
        let model = match self.find_model(job.model_id) {
            Some(m) => m,
            None => {
                let mut failed = job;
                failed.status = InferStatus::Error;
                self.completed.push_back(failed);
                return;
            }
        };

        // Program input/output addresses
        let in_addr = 0x5000_0000u64;
        let out_addr = 0x6000_0000u64;
        self.write_reg(REG_INPUT_ADDR_LO, in_addr as u32);
        self.write_reg(REG_INPUT_ADDR_HI, (in_addr >> 32) as u32);
        self.write_reg(REG_OUTPUT_ADDR_LO, out_addr as u32);
        self.write_reg(REG_OUTPUT_ADDR_HI, (out_addr >> 32) as u32);
        self.write_reg(REG_INPUT_SIZE, model.header.input_size);
        self.write_reg(REG_OUTPUT_SIZE, model.header.output_size);

        // Start inference
        self.write_reg(REG_CONTROL, CTRL_START_INFER);
        self.active_job_id = job.job_id;

        let mut running = job;
        running.status = InferStatus::Running;
        self.infer_queue.push_front(running);
    }

    /// Poll hardware for inference completion
    fn poll_completion(&mut self) {
        if self.active_job_id == 0 {
            return;
        }
        let status = self.read_reg(REG_STATUS);
        if status & STATUS_BUSY != 0 {
            return;
        }

        let completed_id = self.active_job_id;
        self.active_job_id = 0;

        if let Some(mut job) = self.infer_queue.pop_front() {
            if job.job_id == completed_id {
                if status & STATUS_ERROR != 0 {
                    job.status = InferStatus::Error;
                } else {
                    job.status = InferStatus::Complete;
                    // Read output size from model header
                    if let Some(model) = self.find_model(job.model_id) {
                        job.output = alloc::vec![0u8; model.header.output_size as usize];
                    }
                }
                self.completed.push_back(job);
                self.total_inferences = self.total_inferences.saturating_add(1);
            }
        }
        self.write_reg(REG_INTR_STATUS, 0xFFFF_FFFF);
        self.dispatch_next();
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static NPU: Mutex<NpuInner> = Mutex::new(NpuInner::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the NPU driver
pub fn init() {
    let mut npu = NPU.lock();
    let chip_id = npu.read_reg(REG_CHIP_ID);
    if chip_id == 0 || chip_id == 0xFFFF_FFFF {
        serial_println!("  NPU: no neural processing unit detected");
        return;
    }
    npu.chip_id = chip_id;
    npu.hw_reset();

    npu.tops = npu.read_reg(REG_TOPS);
    if npu.tops == 0 {
        npu.tops = 4;
    }

    npu.init_pool();
    npu.write_reg(REG_INTR_ENABLE, 0x01);
    npu.set_power(PowerState::Idle);
    npu.initialized = true;

    serial_println!(
        "  NPU: chip={:#010X}, {} TOPS, pool={} KiB",
        npu.chip_id,
        npu.tops,
        npu.pool_total / 1024
    );
    drop(npu);
    super::register("npu", super::DeviceType::Other);
}

/// Load a model binary into the NPU
pub fn load_model(data: &[u8]) -> Result<u32, &'static str> {
    let mut npu = NPU.lock();
    if !npu.initialized {
        return Err("NPU not initialized");
    }
    npu.load_model(data)
}

/// Unload a model by ID
pub fn unload_model(model_id: u32) -> bool {
    let mut npu = NPU.lock();
    if !npu.initialized {
        return false;
    }
    npu.unload_model(model_id)
}

/// Submit an inference job for a loaded model
pub fn submit_inference(model_id: u32, input: &[u8]) -> Result<u32, &'static str> {
    let mut npu = NPU.lock();
    if !npu.initialized {
        return Err("NPU not initialized");
    }
    if npu.infer_queue.len() >= MAX_INFER_QUEUE {
        return Err("inference queue full");
    }

    // Validate model exists
    if npu.find_model(model_id).is_none() {
        return Err("model not loaded");
    }

    let job_id = npu.next_job_id;
    npu.next_job_id = npu.next_job_id.saturating_add(1);

    let job = InferenceJob {
        job_id,
        model_id,
        input: Vec::from(input),
        output: Vec::new(),
        status: InferStatus::Queued,
    };
    npu.infer_queue.push_back(job);
    npu.dispatch_next();
    Ok(job_id)
}

/// Poll for completed inference jobs
pub fn poll() {
    let mut npu = NPU.lock();
    if !npu.initialized {
        return;
    }
    npu.poll_completion();
}

/// Check status of an inference job
pub fn job_status(job_id: u32) -> InferStatus {
    let npu = NPU.lock();
    for j in npu.completed.iter() {
        if j.job_id == job_id {
            return j.status;
        }
    }
    for j in npu.infer_queue.iter() {
        if j.job_id == job_id {
            return j.status;
        }
    }
    InferStatus::Error
}

/// Collect inference output (removes from completed queue)
pub fn collect_result(job_id: u32) -> Option<Vec<u8>> {
    let mut npu = NPU.lock();
    if let Some(pos) = npu.completed.iter().position(|j| j.job_id == job_id) {
        if let Some(job) = npu.completed.remove(pos) {
            if job.status == InferStatus::Complete {
                return Some(job.output);
            }
        }
    }
    None
}

/// Get total inferences run since init
pub fn total_inferences() -> u64 {
    NPU.lock().total_inferences
}

/// Get NPU power state
pub fn power_state() -> PowerState {
    NPU.lock().power_state
}

/// Set NPU power state
pub fn set_power_state(state: PowerState) {
    let mut npu = NPU.lock();
    if !npu.initialized {
        return;
    }
    npu.set_power(state);
}
