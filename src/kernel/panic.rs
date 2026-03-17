/// Kernel panic handler, stack unwinding, and crash dump generation.
///
/// Part of the AIOS kernel.
use alloc::string::String;
use alloc::vec::Vec;

/// Captured state at the time of a kernel panic.
pub struct PanicInfo {
    /// Human-readable panic message.
    pub message: String,
    /// Captured stack frames (instruction pointers).
    pub backtrace: Vec<usize>,
    /// CPU ID that panicked.
    pub cpu_id: usize,
}

impl PanicInfo {
    pub fn new(message: &str) -> Self {
        PanicInfo {
            message: String::from(message),
            backtrace: Vec::new(), // TODO(integration): walk RSP chain to populate
            cpu_id: 0,             // TODO(integration): read APIC ID for real CPU
        }
    }
}

/// Initiate a kernel panic with the given message.
pub fn panic_halt(message: &str) -> ! {
    // Disable interrupts immediately so no further IRQs fire on this CPU.
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    // Dump panic info to the serial console.
    let info = PanicInfo::new(message);
    crate::serial_println!("=== KERNEL PANIC ===");
    crate::serial_println!("CPU  : {}", info.cpu_id);
    crate::serial_println!("MSG  : {}", info.message);
    crate::serial_println!("HALT.");
    // Spin forever — no return.
    loop {
        core::hint::spin_loop();
    }
}

/// Initialize the panic subsystem (install handlers).
pub fn init() {
    // TODO: Register panic hook, set up crash dump target
}
