use crate::sync::Mutex;
/// DMA engine driver for Genesis
///
/// Provides hardware DMA (Direct Memory Access) for high-throughput data movement:
///   - Up to 8 independent DMA channels with priority levels
///   - Scatter-gather descriptor chains
///   - Memory-to-memory and memory-to-peripheral transfer modes
///   - Per-channel completion callbacks (function pointers)
///   - Channel allocation and release
///   - Transfer abort and error recovery
///   - Burst size and transfer width configuration
///
/// Inspired by: Linux DMA engine framework (drivers/dma/), ARM PL330 DMA
/// controller, Intel IOAT DMA. All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// MMIO register map
// ---------------------------------------------------------------------------

const DMA_BASE: u16 = 0xD200;
const REG_CHIP_ID: u16 = DMA_BASE + 0x00;
const REG_GLOBAL_STATUS: u16 = DMA_BASE + 0x04;
const REG_GLOBAL_CTRL: u16 = DMA_BASE + 0x08;
const REG_INTR_STATUS: u16 = DMA_BASE + 0x0C;
const REG_INTR_ENABLE: u16 = DMA_BASE + 0x10;
const REG_CHANNEL_COUNT: u16 = DMA_BASE + 0x14;

// Per-channel register block (base + 0x100 + ch * 0x40)
const CH_REG_BASE: u16 = DMA_BASE + 0x100;
const CH_STRIDE: u16 = 0x40;

const CH_CTRL: u16 = 0x00;
const CH_STATUS: u16 = 0x04;
const CH_SRC_LO: u16 = 0x08;
const CH_SRC_HI: u16 = 0x0C;
const CH_DST_LO: u16 = 0x10;
const CH_DST_HI: u16 = 0x14;
const CH_XFER_LEN: u16 = 0x18;
const CH_SG_ADDR_LO: u16 = 0x1C;
const CH_SG_ADDR_HI: u16 = 0x20;
const CH_SG_COUNT: u16 = 0x24;
const CH_BURST_CFG: u16 = 0x28;
const CH_PRIORITY: u16 = 0x2C;
const CH_BYTES_DONE: u16 = 0x30;

// Channel control bits
const CHCTRL_ENABLE: u32 = 1 << 0;
const CHCTRL_START: u32 = 1 << 1;
const CHCTRL_ABORT: u32 = 1 << 2;
const CHCTRL_SG_MODE: u32 = 1 << 3;
const CHCTRL_MEM2MEM: u32 = 1 << 4;
const CHCTRL_MEM2PERIPH: u32 = 1 << 5;
const CHCTRL_PERIPH2MEM: u32 = 1 << 6;
const CHCTRL_INTR_COMPLETE: u32 = 1 << 8;

// Channel status bits
const CHST_IDLE: u32 = 0;
const CHST_ACTIVE: u32 = 1 << 0;
const CHST_COMPLETE: u32 = 1 << 1;
const CHST_ERROR: u32 = 1 << 7;

// Global control bits
const GCTRL_ENABLE: u32 = 1 << 0;
const GCTRL_RESET: u32 = 1 << 1;

// Max channels
const MAX_CHANNELS: usize = 8;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Scatter-gather list entry
#[derive(Debug, Clone, Copy)]
pub struct SgEntry {
    /// Physical source address
    pub src_addr: u64,
    /// Physical destination address
    pub dst_addr: u64,
    /// Transfer length in bytes
    pub length: u32,
}

/// Transfer direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Memory to memory
    MemToMem,
    /// Memory to peripheral (e.g. UART TX, SPI TX)
    MemToPeripheral,
    /// Peripheral to memory (e.g. UART RX, ADC)
    PeripheralToMem,
}

/// Burst size (number of beats per burst)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurstSize {
    Single = 0,
    Burst4 = 1,
    Burst8 = 2,
    Burst16 = 3,
}

/// Transfer width (data width per beat)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferWidth {
    Byte = 0,
    HalfWord = 1,
    Word = 2,
    DoubleWord = 3,
}

/// Channel priority level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Low = 0,
    Medium = 1,
    High = 2,
    Urgent = 3,
}

/// DMA channel status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    Free,
    Idle,
    Active,
    Complete,
    Error,
}

/// Completion callback type (channel id, success flag)
type CompletionCallback = fn(u8, bool);

/// Per-channel state
struct ChannelState {
    allocated: bool,
    status: ChannelStatus,
    direction: Direction,
    priority: Priority,
    burst: BurstSize,
    width: TransferWidth,
    /// Scatter-gather list (if any)
    sg_list: Vec<SgEntry>,
    /// Completion callback
    callback: Option<CompletionCallback>,
    /// Bytes transferred
    bytes_done: u64,
    /// Total transfers completed on this channel
    transfer_count: u64,
}

impl ChannelState {
    const fn new() -> Self {
        ChannelState {
            allocated: false,
            status: ChannelStatus::Free,
            direction: Direction::MemToMem,
            priority: Priority::Medium,
            burst: BurstSize::Burst4,
            width: TransferWidth::Word,
            sg_list: Vec::new(),
            callback: None,
            bytes_done: 0,
            transfer_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Inner driver state
// ---------------------------------------------------------------------------

struct DmaInner {
    initialized: bool,
    chip_id: u32,
    num_channels: usize,
    channels: [ChannelState; MAX_CHANNELS],
}

impl DmaInner {
    const fn new() -> Self {
        const EMPTY_CH: ChannelState = ChannelState::new();
        DmaInner {
            initialized: false,
            chip_id: 0,
            num_channels: 0,
            channels: [EMPTY_CH; MAX_CHANNELS],
        }
    }

    fn read_reg(&self, reg: u16) -> u32 {
        crate::io::inl(reg)
    }
    fn write_reg(&self, reg: u16, val: u32) {
        crate::io::outl(reg, val);
    }

    fn ch_reg(&self, ch: usize, offset: u16) -> u16 {
        CH_REG_BASE + (ch as u16) * CH_STRIDE + offset
    }

    fn hw_reset(&self) {
        self.write_reg(REG_GLOBAL_CTRL, GCTRL_RESET);
        for _ in 0..10000 {
            core::hint::spin_loop();
        }
    }

    /// Program a single (non-SG) transfer on a channel
    fn program_transfer(&self, ch: usize, src: u64, dst: u64, len: u32, direction: Direction) {
        self.write_reg(self.ch_reg(ch, CH_SRC_LO), src as u32);
        self.write_reg(self.ch_reg(ch, CH_SRC_HI), (src >> 32) as u32);
        self.write_reg(self.ch_reg(ch, CH_DST_LO), dst as u32);
        self.write_reg(self.ch_reg(ch, CH_DST_HI), (dst >> 32) as u32);
        self.write_reg(self.ch_reg(ch, CH_XFER_LEN), len);

        let dir_bits = match direction {
            Direction::MemToMem => CHCTRL_MEM2MEM,
            Direction::MemToPeripheral => CHCTRL_MEM2PERIPH,
            Direction::PeripheralToMem => CHCTRL_PERIPH2MEM,
        };
        self.write_reg(
            self.ch_reg(ch, CH_CTRL),
            CHCTRL_ENABLE | CHCTRL_INTR_COMPLETE | dir_bits,
        );
    }

    /// Program a scatter-gather transfer
    fn program_sg(&self, ch: usize, sg: &[SgEntry], direction: Direction) {
        if sg.is_empty() {
            return;
        }

        // For simplicity, program the first entry directly and chain via SG registers
        // A real implementation would write SG descriptors to a DMA-accessible memory region
        let first = &sg[0];
        self.write_reg(self.ch_reg(ch, CH_SRC_LO), first.src_addr as u32);
        self.write_reg(self.ch_reg(ch, CH_SRC_HI), (first.src_addr >> 32) as u32);
        self.write_reg(self.ch_reg(ch, CH_DST_LO), first.dst_addr as u32);
        self.write_reg(self.ch_reg(ch, CH_DST_HI), (first.dst_addr >> 32) as u32);
        self.write_reg(self.ch_reg(ch, CH_XFER_LEN), first.length);

        // SG descriptor chain address (simulated)
        let sg_base = 0x7000_0000u64 + (ch as u64) * 0x10000;
        self.write_reg(self.ch_reg(ch, CH_SG_ADDR_LO), sg_base as u32);
        self.write_reg(self.ch_reg(ch, CH_SG_ADDR_HI), (sg_base >> 32) as u32);
        self.write_reg(self.ch_reg(ch, CH_SG_COUNT), sg.len() as u32);

        let dir_bits = match direction {
            Direction::MemToMem => CHCTRL_MEM2MEM,
            Direction::MemToPeripheral => CHCTRL_MEM2PERIPH,
            Direction::PeripheralToMem => CHCTRL_PERIPH2MEM,
        };
        self.write_reg(
            self.ch_reg(ch, CH_CTRL),
            CHCTRL_ENABLE | CHCTRL_SG_MODE | CHCTRL_INTR_COMPLETE | dir_bits,
        );
    }

    /// Start a programmed transfer
    fn start_channel(&self, ch: usize) {
        let ctrl = self.read_reg(self.ch_reg(ch, CH_CTRL));
        // Fence: ensure all descriptor/address register writes are committed
        // to memory before ringing the START doorbell
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.write_reg(self.ch_reg(ch, CH_CTRL), ctrl | CHCTRL_START);
    }

    /// Abort a running transfer
    fn abort_channel(&mut self, ch: usize) {
        self.write_reg(self.ch_reg(ch, CH_CTRL), CHCTRL_ABORT);
        for _ in 0..10000 {
            let st = self.read_reg(self.ch_reg(ch, CH_STATUS));
            if st & CHST_ACTIVE == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.channels[ch].status = ChannelStatus::Idle;
    }

    /// Poll a channel for completion
    fn poll_channel(&mut self, ch: usize) {
        let st = self.read_reg(self.ch_reg(ch, CH_STATUS));
        if st & CHST_ACTIVE != 0 {
            return;
        } // Still running

        let bytes = self.read_reg(self.ch_reg(ch, CH_BYTES_DONE));
        self.channels[ch].bytes_done = self.channels[ch].bytes_done.saturating_add(bytes as u64);

        let success;
        if st & CHST_ERROR != 0 {
            self.channels[ch].status = ChannelStatus::Error;
            success = false;
        } else if st & CHST_COMPLETE != 0 {
            self.channels[ch].status = ChannelStatus::Complete;
            self.channels[ch].transfer_count = self.channels[ch].transfer_count.saturating_add(1);
            success = true;
        } else {
            return;
        }

        // Fire completion callback
        if let Some(cb) = self.channels[ch].callback {
            cb(ch as u8, success);
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static ENGINE: Mutex<DmaInner> = Mutex::new(DmaInner::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the DMA engine
pub fn init() {
    let mut dma = ENGINE.lock();

    let chip_id = dma.read_reg(REG_CHIP_ID);
    if chip_id == 0 || chip_id == 0xFFFF_FFFF {
        serial_println!("  DMA: no engine detected");
        return;
    }
    dma.chip_id = chip_id;
    dma.hw_reset();

    // Detect channel count
    let hw_count = dma.read_reg(REG_CHANNEL_COUNT) as usize;
    dma.num_channels = if hw_count > 0 && hw_count <= MAX_CHANNELS {
        hw_count
    } else {
        MAX_CHANNELS
    };

    // Enable global DMA engine and interrupts
    dma.write_reg(REG_GLOBAL_CTRL, GCTRL_ENABLE);
    dma.write_reg(REG_INTR_ENABLE, 0xFFFF_FFFF);

    // Initialize per-channel burst and priority defaults
    for ch in 0..dma.num_channels {
        dma.write_reg(
            dma.ch_reg(ch, CH_BURST_CFG),
            (BurstSize::Burst4 as u32) | ((TransferWidth::Word as u32) << 8),
        );
        dma.write_reg(dma.ch_reg(ch, CH_PRIORITY), Priority::Medium as u32);
    }

    dma.initialized = true;
    serial_println!(
        "  DMA: {} channels, chip={:#010X}",
        dma.num_channels,
        dma.chip_id
    );
    drop(dma);
    super::register("dma-engine", super::DeviceType::Other);
}

/// Allocate a free DMA channel, returns channel index
pub fn alloc_channel(priority: Priority) -> Option<u8> {
    let mut dma = ENGINE.lock();
    if !dma.initialized {
        return None;
    }
    for ch in 0..dma.num_channels {
        if !dma.channels[ch].allocated {
            dma.channels[ch].allocated = true;
            dma.channels[ch].status = ChannelStatus::Idle;
            dma.channels[ch].priority = priority;
            dma.write_reg(dma.ch_reg(ch, CH_PRIORITY), priority as u32);
            return Some(ch as u8);
        }
    }
    None
}

/// Release a DMA channel
pub fn free_channel(ch: u8) {
    let mut dma = ENGINE.lock();
    let idx = ch as usize;
    if idx >= dma.num_channels {
        return;
    }
    if dma.channels[idx].status == ChannelStatus::Active {
        dma.abort_channel(idx);
    }
    dma.channels[idx] = ChannelState::new();
}

/// Configure channel burst and transfer width
pub fn configure_channel(ch: u8, burst: BurstSize, width: TransferWidth) {
    let mut dma = ENGINE.lock();
    let idx = ch as usize;
    if idx >= dma.num_channels || !dma.channels[idx].allocated {
        return;
    }
    dma.channels[idx].burst = burst;
    dma.channels[idx].width = width;
    dma.write_reg(
        dma.ch_reg(idx, CH_BURST_CFG),
        (burst as u32) | ((width as u32) << 8),
    );
}

/// Set completion callback for a channel
pub fn set_callback(ch: u8, callback: CompletionCallback) {
    let mut dma = ENGINE.lock();
    let idx = ch as usize;
    if idx >= dma.num_channels {
        return;
    }
    dma.channels[idx].callback = Some(callback);
}

/// Start a simple memory-to-memory transfer
pub fn start_mem2mem(ch: u8, src: u64, dst: u64, len: u32) -> Result<(), &'static str> {
    let mut dma = ENGINE.lock();
    let idx = ch as usize;
    if !dma.initialized {
        return Err("DMA not initialized");
    }
    if idx >= dma.num_channels {
        return Err("invalid channel");
    }
    if !dma.channels[idx].allocated {
        return Err("channel not allocated");
    }
    if dma.channels[idx].status == ChannelStatus::Active {
        return Err("channel busy");
    }

    dma.channels[idx].direction = Direction::MemToMem;
    dma.channels[idx].status = ChannelStatus::Active;
    dma.program_transfer(idx, src, dst, len, Direction::MemToMem);
    dma.start_channel(idx);
    Ok(())
}

/// Start a scatter-gather transfer
pub fn start_sg_transfer(ch: u8, sg: &[SgEntry], direction: Direction) -> Result<(), &'static str> {
    let mut dma = ENGINE.lock();
    let idx = ch as usize;
    if !dma.initialized {
        return Err("DMA not initialized");
    }
    if idx >= dma.num_channels {
        return Err("invalid channel");
    }
    if !dma.channels[idx].allocated {
        return Err("channel not allocated");
    }
    if dma.channels[idx].status == ChannelStatus::Active {
        return Err("channel busy");
    }
    if sg.is_empty() {
        return Err("empty scatter-gather list");
    }

    dma.channels[idx].direction = direction;
    dma.channels[idx].status = ChannelStatus::Active;
    dma.channels[idx].sg_list = Vec::from(sg);
    dma.program_sg(idx, sg, direction);
    dma.start_channel(idx);
    Ok(())
}

/// Start a memory-to-peripheral transfer
pub fn start_mem2periph(ch: u8, src: u64, periph_addr: u64, len: u32) -> Result<(), &'static str> {
    let mut dma = ENGINE.lock();
    let idx = ch as usize;
    if !dma.initialized {
        return Err("DMA not initialized");
    }
    if idx >= dma.num_channels || !dma.channels[idx].allocated {
        return Err("invalid channel");
    }
    if dma.channels[idx].status == ChannelStatus::Active {
        return Err("channel busy");
    }

    dma.channels[idx].direction = Direction::MemToPeripheral;
    dma.channels[idx].status = ChannelStatus::Active;
    dma.program_transfer(idx, src, periph_addr, len, Direction::MemToPeripheral);
    dma.start_channel(idx);
    Ok(())
}

/// Abort a running transfer
pub fn abort(ch: u8) {
    let mut dma = ENGINE.lock();
    let idx = ch as usize;
    if idx < dma.num_channels && dma.channels[idx].allocated {
        dma.abort_channel(idx);
    }
}

/// Poll all channels for completion (call from interrupt or timer)
pub fn poll() {
    let mut dma = ENGINE.lock();
    if !dma.initialized {
        return;
    }
    // Clear global interrupt status
    let intr = dma.read_reg(REG_INTR_STATUS);
    if intr != 0 {
        dma.write_reg(REG_INTR_STATUS, intr);
    }
    for ch in 0..dma.num_channels {
        if dma.channels[ch].status == ChannelStatus::Active {
            dma.poll_channel(ch);
        }
    }
}

/// Get channel status
pub fn channel_status(ch: u8) -> ChannelStatus {
    let dma = ENGINE.lock();
    let idx = ch as usize;
    if idx >= MAX_CHANNELS {
        return ChannelStatus::Free;
    }
    dma.channels[idx].status
}

/// Get bytes transferred on a channel (cumulative)
pub fn bytes_transferred(ch: u8) -> u64 {
    let dma = ENGINE.lock();
    let idx = ch as usize;
    if idx >= MAX_CHANNELS {
        return 0;
    }
    dma.channels[idx].bytes_done
}

/// Get number of available (unallocated) channels
pub fn available_channels() -> usize {
    let dma = ENGINE.lock();
    dma.channels[..dma.num_channels]
        .iter()
        .filter(|c| !c.allocated)
        .count()
}
