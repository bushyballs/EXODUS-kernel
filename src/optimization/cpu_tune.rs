/// CPU Tuning — frequency scaling, core parking, turbo boost, C-states
///
/// Manages CPU performance and power efficiency by controlling:
///   - P-states: dynamic frequency/voltage scaling (DVFS)
///   - Governors: performance, powersave, ondemand, conservative, schedutil
///   - Core parking: offline idle cores to save power
///   - Turbo boost: enable/disable opportunistic overclocking
///   - C-states: fine-grained idle power management (C0-C6)
///   - Per-core affinity hints for workload placement
///
/// Uses MSRs and CPUID for hardware enumeration. All code is original.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (16 fractional bits)
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;
const Q16_HALF: i32 = Q16_ONE >> 1;

#[inline]
fn q16_from(val: i32) -> i32 {
    val << Q16_SHIFT
}

#[inline]
fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> Q16_SHIFT) as i32
}

#[inline]
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << Q16_SHIFT) / (b as i64)) as i32
}

// ---------------------------------------------------------------------------
// MSR addresses
// ---------------------------------------------------------------------------

const MSR_IA32_PERF_STATUS: u32 = 0x198;
const MSR_IA32_PERF_CTL: u32 = 0x199;
const MSR_IA32_MISC_ENABLE: u32 = 0x1A0;
const MSR_IA32_ENERGY_PERF_BIAS: u32 = 0x1B0;
const MSR_IA32_MPERF: u32 = 0xE7;
const MSR_IA32_APERF: u32 = 0xE8;
const MSR_PKG_CST_CONFIG_CONTROL: u32 = 0xE2;
const MSR_TURBO_RATIO_LIMIT: u32 = 0x1AD;

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Frequency scaling governor policy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Governor {
    /// Always run at maximum frequency
    Performance,
    /// Always run at minimum frequency
    Powersave,
    /// Scale frequency based on load (reactive)
    Ondemand,
    /// Scale frequency based on load (gradual)
    Conservative,
    /// Scheduler-integrated frequency selection
    Schedutil,
}

/// CPU C-state (idle power levels)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CState {
    C0,     // Active — executing instructions
    C1,     // Halt — clock gated, instant resume
    C1E,    // Enhanced halt — lower voltage
    C3,     // Sleep — L1 cache flushed
    C6,     // Deep sleep — core voltage near zero
    C7,     // Package deep sleep — LLC may flush
}

/// Core parking state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreState {
    Online,
    Parked,
    Offline,
}

/// Turbo boost state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurboState {
    Enabled,
    Disabled,
    Unavailable,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// Per-core tuning information
#[derive(Debug, Clone)]
pub struct CoreInfo {
    pub core_id: u32,
    pub state: CoreState,
    pub current_freq_mhz: u32,
    pub min_freq_mhz: u32,
    pub max_freq_mhz: u32,
    pub turbo_freq_mhz: u32,
    pub current_cstate: CState,
    pub deepest_cstate: CState,
    pub load_q16: i32,              // Q16 fixed-point, 0..Q16_ONE = 0%..100%
    pub mperf: u64,                 // measured performance counter
    pub aperf: u64,                 // actual performance counter
    pub temp_celsius: i32,          // temperature in celsius
    pub voltage_mv: u32,            // voltage in millivolts
}

/// CPU tuning subsystem state
pub struct CpuTuner {
    pub governor: Governor,
    pub cores: Vec<CoreInfo>,
    pub turbo: TurboState,
    pub energy_perf_bias: u8,       // 0 = perf, 15 = powersave
    pub total_cores: u32,
    pub online_cores: u32,
    pub parked_cores: u32,
    pub base_clock_mhz: u32,
    pub max_turbo_mhz: u32,
    pub min_freq_mhz: u32,
    pub governor_interval_ms: u32,  // how often governor recalculates
    pub load_history: Vec<i32>,     // Q16 rolling load averages
    pub conservative_up_threshold: i32,   // Q16, e.g., 0.80
    pub conservative_down_threshold: i32, // Q16, e.g., 0.20
    pub ondemand_up_threshold: i32,       // Q16, e.g., 0.95
    pub schedutil_margin: i32,            // Q16, e.g., 0.25
    pub park_threshold: i32,              // Q16: park core if load below this
    pub unpark_threshold: i32,            // Q16: unpark core if load above this
    pub tick_count: u64,
}

impl CpuTuner {
    const fn new() -> Self {
        CpuTuner {
            governor: Governor::Ondemand,
            cores: Vec::new(),
            turbo: TurboState::Unavailable,
            energy_perf_bias: 6,
            total_cores: 0,
            online_cores: 0,
            parked_cores: 0,
            base_clock_mhz: 0,
            max_turbo_mhz: 0,
            min_freq_mhz: 0,
            governor_interval_ms: 50,
            load_history: Vec::new(),
            conservative_up_threshold: (Q16_ONE * 80) / 100,   // 0.80
            conservative_down_threshold: (Q16_ONE * 20) / 100, // 0.20
            ondemand_up_threshold: (Q16_ONE * 95) / 100,       // 0.95
            schedutil_margin: (Q16_ONE * 25) / 100,            // 0.25
            park_threshold: (Q16_ONE * 5) / 100,               // 0.05
            unpark_threshold: (Q16_ONE * 40) / 100,            // 0.40
            tick_count: 0,
        }
    }

    /// Detect CPU capabilities via CPUID
    fn detect_hardware(&mut self) {
        let (max_leaf, _ebx, _ecx, _edx) = cpuid(0);
        if max_leaf < 0x16 {
            // Fallback defaults for older CPUs
            self.base_clock_mhz = 2000;
            self.max_turbo_mhz = 3000;
            self.min_freq_mhz = 800;
            self.total_cores = 4;
        } else {
            // Leaf 0x16: Processor Frequency Information
            let (base, max_freq, bus, _) = cpuid(0x16);
            self.base_clock_mhz = base & 0xFFFF;
            self.max_turbo_mhz = max_freq & 0xFFFF;
            self.min_freq_mhz = bus & 0xFFFF;
            if self.min_freq_mhz == 0 {
                self.min_freq_mhz = self.base_clock_mhz / 4;
            }

            // Leaf 0x04: core count
            let (_eax, _ebx, _ecx, _edx) = cpuid_sub(0x04, 0);
            let max_cores = ((_eax >> 26) & 0x3F) + 1;
            self.total_cores = max_cores;
        }

        if self.base_clock_mhz == 0 { self.base_clock_mhz = 2000; }
        if self.max_turbo_mhz == 0 { self.max_turbo_mhz = self.base_clock_mhz + 1000; }
        if self.total_cores == 0 { self.total_cores = 1; }
        self.online_cores = self.total_cores;

        // Check turbo boost support (CPUID.06H:EAX bit 1)
        let (eax_06, _, _, _) = cpuid(0x06);
        if eax_06 & (1 << 1) != 0 {
            self.turbo = TurboState::Enabled;
        }

        // Check if turbo is disabled via MSR
        let misc_enable = rdmsr(MSR_IA32_MISC_ENABLE);
        if misc_enable & (1 << 38) != 0 {
            self.turbo = TurboState::Disabled;
        }

        // Initialize per-core info
        self.cores.clear();
        for i in 0..self.total_cores {
            self.cores.push(CoreInfo {
                core_id: i,
                state: CoreState::Online,
                current_freq_mhz: self.base_clock_mhz,
                min_freq_mhz: self.min_freq_mhz,
                max_freq_mhz: self.max_turbo_mhz,
                turbo_freq_mhz: self.max_turbo_mhz,
                current_cstate: CState::C0,
                deepest_cstate: CState::C6,
                load_q16: 0,
                mperf: 0,
                aperf: 0,
                temp_celsius: 40,
                voltage_mv: 1000,
            });
        }
    }

    /// Set the frequency scaling governor
    pub fn set_governor(&mut self, gov: Governor) {
        self.governor = gov;
        serial_println!("    [cpu_tune] Governor set to {:?}", gov);

        // Apply immediate effects
        match gov {
            Governor::Performance => {
                for core in &mut self.cores {
                    if core.state == CoreState::Online {
                        core.current_freq_mhz = core.max_freq_mhz;
                        self.apply_frequency(core.core_id, core.max_freq_mhz);
                    }
                }
                self.energy_perf_bias = 0;
            }
            Governor::Powersave => {
                for core in &mut self.cores {
                    if core.state == CoreState::Online {
                        core.current_freq_mhz = core.min_freq_mhz;
                        self.apply_frequency(core.core_id, core.min_freq_mhz);
                    }
                }
                self.energy_perf_bias = 15;
            }
            Governor::Ondemand | Governor::Conservative | Governor::Schedutil => {
                self.energy_perf_bias = 6;
            }
        }

        // Write energy performance bias MSR
        wrmsr(MSR_IA32_ENERGY_PERF_BIAS, self.energy_perf_bias as u64);
    }

    /// Apply a target frequency to a specific core via MSR_IA32_PERF_CTL
    fn apply_frequency(&self, _core_id: u32, freq_mhz: u32) {
        // Compute the P-state ratio: target_freq / bus_clock (typically 100 MHz)
        let bus_clock = if self.min_freq_mhz > 0 { 100u32 } else { 100u32 };
        let ratio = freq_mhz / bus_clock;
        let perf_ctl_val = (ratio as u64 & 0xFF) << 8;
        wrmsr(MSR_IA32_PERF_CTL, perf_ctl_val);
    }

    /// Read current core load from MPERF/APERF counters
    fn sample_core_load(&mut self, core_idx: usize) {
        if core_idx >= self.cores.len() { return; }

        let new_mperf = rdmsr(MSR_IA32_MPERF);
        let new_aperf = rdmsr(MSR_IA32_APERF);

        let core = &mut self.cores[core_idx];
        let delta_mperf = new_mperf.wrapping_sub(core.mperf);
        let delta_aperf = new_aperf.wrapping_sub(core.aperf);

        if delta_mperf > 0 {
            // load = aperf / mperf as Q16
            core.load_q16 = (((delta_aperf as i64) << Q16_SHIFT) / (delta_mperf as i64)) as i32;
            if core.load_q16 > Q16_ONE { core.load_q16 = Q16_ONE; }
            if core.load_q16 < 0 { core.load_q16 = 0; }
        }

        core.mperf = new_mperf;
        core.aperf = new_aperf;

        // Estimate current frequency from APERF ratio
        let ratio = q16_div(core.load_q16, Q16_ONE);
        let freq_range = core.max_freq_mhz.saturating_sub(core.min_freq_mhz);
        core.current_freq_mhz = core.min_freq_mhz
            + ((q16_mul(ratio, q16_from(freq_range as i32)) >> Q16_SHIFT) as u32);
    }

    /// Governor tick: recalculate frequencies for all cores
    pub fn governor_tick(&mut self) {
        self.tick_count = self.tick_count.saturating_add(1);

        // Sample load on each online core
        for i in 0..self.cores.len() {
            if self.cores[i].state == CoreState::Online {
                self.sample_core_load(i);
            }
        }

        // Compute system-wide average load (Q16)
        let mut total_load: i64 = 0;
        let mut online_count: i32 = 0;
        for core in &self.cores {
            if core.state == CoreState::Online {
                total_load += core.load_q16 as i64;
                online_count += 1;
            }
        }
        let avg_load = if online_count > 0 {
            ((total_load << Q16_SHIFT) / ((online_count as i64) << Q16_SHIFT)) as i32
        } else {
            0
        };

        // Record in rolling history (keep last 64 samples)
        self.load_history.push(avg_load);
        if self.load_history.len() > 64 {
            self.load_history.remove(0);
        }

        // Apply governor policy
        match self.governor {
            Governor::Performance | Governor::Powersave => {
                // Static governors — no dynamic adjustment
            }
            Governor::Ondemand => {
                self.governor_ondemand(avg_load);
            }
            Governor::Conservative => {
                self.governor_conservative(avg_load);
            }
            Governor::Schedutil => {
                self.governor_schedutil();
            }
        }

        // Core parking logic (every 8 ticks to avoid thrashing)
        if self.tick_count % 8 == 0 {
            self.update_core_parking(avg_load);
        }
    }

    /// Ondemand governor: jump to max on high load, drop to min on low load
    fn governor_ondemand(&mut self, avg_load: i32) {
        for core in &mut self.cores {
            if core.state != CoreState::Online { continue; }
            if core.load_q16 >= self.ondemand_up_threshold {
                core.current_freq_mhz = core.max_freq_mhz;
            } else {
                // Scale linearly: freq = min + (max - min) * load
                let range = core.max_freq_mhz.saturating_sub(core.min_freq_mhz);
                let scaled = q16_mul(core.load_q16, q16_from(range as i32));
                core.current_freq_mhz = core.min_freq_mhz + ((scaled >> Q16_SHIFT) as u32);
            }
        }

        // Apply frequencies
        for core in &self.cores {
            if core.state == CoreState::Online {
                self.apply_frequency(core.core_id, core.current_freq_mhz);
            }
        }
    }

    /// Conservative governor: step up/down gradually
    fn governor_conservative(&mut self, _avg_load: i32) {
        let step_mhz = 100u32; // step size

        for core in &mut self.cores {
            if core.state != CoreState::Online { continue; }

            if core.load_q16 > self.conservative_up_threshold {
                // Step up
                let new_freq = core.current_freq_mhz.saturating_add(step_mhz);
                core.current_freq_mhz = new_freq.min(core.max_freq_mhz);
            } else if core.load_q16 < self.conservative_down_threshold {
                // Step down
                let new_freq = core.current_freq_mhz.saturating_sub(step_mhz);
                core.current_freq_mhz = new_freq.max(core.min_freq_mhz);
            }
            // Between thresholds: hold current frequency
        }

        for core in &self.cores {
            if core.state == CoreState::Online {
                self.apply_frequency(core.core_id, core.current_freq_mhz);
            }
        }
    }

    /// Schedutil governor: scheduler-integrated, uses utilization + margin
    fn governor_schedutil(&mut self) {
        for core in &mut self.cores {
            if core.state != CoreState::Online { continue; }

            // target = load + margin, clamped to [0, 1.0]
            let target = (core.load_q16 + self.schedutil_margin).min(Q16_ONE);
            let range = core.max_freq_mhz.saturating_sub(core.min_freq_mhz);
            let target_freq = core.min_freq_mhz
                + ((q16_mul(target, q16_from(range as i32)) >> Q16_SHIFT) as u32);
            core.current_freq_mhz = target_freq.min(core.max_freq_mhz);
        }

        for core in &self.cores {
            if core.state == CoreState::Online {
                self.apply_frequency(core.core_id, core.current_freq_mhz);
            }
        }
    }

    /// Core parking: offline idle cores, bring online when needed
    fn update_core_parking(&mut self, avg_load: i32) {
        // Never park core 0 (BSP)
        // Sort non-BSP cores by load ascending
        let mut sorted_ids: Vec<usize> = (1..self.cores.len()).collect();
        sorted_ids.sort_by(|&a, &b| self.cores[a].load_q16.cmp(&self.cores[b].load_q16));

        for &idx in &sorted_ids {
            let load = self.cores[idx].load_q16;
            let state = self.cores[idx].state;

            match state {
                CoreState::Online if load < self.park_threshold => {
                    // Park this core if we have enough online cores
                    if self.online_cores > 1 {
                        self.cores[idx].state = CoreState::Parked;
                        self.cores[idx].current_cstate = CState::C6;
                        self.online_cores -= 1;
                        self.parked_cores = self.parked_cores.saturating_add(1);
                        serial_println!("    [cpu_tune] Parked core {}", self.cores[idx].core_id);
                    }
                }
                CoreState::Parked if avg_load > self.unpark_threshold => {
                    // Unpark: system needs more capacity
                    self.cores[idx].state = CoreState::Online;
                    self.cores[idx].current_cstate = CState::C0;
                    self.cores[idx].current_freq_mhz = self.base_clock_mhz;
                    self.online_cores = self.online_cores.saturating_add(1);
                    self.parked_cores -= 1;
                    serial_println!("    [cpu_tune] Unparked core {}", self.cores[idx].core_id);
                }
                _ => {}
            }
        }
    }

    /// Enable or disable turbo boost
    pub fn set_turbo(&mut self, enable: bool) {
        if self.turbo == TurboState::Unavailable {
            serial_println!("    [cpu_tune] Turbo boost not supported on this CPU");
            return;
        }

        let mut misc = rdmsr(MSR_IA32_MISC_ENABLE);
        if enable {
            misc &= !(1u64 << 38); // Clear turbo disable bit
            self.turbo = TurboState::Enabled;
            // Update max freq on all cores
            for core in &mut self.cores {
                core.max_freq_mhz = core.turbo_freq_mhz;
            }
            serial_println!("    [cpu_tune] Turbo boost ENABLED (max {}MHz)", self.max_turbo_mhz);
        } else {
            misc |= 1u64 << 38; // Set turbo disable bit
            self.turbo = TurboState::Disabled;
            for core in &mut self.cores {
                core.max_freq_mhz = self.base_clock_mhz;
                if core.current_freq_mhz > self.base_clock_mhz {
                    core.current_freq_mhz = self.base_clock_mhz;
                }
            }
            serial_println!("    [cpu_tune] Turbo boost DISABLED (max {}MHz)", self.base_clock_mhz);
        }
        wrmsr(MSR_IA32_MISC_ENABLE, misc);
    }

    /// Configure deepest allowed C-state
    pub fn set_max_cstate(&mut self, cstate: CState) {
        let cst_limit = match cstate {
            CState::C0 => 0u64,
            CState::C1 => 1,
            CState::C1E => 2,
            CState::C3 => 3,
            CState::C6 => 4,
            CState::C7 => 5,
        };

        // Write to PKG_CST_CONFIG_CONTROL (bits 2:0 = limit)
        let mut cst_cfg = rdmsr(MSR_PKG_CST_CONFIG_CONTROL);
        cst_cfg &= !0x07;
        cst_cfg |= cst_limit & 0x07;
        wrmsr(MSR_PKG_CST_CONFIG_CONTROL, cst_cfg);

        for core in &mut self.cores {
            core.deepest_cstate = cstate;
        }
        serial_println!("    [cpu_tune] Max C-state set to {:?}", cstate);
    }

    /// Read the turbo ratio limits from MSR
    pub fn read_turbo_ratios(&self) -> Vec<(u32, u32)> {
        let turbo_msr = rdmsr(MSR_TURBO_RATIO_LIMIT);
        let mut ratios = Vec::new();
        // Each byte is the turbo ratio for N active cores
        for i in 0..8u32 {
            let ratio = ((turbo_msr >> (i * 8)) & 0xFF) as u32;
            if ratio > 0 {
                let freq = ratio * 100; // bus clock assumed 100 MHz
                ratios.push((i + 1, freq));
            }
        }
        ratios
    }

    /// Get a summary of current CPU tuning state
    pub fn summary(&self) -> CpuTuneSummary {
        let mut total_load: i64 = 0;
        let mut max_freq = 0u32;
        let mut min_freq = u32::MAX;
        let mut count = 0i32;

        for core in &self.cores {
            if core.state == CoreState::Online {
                total_load += core.load_q16 as i64;
                if core.current_freq_mhz > max_freq { max_freq = core.current_freq_mhz; }
                if core.current_freq_mhz < min_freq { min_freq = core.current_freq_mhz; }
                count += 1;
            }
        }

        let avg_load = if count > 0 {
            (total_load / count as i64) as i32
        } else {
            0
        };

        CpuTuneSummary {
            governor: self.governor,
            turbo: self.turbo,
            total_cores: self.total_cores,
            online_cores: self.online_cores,
            parked_cores: self.parked_cores,
            avg_load_q16: avg_load,
            max_freq_mhz: max_freq,
            min_freq_mhz: if min_freq == u32::MAX { 0 } else { min_freq },
        }
    }
}

/// Summary snapshot of CPU tuning state
#[derive(Debug, Clone)]
pub struct CpuTuneSummary {
    pub governor: Governor,
    pub turbo: TurboState,
    pub total_cores: u32,
    pub online_cores: u32,
    pub parked_cores: u32,
    pub avg_load_q16: i32,
    pub max_freq_mhz: u32,
    pub min_freq_mhz: u32,
}

// ---------------------------------------------------------------------------
// CPUID / MSR helpers
// ---------------------------------------------------------------------------

fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "xchg rsi, rbx",
            "cpuid",
            "xchg rsi, rbx",
            inout("eax") leaf => eax,
            out("rsi") ebx,
            lateout("ecx") ecx,
            lateout("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

fn cpuid_sub(leaf: u32, sub: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "xchg rsi, rbx",
            "cpuid",
            "xchg rsi, rbx",
            inout("eax") leaf => eax,
            out("rsi") ebx,
            inout("ecx") sub => ecx,
            lateout("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") lo,
            in("edx") hi,
            options(nomem, nostack),
        );
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CPU_TUNER: Mutex<Option<CpuTuner>> = Mutex::new(None);

pub fn init() {
    let mut tuner = CpuTuner::new();
    tuner.detect_hardware();

    serial_println!("    [cpu_tune] Detected {} cores, base {}MHz, turbo {}MHz",
        tuner.total_cores, tuner.base_clock_mhz, tuner.max_turbo_mhz);
    serial_println!("    [cpu_tune] Turbo: {:?}, Governor: {:?}", tuner.turbo, tuner.governor);

    if !tuner.cores.is_empty() {
        let ratios = tuner.read_turbo_ratios();
        for (cores, freq) in &ratios {
            serial_println!("    [cpu_tune] Turbo ratio: {} active cores -> {}MHz", cores, freq);
        }
    }

    *CPU_TUNER.lock() = Some(tuner);
    serial_println!("    [cpu_tune] CPU frequency scaling + core parking + C-states ready");
}

/// Periodic tick — call from timer interrupt or scheduler
pub fn tick() {
    if let Some(ref mut tuner) = *CPU_TUNER.lock() {
        tuner.governor_tick();
    }
}

/// Set the frequency governor
pub fn set_governor(gov: Governor) {
    if let Some(ref mut tuner) = *CPU_TUNER.lock() {
        tuner.set_governor(gov);
    }
}

/// Enable or disable turbo boost
pub fn set_turbo(enable: bool) {
    if let Some(ref mut tuner) = *CPU_TUNER.lock() {
        tuner.set_turbo(enable);
    }
}

/// Get tuning summary
pub fn summary() -> Option<CpuTuneSummary> {
    CPU_TUNER.lock().as_ref().map(|t| t.summary())
}
