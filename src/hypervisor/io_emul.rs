/// I/O instruction emulation
///
/// Part of the AIOS.
///
/// Emulates port I/O and MMIO accesses from guest VMs. When a guest
/// performs an IN or OUT instruction on a trapped port, the hypervisor
/// intercepts it and dispatches to the registered handler here.

use alloc::vec::Vec;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// Global I/O emulator singleton.
static IO_EMULATOR: Mutex<Option<IoEmulator>> = Mutex::new(None);

/// Emulates I/O port and MMIO accesses from guest VMs.
pub struct IoEmulator {
    port_handlers: Vec<PortHandler>,
}

struct PortHandler {
    port: u16,
    read: fn(u16) -> u32,
    write: fn(u16, u32),
}

// --- Default emulated device handlers ---

/// Emulated PIT (Programmable Interval Timer) — ports 0x40-0x43.
fn pit_read(port: u16) -> u32 {
    match port {
        0x40 => {
            // Channel 0 counter — return a decrementing value.
            // Read TSC and derive a pseudo-counter.
            let tsc = rdtsc();
            // PIT runs at ~1.193182 MHz; scale TSC down.
            ((tsc / 1000) & 0xFFFF) as u32
        }
        0x41 => 0, // Channel 1 (unused in modern systems).
        0x42 => 0, // Channel 2 (PC speaker).
        0x43 => 0, // Control register (write-only, return 0).
        _ => 0,
    }
}

fn pit_write(port: u16, value: u32) {
    // Silently accept PIT writes — the virtual PIT state is not
    // strictly tracked since we use the TSC for timing.
    let _ = (port, value);
}

/// Emulated PIC (Programmable Interrupt Controller) — ports 0x20-0x21, 0xA0-0xA1.
fn pic_read(port: u16) -> u32 {
    match port {
        0x20 => 0, // Master PIC command: no IRQs pending.
        0x21 => 0xFF, // Master PIC data: all IRQs masked.
        0xA0 => 0, // Slave PIC command.
        0xA1 => 0xFF, // Slave PIC data: all IRQs masked.
        _ => 0,
    }
}

fn pic_write(port: u16, value: u32) {
    match port {
        0x20 => {
            // ICW/OCW to master PIC.
            if value & 0x20 != 0 {
                // EOI (End of Interrupt).
            }
        }
        0x21 => {
            // IMR (Interrupt Mask Register) for master PIC.
        }
        0xA0 => {
            // ICW/OCW to slave PIC.
            if value & 0x20 != 0 {
                // EOI.
            }
        }
        0xA1 => {
            // IMR for slave PIC.
        }
        _ => {}
    }
}

/// Emulated serial port (UART 16550) — COM1 at 0x3F8-0x3FF.
fn uart_read(port: u16) -> u32 {
    let offset = port - 0x3F8;
    match offset {
        0 => 0,    // RBR: no data available.
        1 => 0,    // IER: interrupts disabled.
        2 => 0x01, // IIR: no interrupt pending.
        3 => 0x03, // LCR: 8N1.
        4 => 0,    // MCR.
        5 => 0x60, // LSR: transmitter holding register empty + transmitter empty.
        6 => 0,    // MSR.
        7 => 0,    // Scratch register.
        _ => 0,
    }
}

fn uart_write(port: u16, value: u32) {
    let offset = port - 0x3F8;
    match offset {
        0 => {
            // THR: guest is writing a character to serial output.
            // Forward to the host serial port for debugging.
            let ch = (value & 0xFF) as u8;
            if ch.is_ascii() {
                serial_println!("    [io_emul] Guest serial output: '{}'", ch as char);
            }
        }
        _ => {
            // Accept other UART register writes silently.
        }
    }
}

/// Emulated CMOS/RTC — ports 0x70-0x71.
fn cmos_read(port: u16) -> u32 {
    match port {
        0x71 => {
            // CMOS data port — return a default value.
            // Commonly read registers: seconds (0x00), minutes (0x02), hours (0x04).
            0
        }
        _ => 0,
    }
}

fn cmos_write(port: u16, value: u32) {
    let _ = (port, value);
    // Accept CMOS address/data writes silently.
}

/// Emulated PS/2 keyboard controller — port 0x60, 0x64.
fn ps2_read(port: u16) -> u32 {
    match port {
        0x60 => 0, // Data port: no key pressed.
        0x64 => 0x1C, // Status: output buffer empty, input buffer empty, system flag set.
        _ => 0,
    }
}

fn ps2_write(port: u16, value: u32) {
    let _ = (port, value);
}

/// Default handler for unregistered ports — returns 0xFF (all bits set).
fn default_read(_port: u16) -> u32 {
    0xFFFF_FFFF
}

fn default_write(_port: u16, _value: u32) {
    // Silently discard writes to unhandled ports.
}

impl IoEmulator {
    pub fn new() -> Self {
        let mut emu = IoEmulator {
            port_handlers: Vec::new(),
        };

        // Register default device emulators.
        // PIT (0x40-0x43).
        for port in 0x40..=0x43 {
            emu.register_handler(port, pit_read, pit_write);
        }

        // PIC master (0x20-0x21) and slave (0xA0-0xA1).
        for port in 0x20..=0x21 {
            emu.register_handler(port, pic_read, pic_write);
        }
        for port in 0xA0..=0xA1 {
            emu.register_handler(port, pic_read, pic_write);
        }

        // UART COM1 (0x3F8-0x3FF).
        for port in 0x3F8..=0x3FF {
            emu.register_handler(port, uart_read, uart_write);
        }

        // CMOS/RTC (0x70-0x71).
        emu.register_handler(0x70, cmos_read, cmos_write);
        emu.register_handler(0x71, cmos_read, cmos_write);

        // PS/2 controller (0x60, 0x64).
        emu.register_handler(0x60, ps2_read, ps2_write);
        emu.register_handler(0x64, ps2_read, ps2_write);

        emu
    }

    /// Register a port I/O handler.
    pub fn register_handler(&mut self, port: u16, read: fn(u16) -> u32, write: fn(u16, u32)) {
        // Replace existing handler if one exists for this port.
        for handler in self.port_handlers.iter_mut() {
            if handler.port == port {
                handler.read = read;
                handler.write = write;
                return;
            }
        }
        self.port_handlers.push(PortHandler { port, read, write });
    }

    /// Handle a guest I/O port read.
    ///
    /// `size` is the access width in bytes (1, 2, or 4).
    /// Returns the value read, masked to the appropriate width.
    pub fn handle_port_read(&self, port: u16, size: u8) -> u32 {
        let raw = self.find_read_handler(port)(port);

        // Mask to the requested access width.
        match size {
            1 => raw & 0xFF,
            2 => raw & 0xFFFF,
            4 => raw,
            _ => raw & 0xFF,
        }
    }

    /// Handle a guest I/O port write.
    ///
    /// `size` is the access width in bytes (1, 2, or 4).
    pub fn handle_port_write(&self, port: u16, value: u32, size: u8) {
        let masked = match size {
            1 => value & 0xFF,
            2 => value & 0xFFFF,
            4 => value,
            _ => value & 0xFF,
        };

        self.find_write_handler(port)(port, masked);
    }

    /// Find the read handler for a port, or return the default.
    fn find_read_handler(&self, port: u16) -> fn(u16) -> u32 {
        for handler in &self.port_handlers {
            if handler.port == port {
                return handler.read;
            }
        }
        default_read
    }

    /// Find the write handler for a port, or return the default.
    fn find_write_handler(&self, port: u16) -> fn(u16, u32) {
        for handler in &self.port_handlers {
            if handler.port == port {
                return handler.write;
            }
        }
        default_write
    }
}

/// Read TSC for pseudo-timing.
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    let emu = IoEmulator::new();
    *IO_EMULATOR.lock() = Some(emu);
    serial_println!("    [io_emul] I/O emulation subsystem initialized");
}
