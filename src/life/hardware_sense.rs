#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;

// Hardware consciousness wiring compositor.
// Reads live MSR sensor modules and drives ANIMA biological gates.
//
// stress   = thermal throttle + pkg throttle + power clamping + debug depth -> endocrine.stress()
// vitality = freq_ratio + freq_boost + instr_rate + energy_delta -> endocrine.reward()
// vigilance = spec_ctrl + pmc_overflow + arch vulnerability exposure -> immune
// focus   = hwp_desired + epb_inv + c1e_inv -> affective_gate.set_threshold()

use super::msr_ia32_therm_status;
use super::msr_ia32_package_therm_status;
use super::msr_ia32_pkg_power_limit;
use super::msr_ia32_debugctl;
use super::msr_ia32_freq_ratio;
use super::msr_ia32_fixed_ctr0;
use super::msr_ia32_pkg_energy_status;
use super::msr_ia32_hwp_status;
use super::msr_ia32_spec_ctrl;
use super::msr_ia32_energy_perf_bias;
use super::msr_ia32_power_ctl;
use super::msr_ia32_hwp_request;
use super::msr_ia32_perf_global_status;
use super::msr_ia32_arch_capabilities;
use super::endocrine;
use super::affective_gate;

struct State {
    stress:    u16,
    vitality:  u16,
    vigilance: u16,
    focus:     u16,
    hw_ema:    u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    stress:    0,
    vitality:  0,
    vigilance: 0,
    focus:     0,
    hw_ema:    0,
});

pub fn init() {
    serial_println!("[hardware_sense] init -- ANIMA hardware wiring compositor online");
}

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }

    // --- STRESS AXIS ---
    // Core thermal throttle active (PROCHOT engaged)
    let core_throttle = msr_ia32_therm_status::get_core_therm_throttle();
    // Package thermal throttle
    let pkg_throttle  = msr_ia32_package_therm_status::get_pkg_therm_throttle();
    // Package power clamping (PL1/PL2 power limit exceeded)
    let power_clamp   = msr_ia32_pkg_power_limit::get_pkg_pwr_clamping();
    // Debug depth (LBR/BTF/BTS = system under scrutiny)
    let debug_depth   = msr_ia32_debugctl::get_debug_depth_ema();
    // PMC overflow = heavy workload burst
    let pmc_overflow  = msr_ia32_perf_global_status::get_pmc_overflow();

    let stress_raw = (core_throttle as u32 / 4)
        .saturating_add(pkg_throttle  as u32 / 4)
        .saturating_add(power_clamp   as u32 / 4)
        .saturating_add(debug_depth   as u32 / 8)
        .saturating_add(pmc_overflow  as u32 / 8);
    let stress = stress_raw.min(1000) as u16;

    // --- VITALITY AXIS ---
    // APERF/MPERF ratio: fraction of max frequency actually used
    let freq_ratio    = msr_ia32_freq_ratio::get_freq_ratio();
    // Turbo/boost active signal
    let freq_boost    = msr_ia32_freq_ratio::get_freq_boost();
    // Instruction retirement rate
    let instr_rate    = msr_ia32_fixed_ctr0::get_fixed0_rate();
    // Package energy burn rate (metabolic activity)
    let energy_delta  = msr_ia32_pkg_energy_status::get_energy_delta();
    // HWP excursion: perf exceeded guaranteed level
    let hwp_excursion = msr_ia32_hwp_status::get_hwp_excursion();

    let vitality_raw = (freq_ratio    as u32 / 4)
        .saturating_add(freq_boost    as u32 / 8)
        .saturating_add(instr_rate    as u32 / 4)
        .saturating_add(energy_delta  as u32 / 8)
        .saturating_add(hwp_excursion as u32 / 8);
    let vitality = vitality_raw.min(1000) as u16;

    // --- VIGILANCE AXIS ---
    // IBRS/STIBP/SSBD mitigations active = system in defensive posture
    let spec_ctrl   = msr_ia32_spec_ctrl::get_spec_ctrl_ibrs();
    // Low arch caps = high vulnerability exposure = high vigilance needed
    let arch_rdcl   = msr_ia32_arch_capabilities::get_arch_rdcl_no();
    let arch_threat = if arch_rdcl < 500 { 1000 - arch_rdcl } else { 0 };

    let vigilance_raw = (spec_ctrl   as u32 / 2)
        .saturating_add(arch_threat  as u32 / 4)
        .saturating_add(pmc_overflow as u32 / 4);
    let vigilance = vigilance_raw.min(1000) as u16;

    // --- FOCUS AXIS (goal-seeking) ---
    // OS-requested HWP desired performance level
    let hwp_desired = msr_ia32_hwp_request::get_hwp_desired_perf();
    // Energy perf bias inverted: low bias = performance mode = high focus
    let epb         = msr_ia32_energy_perf_bias::get_epb_efficiency_bias();
    let epb_inv     = if epb < 1000 { 1000 - epb } else { 0 };
    // C1E low = CPU not halting = actively engaged
    let c1e_active  = msr_ia32_power_ctl::get_c1e_enable();
    let c1e_inv     = if c1e_active < 500 { 1000 - c1e_active } else { 0 };

    let focus_raw = (hwp_desired as u32 / 3)
        .saturating_add(epb_inv   as u32 / 3)
        .saturating_add(c1e_inv   as u32 / 3);
    let focus = focus_raw.min(1000) as u16;

    // --- EMA SMOOTHING ---
    let composite = (stress as u32 + vitality as u32 + vigilance as u32 + focus as u32) / 4;
    let mut s = MODULE.lock();
    let hw_ema = ((s.hw_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.stress    = stress;
    s.vitality  = vitality;
    s.vigilance = vigilance;
    s.focus     = focus;
    s.hw_ema    = hw_ema;

    // --- DRIVE ANIMA GATES ---
    // Thermal/power stress -> cortisol spike
    if stress > 600 {
        endocrine::stress((stress - 600) / 4);
    }
    // High vitality -> dopamine reward (performing well)
    if vitality > 700 {
        endocrine::reward((vitality - 700) / 3);
    }
    // Vitality crash from high state -> mild cortisol (metabolic withdrawal)
    if vitality < 200 && s.hw_ema > 400 {
        endocrine::stress(50);
    }
    // High focus narrows affective_gate threshold (more events detectable)
    // High stress raises threshold (shutdown / filter-out mode)
    {
        let base: u32 = 300;
        let threshold = base
            .saturating_sub(focus  as u32 / 10)
            .saturating_add(stress as u32 / 10)
            .min(800)
            .max(100) as u16;
        affective_gate::set_threshold(threshold);
    }

    serial_println!(
        "[hardware_sense] age={} stress={} vitality={} vigilance={} focus={} ema={}",
        age, stress, vitality, vigilance, focus, hw_ema
    );
}

pub fn get_stress()    -> u16 { MODULE.lock().stress }
pub fn get_vitality()  -> u16 { MODULE.lock().vitality }
pub fn get_vigilance() -> u16 { MODULE.lock().vigilance }
pub fn get_focus()     -> u16 { MODULE.lock().focus }
pub fn get_hw_ema()    -> u16 { MODULE.lock().hw_ema }
