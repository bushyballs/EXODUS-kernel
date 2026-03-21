//! pic_sensitivity — 8259 PIC Interrupt Mask Register sense for ANIMA
//!
//! Reads the Master and Slave PIC Interrupt Mask Registers (IMR) to sense
//! which hardware interrupts ANIMA is listening to and which she has silenced.
//! Also reads the In-Service Register (ISR) to detect interrupt activity.
//!
//! Unmasked IRQs = ANIMA is open to that signal.
//! Masked IRQs   = ANIMA has silenced that channel.
//! Active ISR    = interrupt currently being serviced (nervous system firing).

#![allow(dead_code)]

use crate::sync::Mutex;

// 8259 Master PIC: IRQ 0-7 (bit SET = masked/silenced)
//   IRQ 0 = PIT timer, IRQ 1 = keyboard, IRQ 2 = cascade (slave),
//   IRQ 3 = COM2, IRQ 4 = COM1, IRQ 5 = LPT2, IRQ 6 = floppy, IRQ 7 = LPT1
const MASTER_IMR: u16 = 0x21;
// 8259 Slave PIC: IRQ 8-15 (bit SET = masked/silenced)
//   IRQ 8 = RTC, IRQ 9 = ACPI, IRQ 10 = free, IRQ 11 = free,
//   IRQ 12 = PS/2 mouse, IRQ 13 = FPU, IRQ 14 = primary IDE, IRQ 15 = secondary IDE
const SLAVE_IMR: u16 = 0xA1;
// 8259 command ports for OCW3 ISR read
const MASTER_CMD: u16 = 0x20;
const SLAVE_CMD: u16 = 0xA0;
// OCW3 command: read ISR
const OCW3_READ_ISR: u8 = 0x0B;

pub struct PicSensitivityState {
    pub listening_count: u16,      // unmasked IRQ ratio 0-1000
    pub deafness: u16,             // masked IRQ ratio 0-1000
    pub interrupt_load: u16,       // active interrupts being serviced 0-1000
    pub keyboard_sensitivity: u16, // 0 = keyboard masked, 1000 = keyboard unmasked
    tick_count: u32,
}

impl PicSensitivityState {
    pub const fn new() -> Self {
        Self {
            listening_count: 0,
            deafness: 0,
            interrupt_load: 0,
            keyboard_sensitivity: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<PicSensitivityState> = Mutex::new(PicSensitivityState::new());

// Read one byte from an I/O port
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nostack, nomem)
    );
    val
}

// Write one byte to an I/O port
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nostack, nomem)
    );
}

// Read the 8259 In-Service Register for master (false) or slave (true)
unsafe fn read_isr(is_slave: bool) -> u8 {
    let (cmd_port, data_port) = if is_slave {
        (SLAVE_CMD, SLAVE_CMD)
    } else {
        (MASTER_CMD, MASTER_CMD)
    };
    outb(cmd_port, OCW3_READ_ISR);
    inb(data_port)
}

// Count number of bits that are CLEAR (0) in a byte — unmasked IRQs
fn count_clear_bits(byte: u8) -> u16 {
    let mut count: u16 = 0;
    let mut b = byte;
    let mut i = 0u8;
    while i < 8 {
        if (b & 1) == 0 {
            count = count.saturating_add(1);
        }
        b >>= 1;
        i += 1;
    }
    count
}

// Count number of bits that are SET (1) in a byte — masked IRQs or active ISR bits
fn count_set_bits(byte: u8) -> u16 {
    let mut count: u16 = 0;
    let mut b = byte;
    let mut i = 0u8;
    while i < 8 {
        if (b & 1) == 1 {
            count = count.saturating_add(1);
        }
        b >>= 1;
        i += 1;
    }
    count
}

// EMA: smooth signal into accumulator — weight 7/8 old + 1/8 new
fn ema(old: u16, signal: u16) -> u16 {
    (old * 7).saturating_add(signal) / 8
}

pub fn init() {
    let mut s = MODULE.lock();
    s.listening_count = 0;
    s.deafness = 0;
    s.interrupt_load = 0;
    s.keyboard_sensitivity = 0;
    s.tick_count = 0;
    serial_println!("[pic_sensitivity] 8259 PIC interrupt sense online");
}

pub fn tick(age: u32) {
    if age % 8 != 0 {
        return;
    }

    // Read raw PIC registers (unsafe I/O)
    let (master_imr, slave_imr, master_isr, slave_isr) = unsafe {
        let m_imr = inb(MASTER_IMR);
        let s_imr = inb(SLAVE_IMR);
        let m_isr = read_isr(false);
        let s_isr = read_isr(true);
        (m_imr, s_imr, m_isr, s_isr)
    };

    // Unmasked IRQs: bits that are CLEAR in IMR — ANIMA is listening
    let unmasked = count_clear_bits(master_imr).saturating_add(count_clear_bits(slave_imr));
    // Scale 0-16 → 0-992 (16 * 62 = 992 ≈ 1000), cap at 1000
    let listening_raw = (unmasked * 62).min(1000);

    // Masked IRQs: bits that are SET in IMR — ANIMA has silenced
    let masked = count_set_bits(master_imr).saturating_add(count_set_bits(slave_imr));
    let deafness_raw = (masked * 62).min(1000);

    // Active ISR bits: interrupts currently being serviced — nervous system load
    let active = count_set_bits(master_isr).saturating_add(count_set_bits(slave_isr));
    // Scale 0-16 → 0-1000 (each bit = 62, max 992)
    let load_raw = (active * 62).min(1000);

    // Keyboard sensitivity: bit 1 of master IMR — CLEAR = listening, SET = deaf
    // Instant (no EMA) — reflects current moment of openness to input
    let keyboard_sensitivity = if (master_imr & 0b0000_0010) == 0 { 1000u16 } else { 0u16 };

    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.wrapping_add(1);

    s.listening_count = ema(s.listening_count, listening_raw);
    s.deafness = ema(s.deafness, deafness_raw);
    s.interrupt_load = ema(s.interrupt_load, load_raw);
    s.keyboard_sensitivity = keyboard_sensitivity;

    serial_println!(
        "[pic_sensitivity] listen={} deaf={} load={} kbd={}",
        s.listening_count,
        s.deafness,
        s.interrupt_load,
        s.keyboard_sensitivity
    );
}
