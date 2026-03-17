/*
 * Genesis OS — Local APIC (Advanced Programmable Interrupt Controller)
 *
 * Controls per-CPU interrupts, timers, and inter-processor interrupts (IPIs).
 *
 * IPI delivery
 * ────────────
 * All IPI sends follow the same three-step protocol:
 *   1. Poll DELIVERY_STATUS until clear (previous IPI delivered).
 *   2. Write ICR_HIGH (destination APIC ID in bits 31:24).
 *   3. Full SeqCst fence — ensures all prior stores reach coherent cache.
 *   4. Write ICR_LOW (triggers delivery).
 *
 * No std, no float, no panics.  All arithmetic is saturating.
 */

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::Ordering;

// ── APIC register offsets ───────────────────────────────────────────────────

const APIC_ID: u32 = 0x020;
const APIC_VERSION: u32 = 0x030;
const APIC_TPR: u32 = 0x080;
const APIC_EOI: u32 = 0x0B0;
const APIC_SPURIOUS: u32 = 0x0F0;
const APIC_ICR_LOW: u32 = 0x300;
const APIC_ICR_HIGH: u32 = 0x310;
const APIC_TIMER_LVT: u32 = 0x320;
const APIC_TIMER_INITIAL: u32 = 0x380;
const APIC_TIMER_CURRENT: u32 = 0x390;
const APIC_TIMER_DIVIDE: u32 = 0x3E0;

// ICR_LOW bit masks
const ICR_DELIVERY_STATUS: u32 = 1 << 12; // 1 = send pending
const ICR_LEVEL_ASSERT: u32 = 1 << 14;
const ICR_TRIGGER_LEVEL: u32 = 1 << 15;

// Delivery modes (bits 10:8)
const DM_FIXED: u32 = 0 << 8;
const DM_NMI: u32 = 4 << 8;
const DM_INIT: u32 = 5 << 8;
const DM_SIPI: u32 = 6 << 8;

// Destination shorthands (bits 19:18)
const DEST_NO_SHORTHAND: u32 = 0 << 18;
const DEST_ALL_INCLUDING_SELF: u32 = 2 << 18;
const DEST_ALL_EXCLUDING_SELF: u32 = 3 << 18;

// ── IPI vector numbers ──────────────────────────────────────────────────────

/// Reschedule IPI: ask a remote CPU to call schedule().
pub const IPI_RESCHEDULE: u8 = 0xF0; // 240
/// TLB shootdown IPI: remote CPU must flush its TLB.
pub const IPI_TLB_FLUSH: u8 = 0xF1; // 241
/// Halt IPI: permanently stop a CPU (panic, hotplug).
pub const IPI_HALT: u8 = 0xF2; // 242

// ── Module state ────────────────────────────────────────────────────────────

static mut APIC_BASE: u64 = 0;

// ── MMIO read / write ───────────────────────────────────────────────────────

/// Read a 32-bit value from a Local APIC MMIO register.
#[inline]
unsafe fn read_apic(offset: u32) -> u32 {
    if APIC_BASE == 0 {
        return 0;
    }
    read_volatile((APIC_BASE + offset as u64) as *const u32)
}

/// Write a 32-bit value to a Local APIC MMIO register.
#[inline]
unsafe fn write_apic(offset: u32, value: u32) {
    if APIC_BASE == 0 {
        return;
    }
    write_volatile((APIC_BASE + offset as u64) as *mut u32, value);
}

// ── Initialisation ──────────────────────────────────────────────────────────

/// Initialize the Local APIC on the Bootstrap Processor (BSP).
///
/// Must be called once after ACPI tables are parsed so that `APIC_BASE` is set.
pub unsafe fn init_bsp() {
    APIC_BASE = crate::acpi::local_apic_address() as u64;

    // Enable APIC; set spurious vector to 0xFF.
    write_apic(APIC_SPURIOUS, 0x1FF);

    // Accept all interrupt priorities.
    write_apic(APIC_TPR, 0);

    // Configure periodic timer (divide by 16, vector 32).
    write_apic(APIC_TIMER_DIVIDE, 0x03);
    write_apic(APIC_TIMER_LVT, 0x00020020); // periodic, vector 0x20
    write_apic(APIC_TIMER_INITIAL, 1_000_000);

    send_eoi();
}

/// Initialize the Local APIC on an Application Processor (AP).
pub unsafe fn init_ap() {
    // APs share the same physical APIC_BASE as the BSP.
    write_apic(APIC_SPURIOUS, 0x1FF);
    write_apic(APIC_TPR, 0);
    write_apic(APIC_TIMER_DIVIDE, 0x03);
    write_apic(APIC_TIMER_LVT, 0x00020020);
    write_apic(APIC_TIMER_INITIAL, 1_000_000);
    send_eoi();
}

// ── EOI / ID ────────────────────────────────────────────────────────────────

/// Send End-Of-Interrupt to acknowledge the current interrupt.
#[inline]
pub unsafe fn send_eoi() {
    write_apic(APIC_EOI, 0);
}

/// Read the APIC ID of the calling CPU.
pub unsafe fn read_apic_id() -> u32 {
    (read_apic(APIC_ID) >> 24) & 0xFF
}

// ── Core IPI send ───────────────────────────────────────────────────────────

/// Wait for ICR delivery to complete (delivery-status bit clear).
#[inline]
unsafe fn wait_for_delivery() {
    for _ in 0..100_000u32 {
        if read_apic(APIC_ICR_LOW) & ICR_DELIVERY_STATUS == 0 {
            return;
        }
        core::hint::spin_loop();
    }
    // Timed out; proceed anyway to avoid livelock.
}

/// Send an IPI to a specific destination APIC ID.
///
/// Steps (per Intel SDM §10.6.1):
///   1. Wait for pending delivery to complete.
///   2. Write ICR_HIGH (destination, bits 31:24).
///   3. SeqCst fence.
///   4. Write ICR_LOW (triggers the IPI).
pub unsafe fn send_ipi(dest_apic_id: u32, vector: u8) {
    wait_for_delivery();
    write_apic(APIC_ICR_HIGH, (dest_apic_id & 0xFF) << 24);
    core::sync::atomic::fence(Ordering::SeqCst);
    write_apic(APIC_ICR_LOW, DM_FIXED | (vector as u32));
}

/// Send an INIT IPI to `dest_apic_id` (level-assert, INIT delivery mode).
pub unsafe fn send_init_ipi(dest_apic_id: u32) {
    wait_for_delivery();
    write_apic(APIC_ICR_HIGH, (dest_apic_id & 0xFF) << 24);
    core::sync::atomic::fence(Ordering::SeqCst);
    write_apic(APIC_ICR_LOW, DM_INIT | ICR_LEVEL_ASSERT);
}

/// Send a STARTUP IPI (SIPI) to `dest_apic_id`.
///
/// `start_page` is the 8-bit page number (real-mode start address = page × 0x1000).
pub unsafe fn send_startup_ipi(dest_apic_id: u32, start_page: u8) {
    wait_for_delivery();
    write_apic(APIC_ICR_HIGH, (dest_apic_id & 0xFF) << 24);
    core::sync::atomic::fence(Ordering::SeqCst);
    write_apic(APIC_ICR_LOW, DM_SIPI | (start_page as u32));
}

/// Convenience wrapper: send_startup_ipi.
pub unsafe fn send_sipi(dest_apic_id: u32, vector: u8) {
    send_startup_ipi(dest_apic_id, vector);
}

// ── Broadcast IPIs ──────────────────────────────────────────────────────────

/// Send a fixed-vector IPI to all CPUs *except* the caller.
pub unsafe fn send_ipi_all(vector: u8) {
    wait_for_delivery();
    write_apic(APIC_ICR_HIGH, 0);
    core::sync::atomic::fence(Ordering::SeqCst);
    write_apic(
        APIC_ICR_LOW,
        DM_FIXED | DEST_ALL_EXCLUDING_SELF | (vector as u32),
    );
}

/// Send a fixed-vector IPI to all CPUs *including* the caller.
pub unsafe fn send_ipi_all_including_self(vector: u8) {
    wait_for_delivery();
    write_apic(APIC_ICR_HIGH, 0);
    core::sync::atomic::fence(Ordering::SeqCst);
    write_apic(
        APIC_ICR_LOW,
        DM_FIXED | DEST_ALL_INCLUDING_SELF | (vector as u32),
    );
}

// ── Typed high-level helpers ────────────────────────────────────────────────

/// Send a reschedule IPI to a specific logical CPU index.
pub unsafe fn send_reschedule_ipi(cpu_id: usize) {
    if let Some(apic_id) = crate::acpi::cpu_apic_id(cpu_id) {
        send_ipi(apic_id as u32, IPI_RESCHEDULE);
    }
}

/// Send a TLB-flush IPI to all CPUs (excluding self).
pub unsafe fn send_tlb_flush_ipi() {
    send_ipi_all(IPI_TLB_FLUSH);
}

/// Send a TLB-shootdown IPI to all CPUs in `cpu_mask`.
///
/// `cpu_mask` is a bitmask of *logical CPU indices*.
/// This version coordinates with `smp::TLB_SHOOTDOWN_DONE`.
pub unsafe fn send_tlb_shootdown_ipi(cpu_mask: u64) {
    crate::smp::send_tlb_shootdown_ipi(cpu_mask);
}

/// Send a halt IPI to all other CPUs (used on panic / shutdown).
pub unsafe fn send_halt_all() {
    send_ipi_all(IPI_HALT);
}

// ── Timer calibration ───────────────────────────────────────────────────────

/// Calibrate the APIC timer and return ticks per millisecond.
pub unsafe fn calibrate_timer() -> u32 {
    // Use a fixed conservative estimate; replace with PIT calibration
    // when a high-resolution delay is available.
    1_000_000 / 100 // ~100 Hz
}
