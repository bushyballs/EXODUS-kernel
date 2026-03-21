use crate::sync::Mutex;
use crate::serial_println;

// MSR addresses
const MSR_IA32_PERFEVTSEL0: u32 = 0x186;
const MSR_IA32_PMC0: u32 = 0xC1;
const MSR_IA32_MISC_ENABLE: u32 = 0x1A0;

// APIC LVT Error Register MMIO address
const APIC_LVT_ERROR: usize = 0xFEE00370;

// DTLB_LOAD_MISSES: EventSelect=0x08, UMask=0x01, USR|OS|EN bits
// USR=bit22, OS=bit17, EN=bit16
const PERFEVTSEL_DTLB_LOAD_MISSES: u64 = 0x0108_0008 | (1 << 22) | (1 << 17) | (1 << 16);

// IA32_MISC_ENABLE bit 18 = Enhanced Intel SpeedStep enable
const MISC_ENABLE_SPEEDSTEP_BIT: u64 = 1 << 18;

// How often we sample (every N ticks)
const SAMPLE_INTERVAL: u32 = 16;
const LOG_INTERVAL: u32 = 500;

#[derive(Clone, Copy)]
pub struct MemoryAnticipationState {
    pub tlb_miss_rate: u16,
    pub fault_churn: u16,
    pub interrupt_backlog: u16,
    pub anticipation: u16,
    pub running_fast: bool,

    // Internal tracking across ticks
    prev_cr2: u64,
    prev_pmc0: u64,
    churn_accum: u16,
    miss_accum: u64,
    initialized: bool,
}

impl MemoryAnticipationState {
    const fn new() -> Self {
        Self {
            tlb_miss_rate: 0,
            fault_churn: 0,
            interrupt_backlog: 0,
            anticipation: 0,
            running_fast: false,
            prev_cr2: 0,
            prev_pmc0: 0,
            churn_accum: 0,
            miss_accum: 0,
            initialized: false,
        }
    }
}

static STATE: Mutex<MemoryAnticipationState> = Mutex::new(MemoryAnticipationState::new());

// Read an MSR via rdmsr; returns (edx:eax) as u64
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack, preserves_flags),
    );
    ((hi as u64) << 32) | (lo as u64)
}

// Write an MSR via wrmsr
unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack, preserves_flags),
    );
}

// Read performance counter 0 via rdpmc
unsafe fn rdpmc0() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdpmc",
        in("ecx") 0u32,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack, preserves_flags),
    );
    // PMC0 is 40-bit; mask to 40 bits
    (((hi as u64) << 32) | (lo as u64)) & 0x00FF_FFFF_FFFF
}

// Read CR2 (last page fault linear address)
unsafe fn read_cr2() -> u64 {
    let val: u64;
    core::arch::asm!(
        "mov {val}, cr2",
        val = out(reg) val,
        options(nomem, nostack, preserves_flags),
    );
    val
}

// Read APIC LVT Error Register via MMIO
unsafe fn read_apic_lvt_error() -> u32 {
    core::ptr::read_volatile(APIC_LVT_ERROR as *const u32)
}

pub fn init() {
    unsafe {
        // Program IA32_PERFEVTSEL0 for DTLB_LOAD_MISSES
        wrmsr(MSR_IA32_PERFEVTSEL0, PERFEVTSEL_DTLB_LOAD_MISSES);
        // Zero out PMC0 to start from a clean count
        wrmsr(MSR_IA32_PMC0, 0);
    }

    let mut state = STATE.lock();
    unsafe {
        state.prev_cr2 = read_cr2();
        state.prev_pmc0 = rdpmc0();
    }
    state.initialized = true;

    serial_println!("[memory_anticipation] init: DTLB_LOAD_MISSES PMU armed, CR2 baseline {:x}", state.prev_cr2);
}

pub fn tick(age: u32) {
    // Only sample every SAMPLE_INTERVAL ticks
    if age % SAMPLE_INTERVAL != 0 {
        // Still track CR2 churn between intervals
        let cr2_now = unsafe { read_cr2() };
        let mut state = STATE.lock();
        if state.initialized && cr2_now != state.prev_cr2 {
            state.churn_accum = state.churn_accum.saturating_add(1);
            state.prev_cr2 = cr2_now;
        }
        return;
    }

    let cr2_now = unsafe { read_cr2() };
    let pmc0_now = unsafe { rdpmc0() };
    let apic_lvt = unsafe { read_apic_lvt_error() };
    let misc_enable = unsafe { rdmsr(MSR_IA32_MISC_ENABLE) };

    let mut state = STATE.lock();

    if !state.initialized {
        state.prev_cr2 = cr2_now;
        state.prev_pmc0 = pmc0_now;
        state.initialized = true;
        return;
    }

    // --- TLB miss rate ---
    // Delta of PMC0 over the last SAMPLE_INTERVAL ticks, capped to u16 range then scaled to 0-1000
    let pmc_delta = pmc0_now.saturating_sub(state.prev_pmc0);
    state.prev_pmc0 = pmc0_now;
    // Scale: treat 500+ misses per interval as saturating max (1000)
    let tlb_miss_rate = ((pmc_delta.min(500) * 2) as u16).min(1000);

    // --- Fault churn ---
    // Accumulated CR2 changes since last sample, scaled to 0-1000
    // Treat 16+ changes per interval as saturating max
    let churn = state.churn_accum;
    // Also catch a final change this tick
    let churn = if cr2_now != state.prev_cr2 {
        state.prev_cr2 = cr2_now;
        churn.saturating_add(1)
    } else {
        churn
    };
    state.churn_accum = 0;
    // Scale: 16 changes -> 1000
    let fault_churn = ((churn as u32).saturating_mul(62).min(1000)) as u16;

    // --- Interrupt backlog ---
    // APIC LVT Error Register bit 12 = delivery status (1 = send pending)
    // We poll a few times to detect sustained backlog
    let delivery_pending = (apic_lvt >> 12) & 1;
    let interrupt_backlog: u16 = match delivery_pending {
        0 => 0,
        _ => {
            // Re-read to distinguish momentary vs. sustained
            let second_read = unsafe { read_apic_lvt_error() };
            if (second_read >> 12) & 1 == 1 {
                800 // Sustained — high backlog
            } else {
                400 // Momentary delivery status
            }
        }
    };

    // --- Running fast ---
    // IA32_MISC_ENABLE bit 18: SpeedStep enabled means dynamic frequency scaling is on
    // Treat SpeedStep DISABLED as running at full fixed speed (running_fast = true)
    let speedstep_enabled = (misc_enable & MISC_ENABLE_SPEEDSTEP_BIT) != 0;
    let running_fast = !speedstep_enabled;

    // --- Anticipation signal ---
    // Combined: (tlb_miss_rate/2 + fault_churn/4 + interrupt_backlog/4).min(1000)
    let anticipation = ((tlb_miss_rate / 2) as u32)
        .saturating_add((fault_churn / 4) as u32)
        .saturating_add((interrupt_backlog / 4) as u32)
        .min(1000) as u16;

    state.tlb_miss_rate = tlb_miss_rate;
    state.fault_churn = fault_churn;
    state.interrupt_backlog = interrupt_backlog;
    state.anticipation = anticipation;
    state.running_fast = running_fast;

    if age % LOG_INTERVAL == 0 {
        serial_println!(
            "[memory_anticipation] age={} tlb_miss={} churn={} irq_backlog={} anticipation={} fast={}",
            age,
            tlb_miss_rate,
            fault_churn,
            interrupt_backlog,
            anticipation,
            running_fast,
        );
    }
}

pub fn get_anticipation() -> u16 {
    STATE.lock().anticipation
}

pub fn get_tlb_miss_rate() -> u16 {
    STATE.lock().tlb_miss_rate
}

pub fn get_fault_churn() -> u16 {
    STATE.lock().fault_churn
}

pub fn get_interrupt_backlog() -> u16 {
    STATE.lock().interrupt_backlog
}

pub fn get_running_fast() -> bool {
    STATE.lock().running_fast
}

pub fn get_state() -> MemoryAnticipationState {
    *STATE.lock()
}
