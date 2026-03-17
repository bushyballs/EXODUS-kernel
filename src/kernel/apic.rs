/// kernel/apic.rs — Local APIC management for Genesis kernel
///
/// Provides a self-contained, no-heap LAPIC driver:
///   - MMIO register read/write via volatile pointers
///   - APIC enable, EOI, ID, TPR
///   - IPI delivery: fixed, INIT, STARTUP, broadcast
///   - APIC timer: one-shot and periodic modes
///   - High-level IPI helpers: TLB flush, sched kick, panic/halt broadcast
///
/// Follows kernel safety rules:
///   - No float casts (no `as f64` / `as f32`)
///   - No heap (no Vec, Box, String, alloc::*)
///   - No panics (no unwrap, expect, panic!)
///   - Saturating arithmetic for counters
///   - read_volatile / write_volatile for all MMIO
use core::sync::atomic::{AtomicBool, Ordering};

// ── APIC MMIO base ──────────────────────────────────────────────────────────

/// Default Local APIC MMIO physical base address (xAPIC mode).
pub const APIC_BASE: u64 = 0xFEE0_0000;

// ── APIC register offsets (byte offset from APIC_BASE) ──────────────────────

const APIC_ID: usize = 0x020;
const APIC_VERSION: usize = 0x030;
const APIC_TPR: usize = 0x080; // Task Priority Register
const APIC_EOI: usize = 0x0B0; // End of Interrupt
const APIC_SPURIOUS: usize = 0x0F0; // Spurious Interrupt Vector Register
const APIC_ICR_LO: usize = 0x300; // Interrupt Command Register — low 32 bits
const APIC_ICR_HI: usize = 0x310; // Interrupt Command Register — high 32 bits
const APIC_LVT_TIMER: usize = 0x320; // LVT Timer Register
const APIC_TIMER_ICR: usize = 0x380; // Timer Initial Count Register
const APIC_TIMER_CCR: usize = 0x390; // Timer Current Count Register
const APIC_TIMER_DCR: usize = 0x3E0; // Timer Divide Configuration Register

// ── ICR delivery-mode constants ─────────────────────────────────────────────

pub const IPI_FIXED: u32 = 0 << 8;
pub const IPI_LOWEST: u32 = 1 << 8;
pub const IPI_SMI: u32 = 2 << 8;
pub const IPI_NMI: u32 = 4 << 8;
pub const IPI_INIT: u32 = 5 << 8;
pub const IPI_STARTUP: u32 = 6 << 8;

pub const IPI_LEVEL_ASSERT: u32 = 1 << 14;
pub const IPI_LEVEL_DEASSERT: u32 = 0 << 14;
pub const IPI_DEST_PHYSICAL: u32 = 0 << 11;

// Destination shorthand (bits 19:18 of ICR_LO)
pub const IPI_DEST_SELF: u32 = 1 << 18;
pub const IPI_DEST_ALL: u32 = 2 << 18; // all including self
pub const IPI_DEST_ALL_EX_SELF: u32 = 3 << 18; // all excluding self

// Delivery status bit (read ICR_LO bit 12; 1 = send pending)
const ICR_DELIVERY_STATUS: u32 = 1 << 12;

// ── IPI vector assignments ───────────────────────────────────────────────────

/// Vector 0xF0: TLB shootdown — remote CPU must flush its TLB.
pub const IPI_VEC_TLB_FLUSH: u8 = 0xF0;
/// Vector 0xF1: Scheduler kick — remote CPU should run schedule().
pub const IPI_VEC_SCHED_KICK: u8 = 0xF1;
/// Vector 0xF2: Panic broadcast — kernel panic in progress, halt all APs.
pub const IPI_VEC_PANIC: u8 = 0xF2;
/// Vector 0xF3: Halt IPI — CPU hot-unplug or clean shutdown of one CPU.
pub const IPI_VEC_HALT: u8 = 0xF3;

// ── Module state ─────────────────────────────────────────────────────────────

/// Set to true after the BSP APIC has been initialized.
static APIC_READY: AtomicBool = AtomicBool::new(false);

// ── MMIO helpers ─────────────────────────────────────────────────────────────

/// Read a 32-bit LAPIC register at `offset` bytes from APIC_BASE.
#[inline]
fn lapic_read(offset: usize) -> u32 {
    unsafe { ((APIC_BASE as usize).saturating_add(offset) as *const u32).read_volatile() }
}

/// Write a 32-bit value to LAPIC register at `offset` bytes from APIC_BASE.
#[inline]
fn lapic_write(offset: usize, val: u32) {
    unsafe {
        ((APIC_BASE as usize).saturating_add(offset) as *mut u32).write_volatile(val);
    }
}

// ── Core LAPIC operations ────────────────────────────────────────────────────

/// Return the APIC ID of the calling CPU (bits 31:24 of the APIC ID register).
///
/// Returns 0 if the APIC has not yet been enabled.
#[inline]
pub fn lapic_id() -> u8 {
    if !APIC_READY.load(Ordering::Acquire) {
        return 0;
    }
    (lapic_read(APIC_ID) >> 24) as u8
}

/// Return the LAPIC version register value.
pub fn lapic_version() -> u32 {
    lapic_read(APIC_VERSION)
}

/// Send End-of-Interrupt to the LAPIC.
///
/// Must be called at the end of every interrupt handler that was delivered
/// through the LAPIC (APIC-mode vectors, IPIs).  The 8259 PIC uses a
/// separate EOI path.
#[inline]
pub fn lapic_eoi() {
    if APIC_READY.load(Ordering::Relaxed) {
        lapic_write(APIC_EOI, 0);
    }
}

/// Enable the Local APIC and set the spurious vector.
///
/// `spurious_vec` — vector delivered when a spurious interrupt is detected
/// (conventionally 0xFF).  Sets TPR to 0 so all interrupts are accepted.
pub fn lapic_enable(spurious_vec: u8) {
    // Spurious Vector Register: bit 8 = APIC Software Enable, bits 7:0 = vector.
    lapic_write(APIC_SPURIOUS, 0x100 | (spurious_vec as u32));
    // Accept all interrupt priorities.
    lapic_write(APIC_TPR, 0);
    APIC_READY.store(true, Ordering::Release);
}

// ── IPI send primitives ───────────────────────────────────────────────────────

/// Poll the DELIVERY_STATUS bit of ICR_LO until clear (or timeout).
///
/// The LAPIC cannot accept another IPI while the previous one is still
/// in the "send pending" state.  We spin up to 100 000 iterations.
#[inline]
fn wait_for_delivery() {
    let mut tries = 0u32;
    while lapic_read(APIC_ICR_LO) & ICR_DELIVERY_STATUS != 0 {
        core::hint::spin_loop();
        tries = tries.saturating_add(1);
        if tries >= 100_000 {
            break; // timeout — proceed to avoid livelock
        }
    }
}

/// Send an IPI to a specific destination APIC ID.
///
/// `dest_apic_id` — 8-bit xAPIC destination (field sits in ICR_HI bits 31:24).
/// `mode`  — delivery mode constant (IPI_FIXED, IPI_INIT, IPI_STARTUP, …).
/// `vector` — interrupt vector (only meaningful for FIXED and STARTUP modes).
pub fn lapic_send_ipi(dest_apic_id: u8, mode: u32, vector: u8) {
    if !APIC_READY.load(Ordering::Acquire) {
        return;
    }
    wait_for_delivery();
    // Write destination high word first.
    lapic_write(APIC_ICR_HI, (dest_apic_id as u32) << 24);
    // Full SeqCst fence: all prior memory writes must be visible on the
    // remote CPU before the IPI fires.
    core::sync::atomic::fence(Ordering::SeqCst);
    // Writing ICR_LO triggers delivery.
    lapic_write(APIC_ICR_LO, mode | (vector as u32));
    // Wait for delivery to complete before returning.
    wait_for_delivery();
}

/// Broadcast an IPI to all CPUs *excluding* the caller.
///
/// Uses the destination shorthand `ALL_EXCLUDING_SELF`, so no per-CPU
/// APIC ID lookup is required.
pub fn lapic_send_ipi_all_except_self(vector: u8) {
    if !APIC_READY.load(Ordering::Acquire) {
        return;
    }
    wait_for_delivery();
    lapic_write(APIC_ICR_HI, 0);
    core::sync::atomic::fence(Ordering::SeqCst);
    lapic_write(
        APIC_ICR_LO,
        IPI_DEST_ALL_EX_SELF | IPI_FIXED | (vector as u32),
    );
    wait_for_delivery();
}

/// Send an IPI to the *calling* CPU itself.
///
/// Useful for self-testing the interrupt path or deferred self-delivery.
pub fn lapic_send_self_ipi(vector: u8) {
    if !APIC_READY.load(Ordering::Acquire) {
        return;
    }
    wait_for_delivery();
    lapic_write(APIC_ICR_HI, 0);
    core::sync::atomic::fence(Ordering::SeqCst);
    lapic_write(APIC_ICR_LO, IPI_DEST_SELF | IPI_FIXED | (vector as u32));
    wait_for_delivery();
}

// ── APIC timer ────────────────────────────────────────────────────────────────

/// Configure the LAPIC timer in **one-shot** mode.
///
/// `vector` — interrupt vector delivered when the counter reaches zero.
/// `count`  — initial count value (ticks until the interrupt fires).
/// Divide-by-16 is applied to the bus clock (DCR = 0x3).
pub fn lapic_timer_oneshot(vector: u8, count: u32) {
    // Divide configuration: divide by 16 (DCR = 0x3).
    lapic_write(APIC_TIMER_DCR, 0x3);
    // LVT timer: one-shot mode (bit 17 = 0), vector in bits 7:0.
    lapic_write(APIC_LVT_TIMER, vector as u32);
    // Writing the ICR starts the countdown.
    lapic_write(APIC_TIMER_ICR, count);
}

/// Configure the LAPIC timer in **periodic** mode.
///
/// The timer fires repeatedly every `count` ticks.
/// Divide-by-16 is applied (DCR = 0x3).
pub fn lapic_timer_periodic(vector: u8, count: u32) {
    lapic_write(APIC_TIMER_DCR, 0x3);
    // LVT timer: periodic mode (bit 17 = 1).
    lapic_write(APIC_LVT_TIMER, (1 << 17) | (vector as u32));
    lapic_write(APIC_TIMER_ICR, count);
}

/// Stop the LAPIC timer by writing 0 to the Initial Count Register.
pub fn lapic_timer_stop() {
    lapic_write(APIC_TIMER_ICR, 0);
}

/// Read the LAPIC timer current count.
pub fn lapic_timer_current() -> u32 {
    lapic_read(APIC_TIMER_CCR)
}

// ── High-level IPI helpers ────────────────────────────────────────────────────

/// Broadcast a TLB-flush IPI to all CPUs except the caller.
///
/// The receiving CPUs must flush their TLBs and call `lapic_eoi()`.
pub fn tlb_flush_ipi() {
    lapic_send_ipi_all_except_self(IPI_VEC_TLB_FLUSH);
}

/// Send a scheduler-kick IPI to a specific CPU identified by its APIC ID.
///
/// The target CPU should call `schedule()` as soon as the handler returns.
pub fn sched_kick_ipi(dest_apic_id: u8) {
    lapic_send_ipi(dest_apic_id, IPI_FIXED, IPI_VEC_SCHED_KICK);
}

/// Broadcast a panic IPI to all CPUs except the caller.
///
/// Used when the kernel panics: all other CPUs should halt so only the
/// panicking CPU continues to print the diagnostic.
pub fn panic_ipi() {
    lapic_send_ipi_all_except_self(IPI_VEC_PANIC);
}

/// Broadcast a halt IPI to all CPUs except the caller.
///
/// Used for clean system shutdown or hot-unplug of all non-BSP CPUs.
pub fn halt_ipi() {
    lapic_send_ipi_all_except_self(IPI_VEC_HALT);
}

// ── Initialization ────────────────────────────────────────────────────────────

/// Initialize the Local APIC on the *calling* CPU.
///
/// Enables the APIC with spurious vector 0xFF and clears the TPR so that
/// all interrupt priorities are accepted.  Safe to call on both BSP and APs.
pub fn init() {
    lapic_enable(0xFF);
    crate::serial_println!(
        "  [kernel::apic] LAPIC enabled on CPU {} (APIC ID {})",
        crate::smp::current_cpu(),
        lapic_id(),
    );
}

/// Initialize the Local APIC on an Application Processor.
///
/// Equivalent to `init()` but emits an AP-specific log message.
pub fn init_ap() {
    lapic_enable(0xFF);
    crate::serial_println!(
        "  [kernel::apic] LAPIC ready on AP {} (APIC ID {})",
        crate::smp::current_cpu(),
        lapic_id(),
    );
}
