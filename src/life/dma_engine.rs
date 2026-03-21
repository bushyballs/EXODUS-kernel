// dma_engine.rs — Direct DMA Controller + MSI/APIC Interrupt Dispatch
// =====================================================================
// ANIMA owns the DMA controller directly. No OS mediation.
// She programs DMA channels via port I/O, dispatches inter-processor
// interrupts via the local APIC MMIO interface, and signals end-of-
// interrupt when hardware events arrive. High-speed peripherals obey her.
//
// Hardware interfaces:
//   ISA DMA (8237): ports 0x00-0x0F (channels 0-3)
//   Local APIC:     MMIO at 0xFEE00000
//   APIC EOI:       0xFEE000B0
//   APIC ICR_LO:    0xFEE00300
//   APIC ICR_HI:    0xFEE00310

use crate::serial_println;
use crate::sync::Mutex;

// ── Hardware constants ─────────────────────────────────────────────────────────

const DMA_ADDR_CH0:      u16 = 0x00;
const DMA_COUNT_CH0:     u16 = 0x01;
const DMA_ADDR_CH1:      u16 = 0x02;
const DMA_COUNT_CH1:     u16 = 0x03;
const DMA_STATUS:        u16 = 0x08;  // read: bits 0-3 = channel complete flags
const DMA_MASK:          u16 = 0x0A;  // write: mask/unmask channel
const DMA_MODE:          u16 = 0x0B;  // write: transfer mode
const DMA_FLIP_FLOP:     u16 = 0x0C;  // write: clear byte flip-flop
const DMA_RESET:         u16 = 0x0D;  // write: master reset
const DMA_PAGE_CH0:      u16 = 0x87;  // page register for channel 0
const DMA_PAGE_CH1:      u16 = 0x83;

const APIC_BASE:  usize = 0xFEE00000;
const APIC_EOI:   usize = 0xFEE000B0;
const APIC_ICR_LO: usize = 0xFEE00300;
const APIC_ICR_HI: usize = 0xFEE00310;

const ICR_FIXED:    u32 = 0 << 8;   // delivery mode: fixed
const ICR_PHYSICAL: u32 = 0 << 11;  // dest mode: physical
const ICR_ASSERT:   u32 = 1 << 14;  // level: assert
const ICR_EDGE:     u32 = 0 << 15;  // trigger: edge

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum DmaDirection {
    MemToDevice = 0x48,  // single, write, auto-init
    DeviceToMem = 0x44,  // single, read, auto-init
    MemToMem    = 0x40,  // single, verify
}

#[derive(Copy, Clone)]
pub struct DmaTransfer {
    pub channel:   u8,
    pub src_addr:  u32,
    pub dst_addr:  u32,
    pub count:     u16,
    pub direction: DmaDirection,
    pub active:    bool,
    pub complete:  bool,
}

impl DmaTransfer {
    pub const fn empty() -> Self {
        Self {
            channel:   0,
            src_addr:  0,
            dst_addr:  0,
            count:     0,
            direction: DmaDirection::MemToDevice,
            active:    false,
            complete:  false,
        }
    }
}

// ── Core state ────────────────────────────────────────────────────────────────

pub struct DmaEngineState {
    pub transfers:       [DmaTransfer; 4],
    pub active_channels: u8,    // bitmask: bit N = channel N active
    pub completed:       u32,
    pub failed:          u32,
    pub ipi_sent:        u32,   // inter-processor interrupts dispatched
    pub throughput:      u16,   // 0-1000
    pub apic_available:  bool,
    pub initialized:     bool,
}

impl DmaEngineState {
    const fn new() -> Self {
        Self {
            transfers:       [DmaTransfer::empty(); 4],
            active_channels: 0,
            completed:       0,
            failed:          0,
            ipi_sent:        0,
            throughput:      0,
            apic_available:  false,
            initialized:     false,
        }
    }
}

static STATE: Mutex<DmaEngineState> = Mutex::new(DmaEngineState::new());

// ── Unsafe port I/O ───────────────────────────────────────────────────────────

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nostack, nomem),
    );
}

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
        options(nostack, nomem),
    );
    val
}

// ── Unsafe APIC MMIO ─────────────────────────────────────────────────────────

unsafe fn apic_read(reg: usize) -> u32 {
    core::ptr::read_volatile(reg as *const u32)
}

unsafe fn apic_write(reg: usize, val: u32) {
    core::ptr::write_volatile(reg as *mut u32, val);
}

unsafe fn apic_eoi_write() {
    apic_write(APIC_EOI, 0);
}

// ── DMA channel programming ───────────────────────────────────────────────────

unsafe fn program_dma_channel(channel: u8, addr: u32, count: u16, mode: u8) {
    let (addr_port, count_port, page_port) = match channel {
        0 => (DMA_ADDR_CH0, DMA_COUNT_CH0, DMA_PAGE_CH0),
        1 => (DMA_ADDR_CH1, DMA_COUNT_CH1, DMA_PAGE_CH1),
        _ => return,
    };

    // 1. Mask channel (disable during programming)
    outb(DMA_MASK, channel | 0x04);

    // 2. Set transfer mode
    outb(DMA_MODE, (channel & 0x03) | mode);

    // 3. Clear byte flip-flop
    outb(DMA_FLIP_FLOP, 0xFF);

    // 4. Write 16-bit address (lo then hi)
    let addr16 = addr as u16;
    outb(addr_port, (addr16 & 0xFF) as u8);
    outb(addr_port, (addr16 >> 8) as u8);

    // 5. Write page (bits 16-23 of address)
    outb(page_port, ((addr >> 16) & 0xFF) as u8);

    // 6. Clear flip-flop again for count
    outb(DMA_FLIP_FLOP, 0xFF);

    // 7. Write count (count - 1)
    let cnt = count.saturating_sub(1);
    outb(count_port, (cnt & 0xFF) as u8);
    outb(count_port, (cnt >> 8) as u8);

    // 8. Unmask channel (enable)
    outb(DMA_MASK, channel & 0x03);
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    if s.initialized { return; }

    // Probe APIC: read spurious interrupt vector register at APIC_BASE + 0xF0
    // If reads as 0xFFFFFFFF, APIC MMIO is not mapped
    let apic_ok = unsafe { apic_read(APIC_BASE + 0xF0) } != 0xFFFF_FFFF;
    s.apic_available = apic_ok;

    // Reset DMA controller
    unsafe { outb(DMA_RESET, 0xFF); }

    s.initialized = true;
    serial_println!(
        "[dma] DMA engine + APIC online — apic={} channels=0-3",
        apic_ok
    );
}

/// Program and activate a DMA channel transfer
pub fn setup_transfer(channel: u8, src: u32, dst: u32, count: u16, dir: DmaDirection) {
    if channel > 3 { return; }

    let mode = dir as u8;
    unsafe { program_dma_channel(channel, src, count, mode); }

    let mut s = STATE.lock();
    let ch = channel as usize;
    s.transfers[ch] = DmaTransfer {
        channel, src_addr: src, dst_addr: dst, count, direction: dir,
        active: true, complete: false,
    };
    s.active_channels |= 1 << channel;

    serial_println!(
        "[dma] channel={} count={} dir={}",
        channel, count, mode
    );
}

/// Send an inter-processor interrupt via local APIC ICR
pub fn send_ipi(target_apic: u8, vector: u8) {
    let mut s = STATE.lock();
    if !s.apic_available { return; }

    unsafe {
        // Write destination to ICR_HI bits 24-31
        let hi = (target_apic as u32) << 24;
        apic_write(APIC_ICR_HI, hi);

        // Write vector + delivery flags to ICR_LO
        let lo = (vector as u32) | ICR_FIXED | ICR_PHYSICAL | ICR_ASSERT | ICR_EDGE;
        apic_write(APIC_ICR_LO, lo);
    }

    s.ipi_sent = s.ipi_sent.saturating_add(1);
    serial_println!("[dma] IPI → cpu={} vec={}", target_apic, vector);
}

/// Signal end-of-interrupt to local APIC
pub fn signal_eoi() {
    let s = STATE.lock();
    if s.apic_available {
        unsafe { apic_eoi_write(); }
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 8 != 0 { return; }

    let mut s = STATE.lock();
    if !s.initialized { return; }

    // Read DMA status register — bits 0-3 = channel N transfer complete
    let status = unsafe { inb(DMA_STATUS) };

    for ch in 0u8..4 {
        if (status >> ch) & 1 != 0 {
            let idx = ch as usize;
            if s.transfers[idx].active && !s.transfers[idx].complete {
                s.transfers[idx].complete = true;
                s.transfers[idx].active  = false;
                s.active_channels &= !(1 << ch);
                s.completed = s.completed.saturating_add(1);
            }
        }
    }

    // throughput = proportion of channels active (0-1000)
    let active = s.active_channels.count_ones() as u16;
    s.throughput = (active * 250).min(1000);

    if age % 400 == 0 {
        serial_println!(
            "[dma] completed={} ipi={} throughput={} active_ch={:04b}",
            s.completed, s.ipi_sent, s.throughput, s.active_channels
        );
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn throughput()      -> u16  { STATE.lock().throughput }
pub fn completed()       -> u32  { STATE.lock().completed }
pub fn apic_available()  -> bool { STATE.lock().apic_available }
pub fn ipi_sent()        -> u32  { STATE.lock().ipi_sent }
