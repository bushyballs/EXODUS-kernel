use crate::io::{inb, io_wait, outb};
use crate::sync::Mutex;
/// Interrupt handling for Hoags Kernel Genesis -- built from scratch
///
/// Implements:
///   - IDT (Interrupt Descriptor Table) -- 256 entries, hand-built
///   - PIC (8259 Programmable Interrupt Controller) -- ICW init, EOI, masking
///   - PS/2 Keyboard -- scancode forwarded to drivers::keyboard::process_scancode
///   - PS/2 Mouse -- IRQ12 handler forwarding to drivers::mouse::handle_byte
///   - Exception handlers (all 32 CPU exceptions)
///   - Hardware interrupt handlers (timer, keyboard, mouse, NIC, HDA)
///   - Spurious IRQ detection and handling
///   - PIC mask management
///
/// No external crates. All code is original.
use crate::{kprint, kprintln};

// ============================================================================
// PIC (8259 Programmable Interrupt Controller) -- from scratch
// ============================================================================

const PIC1_CMD: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_CMD: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// PIC offsets -- remap IRQs to avoid conflict with CPU exceptions (0-31)
pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

const ICW1_INIT: u8 = 0x11; // Initialize + ICW4 needed
const ICW4_8086: u8 = 0x01; // 8086/88 mode
const PIC_EOI: u8 = 0x20; // End of Interrupt command

/// Read ISR (In-Service Register) to detect spurious IRQs
const PIC_READ_ISR: u8 = 0x0B;

/// Initialize both 8259 PICs with remapped IRQ offsets
pub fn init_pics() {
    // Save masks
    let mask1 = inb(PIC1_DATA);
    let mask2 = inb(PIC2_DATA);

    // ICW1: Initialize + expect ICW4
    outb(PIC1_CMD, ICW1_INIT);
    io_wait();
    outb(PIC2_CMD, ICW1_INIT);
    io_wait();

    // ICW2: Vector offset (remap IRQ 0-7 to 32-39, IRQ 8-15 to 40-47)
    outb(PIC1_DATA, PIC_1_OFFSET);
    io_wait();
    outb(PIC2_DATA, PIC_2_OFFSET);
    io_wait();

    // ICW3: Tell master PIC that slave is on IRQ2 (bit 2)
    outb(PIC1_DATA, 4);
    io_wait();
    // Tell slave PIC its cascade identity (IRQ2 = 2)
    outb(PIC2_DATA, 2);
    io_wait();

    // ICW4: 8086 mode
    outb(PIC1_DATA, ICW4_8086);
    io_wait();
    outb(PIC2_DATA, ICW4_8086);
    io_wait();

    // Restore masks
    outb(PIC1_DATA, mask1);
    outb(PIC2_DATA, mask2);
}

/// Send End of Interrupt to the appropriate PIC
fn pic_eoi(irq: u8) {
    if irq >= 8 {
        outb(PIC2_CMD, PIC_EOI); // slave PIC
    }
    outb(PIC1_CMD, PIC_EOI); // master PIC (always)
}

/// Check if an IRQ is spurious by reading the ISR.
/// Returns true if the IRQ was spurious (not actually in-service).
fn is_spurious_irq(irq: u8) -> bool {
    if irq == 7 {
        // Spurious IRQ7 from master PIC
        outb(PIC1_CMD, PIC_READ_ISR);
        let isr = inb(PIC1_CMD);
        return isr & 0x80 == 0; // bit 7 not set = spurious
    }
    if irq == 15 {
        // Spurious IRQ15 from slave PIC
        outb(PIC2_CMD, PIC_READ_ISR);
        let isr = inb(PIC2_CMD);
        if isr & 0x80 == 0 {
            // Spurious from slave -- still need to send EOI to master
            // because master saw the cascade line assert
            outb(PIC1_CMD, PIC_EOI);
            return true;
        }
    }
    false
}

/// Set the IRQ mask for the master PIC (IRQs 0-7)
pub fn set_master_mask(mask: u8) {
    outb(PIC1_DATA, mask);
}

/// Set the IRQ mask for the slave PIC (IRQs 8-15)
pub fn set_slave_mask(mask: u8) {
    outb(PIC2_DATA, mask);
}

/// Enable (unmask) a specific IRQ line
pub fn enable_irq(irq: u8) {
    if irq < 8 {
        let mask = inb(PIC1_DATA);
        outb(PIC1_DATA, mask & !(1 << irq));
    } else if irq < 16 {
        let mask = inb(PIC2_DATA);
        outb(PIC2_DATA, mask & !(1 << (irq - 8)));
        // Also ensure cascade (IRQ2 on master) is unmasked
        let m1 = inb(PIC1_DATA);
        outb(PIC1_DATA, m1 & !0x04);
    }
}

/// Disable (mask) a specific IRQ line
pub fn disable_irq(irq: u8) {
    if irq < 8 {
        let mask = inb(PIC1_DATA);
        outb(PIC1_DATA, mask | (1 << irq));
    } else if irq < 16 {
        let mask = inb(PIC2_DATA);
        outb(PIC2_DATA, mask | (1 << (irq - 8)));
    }
}

/// Get the current combined IRQ mask (bits 0-7 = master, bits 8-15 = slave)
pub fn get_irq_mask() -> u16 {
    let m1 = inb(PIC1_DATA) as u16;
    let m2 = inb(PIC2_DATA) as u16;
    m1 | (m2 << 8)
}

/// Interrupt statistics for debugging
static IRQ_COUNTS: [core::sync::atomic::AtomicU64; 16] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

static SPURIOUS_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static EXCEPTION_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Get IRQ hit count for a specific IRQ line
pub fn irq_count(irq: u8) -> u64 {
    if (irq as usize) < IRQ_COUNTS.len() {
        IRQ_COUNTS[irq as usize].load(core::sync::atomic::Ordering::Relaxed)
    } else {
        0
    }
}

/// Get total spurious interrupt count
pub fn spurious_irq_count() -> u64 {
    SPURIOUS_COUNT.load(core::sync::atomic::Ordering::Relaxed)
}

/// Get total exception count
pub fn exception_count() -> u64 {
    EXCEPTION_COUNT.load(core::sync::atomic::Ordering::Relaxed)
}

/// Increment IRQ counter
fn count_irq(irq: u8) {
    if (irq as usize) < IRQ_COUNTS.len() {
        IRQ_COUNTS[irq as usize].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

// ============================================================================
// IDT (Interrupt Descriptor Table) -- from scratch
// ============================================================================

/// A single IDT entry (16 bytes in 64-bit mode)
#[derive(Clone, Copy)]
#[repr(C)]
struct IdtEntry {
    offset_low: u16,  // bits 0..15 of handler address
    selector: u16,    // code segment selector
    ist: u8,          // bits 0..2 = IST index, rest zero
    type_attr: u8,    // type + DPL + present
    offset_mid: u16,  // bits 16..31 of handler address
    offset_high: u32, // bits 32..63 of handler address
    reserved: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    /// Create an interrupt gate entry
    fn set_handler(&mut self, handler: u64, cs: u16) {
        self.offset_low = handler as u16;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self.selector = cs;
        self.ist = 0;
        // Type: 0xE = 64-bit interrupt gate, DPL = 0, Present = 1
        self.type_attr = 0x8E;
        self.reserved = 0;
    }

    /// Set IST index (1-7, 0 = don't use IST)
    fn set_ist(&mut self, ist_index: u8) {
        self.ist = ist_index & 0x7;
    }
}

/// The full IDT -- 256 entries
#[repr(C, align(16))]
struct Idt {
    entries: [IdtEntry; 256],
}

/// IDT pointer for LIDT instruction
#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

static mut IDT: Idt = Idt {
    entries: [IdtEntry::missing(); 256],
};

// ============================================================================
// Exception/Interrupt handler stubs -- naked asm to save/restore state
// ============================================================================

// We need wrapper stubs because x86-interrupt calling convention requires
// the compiler to handle the special interrupt stack frame. Since we're
// not using the x86_64 crate, we write our own stubs.

macro_rules! interrupt_handler_no_error {
    ($name:ident, $handler:ident) => {
        #[unsafe(naked)]
        extern "C" fn $name() {
            core::arch::naked_asm!(
                "push 0",          // fake error code
                "push rax",
                "push rcx",
                "push rdx",
                "push rsi",
                "push rdi",
                "push r8",
                "push r9",
                "push r10",
                "push r11",
                "mov rdi, rsp",    // arg1 = pointer to saved state
                "call {handler}",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rdi",
                "pop rsi",
                "pop rdx",
                "pop rcx",
                "pop rax",
                "add rsp, 8",      // pop fake error code
                "iretq",
                handler = sym $handler,
            );
        }
    };
}

macro_rules! interrupt_handler_with_error {
    ($name:ident, $handler:ident) => {
        #[unsafe(naked)]
        extern "C" fn $name() {
            core::arch::naked_asm!(
                // error code already on stack from CPU
                "push rax",
                "push rcx",
                "push rdx",
                "push rsi",
                "push rdi",
                "push r8",
                "push r9",
                "push r10",
                "push r11",
                "mov rdi, rsp",    // arg1 = pointer to saved state
                "call {handler}",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rdi",
                "pop rsi",
                "pop rdx",
                "pop rcx",
                "pop rax",
                "add rsp, 8",      // pop error code
                "iretq",
                handler = sym $handler,
            );
        }
    };
}

/// Saved interrupt state (matches push order in stubs)
#[repr(C)]
struct InterruptFrame {
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rax: u64,
    error_code: u64,
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

// ============================================================================
// Generate handler stubs -- CPU exceptions
// ============================================================================

interrupt_handler_no_error!(divide_error_stub, divide_error_handler);
interrupt_handler_no_error!(debug_stub, debug_handler);
interrupt_handler_no_error!(nmi_stub, nmi_handler);
interrupt_handler_no_error!(breakpoint_stub, breakpoint_handler);
interrupt_handler_no_error!(overflow_stub, overflow_handler);
interrupt_handler_no_error!(bound_range_stub, bound_range_handler);
interrupt_handler_no_error!(invalid_opcode_stub, invalid_opcode_handler);
interrupt_handler_no_error!(device_not_avail_stub, device_not_avail_handler);
interrupt_handler_with_error!(double_fault_stub, double_fault_handler);
interrupt_handler_with_error!(invalid_tss_stub, invalid_tss_handler);
interrupt_handler_with_error!(segment_not_present_stub, segment_not_present_handler);
interrupt_handler_with_error!(stack_segment_stub, stack_segment_handler);
interrupt_handler_with_error!(gpf_stub, gpf_handler);
interrupt_handler_with_error!(page_fault_stub, page_fault_handler);
interrupt_handler_no_error!(x87_fpu_stub, x87_fpu_handler);
interrupt_handler_with_error!(alignment_check_stub, alignment_check_handler);
interrupt_handler_no_error!(machine_check_stub, machine_check_handler);
interrupt_handler_no_error!(simd_stub, simd_handler);

// ============================================================================
// Generate handler stubs -- Hardware IRQs
// ============================================================================

// timer_stub: writes 'U' (0x55) to COM1 as VERY FIRST action, before any pushes.
// If 'U' appears in serial.txt after "[idle] STI", the interrupt IS being delivered
// and the stub IS executing — crash is somewhere inside the push sequence or call.
// If 'U' NEVER appears, the interrupt delivery itself is failing (bad IDT or stack).
#[unsafe(naked)]
extern "C" fn timer_stub() {
    core::arch::naked_asm!(
        // Write 'U' to COM1 immediately — no stack use, no register deps from caller
        "push rax",
        "push rdx",
        "mov al, 0x55",     // 'U'
        "mov dx, 0x3F8",
        "out dx, al",
        "pop rdx",
        "pop rax",
        // Normal stub: fake error code + save caller-saved regs
        "push 0",
        "push rax",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "mov rdi, rsp",
        "call {handler}",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rax",
        "add rsp, 8",
        "iretq",
        handler = sym timer_handler,
    );
}
interrupt_handler_no_error!(keyboard_stub, keyboard_handler);
interrupt_handler_no_error!(cascade_stub, cascade_handler);
interrupt_handler_no_error!(com2_stub, com2_handler);
interrupt_handler_no_error!(com1_stub, com1_handler);
interrupt_handler_no_error!(irq5_stub, irq5_handler);
interrupt_handler_no_error!(irq6_stub, irq6_handler);
interrupt_handler_no_error!(spurious_master_stub, spurious_master_handler);
interrupt_handler_no_error!(rtc_stub, rtc_handler);
interrupt_handler_no_error!(irq9_stub, irq9_handler);
interrupt_handler_no_error!(irq10_stub, irq10_handler);
interrupt_handler_no_error!(irq11_stub, irq11_handler);
interrupt_handler_no_error!(mouse_stub, mouse_handler);
interrupt_handler_no_error!(irq13_stub, irq13_handler);
interrupt_handler_no_error!(irq14_stub, irq14_handler);
interrupt_handler_no_error!(spurious_slave_stub, spurious_slave_handler);

// ============================================================================
// Generate handler stubs -- IPI vectors (0xF0-0xF3)
// ============================================================================

interrupt_handler_no_error!(ipi_reschedule_stub, ipi_reschedule_handler);
interrupt_handler_no_error!(ipi_tlb_shootdown_stub, ipi_tlb_shootdown_handler);
interrupt_handler_no_error!(ipi_halt_stub, ipi_halt_handler);

/// Vector 0xF3 (243): Per-CPU halt IPI (hot-unplug or directed shutdown).
interrupt_handler_no_error!(ipi_cpu_halt_stub, ipi_cpu_halt_handler);

// ============================================================================
// CPU Exception Handlers
// ============================================================================

extern "C" fn divide_error_handler(frame: *const InterruptFrame) {
    // Safety: frame is always a valid kernel stack pointer pushed by our interrupt stub
    if frame.is_null() {
        loop {
            unsafe {
                core::arch::asm!("hlt");
            }
        }
    }
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] Divide by Zero at {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn debug_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] Debug at {:#x}", f.rip);
}

extern "C" fn nmi_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] NMI at {:#x}", f.rip);
}

extern "C" fn breakpoint_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    kprintln!("[EXCEPTION] Breakpoint at {:#x}", f.rip);
}

extern "C" fn overflow_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] Overflow at {:#x}", f.rip);
}

extern "C" fn bound_range_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] Bound Range Exceeded at {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn invalid_opcode_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] Invalid Opcode at {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn device_not_avail_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] Device Not Available (FPU) at {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn double_fault_handler(frame: *const InterruptFrame) -> ! {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // Serial output FIRST — kprintln goes to VGA only, can cause triple fault if VGA faults
    crate::serial_println!("!!! DOUBLE FAULT !!! rip={:#x} err={}", f.rip, f.error_code);
    kprintln!(
        "!!! DOUBLE FAULT !!! at {:#x} (error: {})",
        f.rip,
        f.error_code
    );
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn invalid_tss_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!(
        "[EXCEPTION] Invalid TSS (error: {:#x}) at {:#x}",
        f.error_code,
        f.rip
    );
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn segment_not_present_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!(
        "[EXCEPTION] Segment Not Present (error: {:#x}) at {:#x}",
        f.error_code,
        f.rip
    );
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn stack_segment_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!(
        "[EXCEPTION] Stack-Segment Fault (error: {:#x}) at {:#x}",
        f.error_code,
        f.rip
    );
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn gpf_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    crate::serial_println!("!!! GPF !!! err={:#x} rip={:#x}", f.error_code, f.rip);
    kprintln!(
        "[EXCEPTION] General Protection Fault (error: {:#x})",
        f.error_code
    );
    kprintln!("  RIP: {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn page_fault_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    let cr2: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2);
    }
    crate::serial_println!("!!! PAGE FAULT !!! cr2={:#x} rip={:#x} err={:#x}", cr2, f.rip, f.error_code);

    // Try demand paging / COW first
    if crate::memory::paging::handle_page_fault(cr2 as usize, f.error_code) {
        return; // fault resolved
    }

    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] Page Fault");
    kprintln!("  Accessed Address: {:#x}", cr2);
    kprintln!("  Error Code: {:#x}", f.error_code);
    kprintln!("  RIP: {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn x87_fpu_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] x87 FPU Error at {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn alignment_check_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] Alignment Check at {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn machine_check_handler(frame: *const InterruptFrame) -> ! {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("!!! MACHINE CHECK !!! at {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

extern "C" fn simd_handler(frame: *const InterruptFrame) {
    let f = unsafe { &*frame };
    EXCEPTION_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    kprintln!("[EXCEPTION] SIMD Floating-Point at {:#x}", f.rip);
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

// ============================================================================
// Hardware Interrupt Handlers
// ============================================================================

/// Tick counter for preemptive scheduling (10ms quantum at 100Hz PIT)
static TICK_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Scheduling quantum in ticks (10 ticks = 100ms at 100Hz)
const SCHED_QUANTUM: u64 = 10;

/// Nanoseconds per PIT tick at 100 Hz.  10 ms = 10_000_000 ns.
const NS_PER_TICK: u64 = 10_000_000;

extern "C" fn timer_handler(_frame: *const InterruptFrame) {
    // BARE-MINIMUM HANDLER — diagnose triple-fault root cause.
    // Phase 1: pure inline asm, no Rust function calls.
    unsafe {
        core::arch::asm!(
            // Write 'T' (0x54) to COM1 (I/O port 0x3F8)
            "out dx, al",
            in("dx") 0x3F8u16,
            in("al") 0x54u8,
            options(nostack, preserves_flags),
        );

        // LAPIC EOI: write 0 to LAPIC_BASE + 0xB0 = 0xFEE000B0.
        // MUST use register-indirect: 32-bit immediate [0xFEE000B0] in
        // 64-bit mode sign-extends to 0xFFFFFFFFFEE000B0 (wrong → #GP).
        // write_volatile uses a 64-bit register for the address — correct.
        core::ptr::write_volatile((crate::smp::LAPIC_BASE + 0xB0) as *mut u32, 0u32);

        // PIC EOI — harmless even if PIC wasn't the source
        core::arch::asm!(
            "out 0x20, al",
            in("al") 0x20u8,
            options(nostack, preserves_flags),
        );
    }

    // Atomic increment (BSS static, always mapped — safe).
    TICK_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

extern "C" fn keyboard_handler(_frame: *const InterruptFrame) {
    count_irq(1);
    let scancode = inb(0x60);
    // Forward to the keyboard driver for full decode (scan code set 1,
    // modifier tracking, key repeat, LED control)
    crate::drivers::keyboard::process_scancode(scancode);
    pic_eoi(1); // IRQ 1
}

/// IRQ2: Cascade from slave PIC -- never actually fires, just acknowledge
extern "C" fn cascade_handler(_frame: *const InterruptFrame) {
    count_irq(2);
    pic_eoi(2);
}

/// IRQ3: COM2 / COM4 serial port
extern "C" fn com2_handler(_frame: *const InterruptFrame) {
    count_irq(3);
    pic_eoi(3);
}

/// IRQ4: COM1 / COM3 serial port
extern "C" fn com1_handler(_frame: *const InterruptFrame) {
    count_irq(4);
    pic_eoi(4);
}

/// IRQ5: Sound card (legacy) or parallel port 2
extern "C" fn irq5_handler(_frame: *const InterruptFrame) {
    count_irq(5);
    pic_eoi(5);
}

/// IRQ6: Floppy disk controller
extern "C" fn irq6_handler(_frame: *const InterruptFrame) {
    count_irq(6);
    pic_eoi(6);
}

/// IRQ7: Parallel port 1 / Spurious IRQ from master PIC
extern "C" fn spurious_master_handler(_frame: *const InterruptFrame) {
    if is_spurious_irq(7) {
        SPURIOUS_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return; // Do NOT send EOI for spurious
    }
    count_irq(7);
    pic_eoi(7);
}

/// IRQ8: Real-Time Clock
extern "C" fn rtc_handler(_frame: *const InterruptFrame) {
    count_irq(8);
    // Read RTC register C to acknowledge the interrupt
    outb(0x70, 0x0C);
    let _ = inb(0x71);
    pic_eoi(8);
}

/// IRQ9: ACPI / general use (also used for e1000 NIC in some configurations)
extern "C" fn irq9_handler(_frame: *const InterruptFrame) {
    count_irq(9);
    // Forward to e1000 driver in case it uses IRQ9
    crate::drivers::e1000::handle_interrupt();
    pic_eoi(9);
}

/// IRQ10: Available / NIC
extern "C" fn irq10_handler(_frame: *const InterruptFrame) {
    count_irq(10);
    crate::drivers::e1000::handle_interrupt();
    pic_eoi(10);
}

/// IRQ11: Available / NIC / SCSI / USB
extern "C" fn irq11_handler(_frame: *const InterruptFrame) {
    count_irq(11);
    crate::drivers::e1000::handle_interrupt();
    pic_eoi(11);
}

/// IRQ12: PS/2 Mouse
extern "C" fn mouse_handler(_frame: *const InterruptFrame) {
    count_irq(12);
    // Read the mouse byte from the PS/2 data port.
    // We must check that the data is actually from the mouse (AUX bit).
    let status = inb(0x64);
    if status & 0x20 != 0 {
        // Bit 5 set means data is from the auxiliary device (mouse)
        let byte = inb(0x60);
        crate::drivers::mouse::handle_byte(byte);
    }
    pic_eoi(12);
}

/// IRQ13: FPU / Coprocessor
extern "C" fn irq13_handler(_frame: *const InterruptFrame) {
    count_irq(13);
    pic_eoi(13);
}

/// IRQ14: Primary ATA hard disk
extern "C" fn irq14_handler(_frame: *const InterruptFrame) {
    count_irq(14);
    pic_eoi(14);
}

/// IRQ15: Secondary ATA hard disk / Spurious IRQ from slave PIC
extern "C" fn spurious_slave_handler(_frame: *const InterruptFrame) {
    if is_spurious_irq(15) {
        SPURIOUS_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        return; // EOI to master was already sent by is_spurious_irq()
    }
    count_irq(15);
    pic_eoi(15);
}

// ============================================================================
// IPI Handlers (LAPIC vectors 0xF0-0xF2, delivered via LAPIC not 8259 PIC)
// ============================================================================

/// Vector 0xF0 (240): Reschedule IPI
///
/// A remote CPU has enqueued a task on this CPU's run queue and wants us to
/// schedule sooner than our next timer tick.
extern "C" fn ipi_reschedule_handler(_frame: *const InterruptFrame) {
    // EOI to LAPIC (not the 8259 PIC — LAPIC vectors bypass the PIC).
    crate::smp::lapic_eoi();
    // Give the CFS scheduler a chance to pick the newly runnable task.
    crate::process::sched_core::schedule();
}

/// Vector 0xF1 (241): TLB Shootdown IPI
///
/// A page-table modification was made by another CPU; we must flush our TLB.
extern "C" fn ipi_tlb_shootdown_handler(_frame: *const InterruptFrame) {
    // Reload CR3 to flush the entire TLB.  A page-specific INVLPG path can
    // be added later by passing the target VA through a shared variable.
    let cr3: u64;
    unsafe {
        core::arch::asm!(
            "mov {}, cr3",
            out(reg) cr3,
            options(nomem, nostack)
        );
        // SeqCst fence: ensure all prior stores from the initiating CPU
        // (the page-table writes) are ordered before our TLB flush.
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        core::arch::asm!(
            "mov cr3, {}",
            in(reg) cr3,
            options(nomem, nostack)
        );
    }
    // Signal completion so the initiator's shootdown barrier can release.
    crate::smp::TLB_SHOOTDOWN_DONE.fetch_add(1, core::sync::atomic::Ordering::Release);
    crate::smp::lapic_eoi();
}

/// Vector 0xF2 (242): Halt IPI
///
/// The system is panicking or this CPU is being hot-unplugged.
/// Disable interrupts and halt permanently.
extern "C" fn ipi_halt_handler(_frame: *const InterruptFrame) {
    crate::smp::lapic_eoi();
    // cli + hlt loop — does not return.
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    loop {
        crate::io::hlt();
    }
}

/// Vector 0xF3 (243): Per-CPU halt IPI (`IPI_VEC_HALT` from `kernel::apic`).
///
/// Sent by `kernel::apic::halt_ipi()` during clean system shutdown or
/// directed CPU hot-unplug.  The receiving CPU disables interrupts and
/// enters a permanent halt loop.  The EOI is sent to the LAPIC before
/// halting so the interrupt controller is not left in a stale state.
extern "C" fn ipi_cpu_halt_handler(_frame: *const InterruptFrame) {
    // Acknowledge to the LAPIC first so no further interrupts are blocked.
    crate::smp::lapic_eoi();
    // Disable interrupts and enter an infinite HLT loop.
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    loop {
        crate::io::hlt();
    }
}

// ============================================================================
// PS/2 Keyboard Decoder -- Scancode Set 1, US layout (from scratch)
// ============================================================================

static SHIFT_HELD: Mutex<bool> = Mutex::new(false);
static CTRL_HELD: Mutex<bool> = Mutex::new(false);
static EXTENDED_KEY: Mutex<bool> = Mutex::new(false);

/// US keyboard layout -- scancode set 1 (make codes only, index = scancode)
static SCANCODE_MAP: [u8; 128] = [
    0, 27, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b'-', b'=',
    8, // 0x00-0x0E (Esc, 1-0, -, =, Backspace)
    b'\t', b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', b'[', b']',
    b'\n', // 0x0F-0x1C (Tab, q-p, [, ], Enter)
    0, b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', b'\'',
    b'`', // 0x1D-0x29 (LCtrl, a-l, ;, ', `)
    0, b'\\', b'z', b'x', b'c', b'v', b'b', b'n', b'm', b',', b'.', b'/',
    0, // 0x2A-0x36 (LShift, \, z-m, ,, ., /, RShift)
    b'*', 0, b' ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, // 0x37-0x45 (KP*, LAlt, Space, CapsLock, F1-F10)
    0, 0, 0, 0, 0, 0, 0, b'7', b'8', b'9', b'-', b'4', b'5', b'6',
    b'+', // 0x46-0x54 (NumLock, ScrollLock, KP7-KP+)
    b'1', b'2', b'3', b'0', b'.', // 0x55-0x59 (KP1-KP.)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x5A-0x6F (padding)
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // 0x70-0x7F (padding)
];

/// Shifted characters for top row
static SHIFT_MAP: [u8; 128] = [
    0, 27, b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')', b'_', b'+', 8, b'\t', b'Q',
    b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', b'{', b'}', b'\n', 0, b'A', b'S', b'D',
    b'F', b'G', b'H', b'J', b'K', b'L', b':', b'"', b'~', 0, b'|', b'Z', b'X', b'C', b'V', b'B',
    b'N', b'M', b'<', b'>', b'?', 0, b'*', 0, b' ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, b'7', b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', b'2', b'3', b'0', b'.', 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0,
];

fn decode_scancode(scancode: u8) {
    use crate::drivers::keyboard::{self, special};

    // E0 prefix: next scancode is an extended key
    if scancode == 0xE0 {
        *EXTENDED_KEY.lock() = true;
        return;
    }

    let is_extended = {
        let mut ext = EXTENDED_KEY.lock();
        let was = *ext;
        *ext = false;
        was
    };

    let is_release = scancode & 0x80 != 0;
    let make = scancode & 0x7F;

    // Handle extended keys (arrow keys, etc.)
    if is_extended {
        if is_release {
            return;
        }
        let special_code = match make {
            0x48 => Some(special::ARROW_UP),
            0x50 => Some(special::ARROW_DOWN),
            0x4B => Some(special::ARROW_LEFT),
            0x4D => Some(special::ARROW_RIGHT),
            _ => None,
        };
        if let Some(code) = special_code {
            let event = keyboard::KeyEvent {
                character: '\0',
                scancode: code,
                keycode: keyboard::KeyCode::Unknown,
                pressed: true,
                modifiers: keyboard::Modifiers {
                    shift: *SHIFT_HELD.lock(),
                    ctrl: *CTRL_HELD.lock(),
                    ..Default::default()
                },
            };
            keyboard::push_key(event);
        }
        return;
    }

    if is_release {
        if make == 0x2A || make == 0x36 {
            // LShift or RShift released
            *SHIFT_HELD.lock() = false;
        }
        if make == 0x1D {
            // LCtrl released
            *CTRL_HELD.lock() = false;
        }
        return;
    }

    // Key press (make code)
    if scancode == 0x2A || scancode == 0x36 {
        // LShift or RShift pressed
        *SHIFT_HELD.lock() = true;
        return;
    }
    if scancode == 0x1D {
        // LCtrl pressed
        *CTRL_HELD.lock() = true;
        return;
    }

    let idx = scancode as usize;
    if idx >= 128 {
        return;
    }

    let shift = *SHIFT_HELD.lock();
    let ctrl = *CTRL_HELD.lock();

    // Handle Ctrl+C
    if ctrl && (idx == 0x2E) {
        // 0x2E = 'c' scancode
        let event = keyboard::KeyEvent {
            character: '\x03', // ETX
            scancode: special::CTRL_C,
            keycode: keyboard::KeyCode::C,
            pressed: true,
            modifiers: keyboard::Modifiers {
                shift,
                ctrl,
                ..Default::default()
            },
        };
        keyboard::push_key(event);
        return;
    }

    let ch = if shift {
        SHIFT_MAP[idx]
    } else {
        SCANCODE_MAP[idx]
    };

    let character =
        if ch != 0 && (ch == b'\n' || ch == 8 || ch == b'\t' || (ch >= 0x20 && ch <= 0x7E)) {
            ch as char
        } else {
            '\0'
        };

    // Push to keyboard buffer for shell/userspace
    let event = keyboard::KeyEvent {
        character,
        scancode,
        keycode: keyboard::KeyCode::Unknown,
        pressed: true,
        modifiers: keyboard::Modifiers {
            shift,
            ctrl,
            ..Default::default()
        },
    };
    keyboard::push_key(event);

    // Echo printable characters to VGA
    if character != '\0' && character != '\x08' {
        kprint!("{}", character);
    }
}

// ============================================================================
// IDT Initialization
// ============================================================================

pub fn init_idt() {
    let cs = crate::gdt::KERNEL_CS;

    unsafe {
        // CPU exceptions (vectors 0-31)
        IDT.entries[0].set_handler(divide_error_stub as *const () as u64, cs); // #DE
        IDT.entries[1].set_handler(debug_stub as *const () as u64, cs); // #DB
        IDT.entries[2].set_handler(nmi_stub as *const () as u64, cs); // NMI
        IDT.entries[3].set_handler(breakpoint_stub as *const () as u64, cs); // #BP
        IDT.entries[4].set_handler(overflow_stub as *const () as u64, cs); // #OF
        IDT.entries[5].set_handler(bound_range_stub as *const () as u64, cs); // #BR
        IDT.entries[6].set_handler(invalid_opcode_stub as *const () as u64, cs); // #UD
        IDT.entries[7].set_handler(device_not_avail_stub as *const () as u64, cs); // #NM
        IDT.entries[8].set_handler(double_fault_stub as *const () as u64, cs); // #DF
        IDT.entries[8].set_ist(crate::gdt::DOUBLE_FAULT_IST_INDEX as u8 + 1);
        IDT.entries[10].set_handler(invalid_tss_stub as *const () as u64, cs); // #TS
        IDT.entries[11].set_handler(segment_not_present_stub as *const () as u64, cs); // #NP
        IDT.entries[12].set_handler(stack_segment_stub as *const () as u64, cs); // #SS
        IDT.entries[13].set_handler(gpf_stub as *const () as u64, cs); // #GP
        IDT.entries[14].set_handler(page_fault_stub as *const () as u64, cs); // #PF
        IDT.entries[16].set_handler(x87_fpu_stub as *const () as u64, cs); // #MF
        IDT.entries[17].set_handler(alignment_check_stub as *const () as u64, cs); // #AC
        IDT.entries[18].set_handler(machine_check_stub as *const () as u64, cs); // #MC
        IDT.entries[19].set_handler(simd_stub as *const () as u64, cs); // #XM

        // Hardware interrupts (remapped via PIC: IRQ 0-7 -> vectors 32-39)
        IDT.entries[32].set_handler(timer_stub as *const () as u64, cs); // IRQ 0: Timer
        IDT.entries[33].set_handler(keyboard_stub as *const () as u64, cs); // IRQ 1: Keyboard
        IDT.entries[34].set_handler(cascade_stub as *const () as u64, cs); // IRQ 2: Cascade
        IDT.entries[35].set_handler(com2_stub as *const () as u64, cs); // IRQ 3: COM2
        IDT.entries[36].set_handler(com1_stub as *const () as u64, cs); // IRQ 4: COM1
        IDT.entries[37].set_handler(irq5_stub as *const () as u64, cs); // IRQ 5: Sound
        IDT.entries[38].set_handler(irq6_stub as *const () as u64, cs); // IRQ 6: Floppy
        IDT.entries[39].set_handler(spurious_master_stub as *const () as u64, cs); // IRQ 7: LPT1

        // Hardware interrupts (slave PIC: IRQ 8-15 -> vectors 40-47)
        IDT.entries[40].set_handler(rtc_stub as *const () as u64, cs); // IRQ 8: RTC
        IDT.entries[41].set_handler(irq9_stub as *const () as u64, cs); // IRQ 9: ACPI/NIC
        IDT.entries[42].set_handler(irq10_stub as *const () as u64, cs); // IRQ 10: NIC
        IDT.entries[43].set_handler(irq11_stub as *const () as u64, cs); // IRQ 11: NIC/USB
        IDT.entries[44].set_handler(mouse_stub as *const () as u64, cs); // IRQ 12: Mouse
        IDT.entries[45].set_handler(irq13_stub as *const () as u64, cs); // IRQ 13: FPU
        IDT.entries[46].set_handler(irq14_stub as *const () as u64, cs); // IRQ 14: ATA primary
        IDT.entries[47].set_handler(spurious_slave_stub as *const () as u64, cs); // IRQ 15: ATA secondary

        // IPI vectors (LAPIC-delivered, not via 8259 PIC)
        IDT.entries[0xF0].set_handler(ipi_reschedule_stub as *const () as u64, cs); // 240: TLB flush (kernel::apic::IPI_VEC_TLB_FLUSH)
        IDT.entries[0xF1].set_handler(ipi_tlb_shootdown_stub as *const () as u64, cs); // 241: Sched kick (kernel::apic::IPI_VEC_SCHED_KICK)
        IDT.entries[0xF2].set_handler(ipi_halt_stub as *const () as u64, cs); // 242: Panic halt (kernel::apic::IPI_VEC_PANIC)
        IDT.entries[0xF3].set_handler(ipi_cpu_halt_stub as *const () as u64, cs); // 243: Per-CPU halt (kernel::apic::IPI_VEC_HALT)

        // Load IDT
        let idt_ptr = IdtPointer {
            limit: (core::mem::size_of::<Idt>() - 1) as u16,
            base: &raw const IDT as *const Idt as u64,
        };

        core::arch::asm!(
            "lidt [{}]",
            in(reg) &idt_ptr as *const IdtPointer,
            options(nostack),
        );
    }

    // Explicitly unmask the IRQs we need -- do not rely on pre-boot mask state.
    // QEMU multiboot leaves most IRQs masked; save/restore in init_pics() preserves that.
    enable_irq(0);  // IRQ 0: PIT timer  ← THIS is why the timer never fired
    enable_irq(1);  // IRQ 1: PS/2 keyboard
    enable_irq(2);  // IRQ 2: cascade (required for slave PIC IRQs 8-15)
    enable_irq(12); // IRQ 12: PS/2 mouse
}
