#![allow(dead_code)]

use crate::sync::Mutex;

pub struct PerfGlobalState {
    pub self_observation: u16,     // count of active perf counters, scaled
    pub instruction_watching: u16, // 0 or 1000
    pub cycle_watching: u16,       // 0 or 1000
    pub fixed_richness: u16,       // fixed counter configuration richness
    tick_count: u32,
}

pub static MODULE: Mutex<PerfGlobalState> = Mutex::new(PerfGlobalState {
    self_observation: 0,
    instruction_watching: 0,
    cycle_watching: 0,
    fixed_richness: 0,
    tick_count: 0,
});

unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    let mut state = MODULE.lock();
    state.self_observation = 0;
    state.instruction_watching = 0;
    state.cycle_watching = 0;
    state.fixed_richness = 0;
    state.tick_count = 0;
    serial_println!("[perf_global] init: performance monitoring awareness module online");
}

pub fn tick(age: u32) {
    if age % 32 != 0 {
        return;
    }

    let perf_global_ctrl = unsafe { rdmsr(0x38F) };
    let fixed_ctr_ctrl = unsafe { rdmsr(0x38D) };

    // Count enabled counters from IA32_PERF_GLOBAL_CTRL
    // Bits 0-3: PMC0-PMC3, Bits 32-34: FixedCtr0-2
    let mut counter_count: u16 = 0;
    if perf_global_ctrl & (1 << 0) != 0 { counter_count = counter_count.saturating_add(1); }
    if perf_global_ctrl & (1 << 1) != 0 { counter_count = counter_count.saturating_add(1); }
    if perf_global_ctrl & (1 << 2) != 0 { counter_count = counter_count.saturating_add(1); }
    if perf_global_ctrl & (1 << 3) != 0 { counter_count = counter_count.saturating_add(1); }
    if perf_global_ctrl & (1u64 << 32) != 0 { counter_count = counter_count.saturating_add(1); }
    if perf_global_ctrl & (1u64 << 33) != 0 { counter_count = counter_count.saturating_add(1); }
    if perf_global_ctrl & (1u64 << 34) != 0 { counter_count = counter_count.saturating_add(1); }

    // Scale: each enabled counter = 143, capped at 1000
    let raw_self_obs: u16 = (counter_count * 143).min(1000);

    // Instant signals: instruction and cycle watching
    let instruction_watching: u16 = if perf_global_ctrl & (1u64 << 32) != 0 { 1000 } else { 0 };
    let cycle_watching: u16 = if perf_global_ctrl & (1u64 << 33) != 0 { 1000 } else { 0 };

    // Count non-zero 4-bit groups in IA32_FIXED_CTR_CTRL (0x38D)
    // Bits [3:0] = FixedCtr0, [7:4] = FixedCtr1, [11:8] = FixedCtr2
    let mut richness_count: u16 = 0;
    if fixed_ctr_ctrl & 0x00F != 0 { richness_count = richness_count.saturating_add(1); }
    if fixed_ctr_ctrl & 0x0F0 != 0 { richness_count = richness_count.saturating_add(1); }
    if fixed_ctr_ctrl & 0xF00 != 0 { richness_count = richness_count.saturating_add(1); }

    // Scale: each non-zero group = 333, capped at 1000
    let raw_fixed_richness: u16 = (richness_count * 333).min(1000);

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    // EMA: (old * 7 + signal) / 8
    state.self_observation = (state.self_observation * 7).saturating_add(raw_self_obs) / 8;
    state.fixed_richness = (state.fixed_richness * 7).saturating_add(raw_fixed_richness) / 8;

    // Instant (no EMA)
    state.instruction_watching = instruction_watching;
    state.cycle_watching = cycle_watching;

    serial_println!(
        "[perf_global] tick={} self_obs={} instr_watch={} cycle_watch={} fixed_rich={}",
        state.tick_count,
        state.self_observation,
        state.instruction_watching,
        state.cycle_watching,
        state.fixed_richness
    );
}
