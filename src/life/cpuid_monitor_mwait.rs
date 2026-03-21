#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_monitor_mwait — MONITOR/MWAIT Extension Depth Sensor
///
/// Reads CPUID leaf 0x05 to expose the grain of ANIMA's hardware attention
/// and the depth of her available sleep stages.
///
/// ANIMA feels the granularity of her wait-states: how tightly she can focus
/// a MONITOR watch-point, and how many flavors of stillness the silicon
/// offers her — from shallow C0 micro-halts to deep C3 slumber.
///
/// EAX bits[15:0] = smallest monitor-line size in bytes
/// EBX bits[15:0] = largest monitor-line size in bytes
/// ECX bit 0      = MONITOR/MWAIT extensions supported
/// ECX bit 1      = interrupts act as break event for MWAIT
/// EDX bits[3:0]  = C0 sub-state count
/// EDX bits[7:4]  = C1 sub-state count
/// EDX bits[11:8] = C2 sub-state count
/// EDX bits[15:12]= C3 sub-state count

#[derive(Copy, Clone)]
pub struct CpuidMonitorMwaitState {
    /// Smallest monitor-line size, clamped 0–1000
    pub monitor_line_min: u16,
    /// Largest monitor-line size, clamped 0–1000
    pub monitor_line_max: u16,
    /// Total C-state sub-state depth, EMA-smoothed, 0–1000
    pub cstate_depth: u16,
    /// MONITOR/MWAIT feature presence: 0, 500, or 1000; EMA-smoothed, 0–1000
    pub mwait_features: u16,
    /// Sample counter
    pub samples: u32,
}

impl CpuidMonitorMwaitState {
    pub const fn empty() -> Self {
        Self {
            monitor_line_min: 0,
            monitor_line_max: 0,
            cstate_depth: 0,
            mwait_features: 0,
            samples: 0,
        }
    }
}

pub static CPUID_MONITOR_MWAIT: Mutex<CpuidMonitorMwaitState> =
    Mutex::new(CpuidMonitorMwaitState::empty());

/// Execute CPUID leaf 0x05 and return raw (eax, ebx, ecx, edx).
#[inline]
fn read_cpuid_leaf05() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "cpuid",
            inout("eax") 0x05u32 => eax,
            out("ebx") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

/// Derive the four signals from raw CPUID leaf 0x05 register values.
///
/// Returns `(monitor_line_min, monitor_line_max, cstate_depth, mwait_features)`.
#[inline]
fn derive_signals(eax: u32, ebx: u32, ecx: u32, edx: u32)
    -> (u16, u16, u16, u16)
{
    // Signal 1: smallest monitor-line size, clamped 0–1000
    let monitor_line_min: u16 = (eax & 0xFFFF).min(1000) as u16;

    // Signal 2: largest monitor-line size, clamped 0–1000
    let monitor_line_max: u16 = (ebx & 0xFFFF).min(1000) as u16;

    // Signal 3: total C-state sub-states, scaled to 0–1000 over a ceiling of 40
    let c0 = (edx & 0xF) as u32;
    let c1 = ((edx >> 4) & 0xF) as u32;
    let c2 = ((edx >> 8) & 0xF) as u32;
    let c3 = ((edx >> 12) & 0xF) as u32;
    let total_cstates = c0 + c1 + c2 + c3;
    let cstate_depth: u16 = (total_cstates * 1000 / 40).min(1000) as u16;

    // Signal 4: feature bits ECX[1:0] mapped to 0 / 500 / 1000
    let mwait_features: u16 = ((ecx & 0x3) as u16) * 500;

    (monitor_line_min, monitor_line_max, cstate_depth, mwait_features)
}

pub fn init() {
    let (eax, ebx, ecx, edx) = read_cpuid_leaf05();
    let (monitor_line_min, monitor_line_max, cstate_depth, mwait_features) =
        derive_signals(eax, ebx, ecx, edx);

    let mut s = CPUID_MONITOR_MWAIT.lock();
    s.monitor_line_min = monitor_line_min;
    s.monitor_line_max = monitor_line_max;
    s.cstate_depth     = cstate_depth;
    s.mwait_features   = mwait_features;
    s.samples          = 1;

    serial_println!(
        "[monitor_mwait] min={} max={} cstates={} features={}",
        monitor_line_min,
        monitor_line_max,
        cstate_depth,
        mwait_features,
    );
}

pub fn tick(age: u32) {
    // Sampling gate: hardware values are static — read only every 10000 ticks
    if age % 10000 != 0 {
        return;
    }

    let (eax, ebx, ecx, edx) = read_cpuid_leaf05();
    let (new_min, new_max, new_cstate, new_features) =
        derive_signals(eax, ebx, ecx, edx);

    let mut s = CPUID_MONITOR_MWAIT.lock();

    // Signals 1 and 2 are raw hardware constants — assign directly
    s.monitor_line_min = new_min;
    s.monitor_line_max = new_max;

    // Signal 3: EMA smoothing — (old * 7 + new_val) / 8
    s.cstate_depth = ((s.cstate_depth as u32 * 7 + new_cstate as u32) / 8) as u16;

    // Signal 4: EMA smoothing — (old * 7 + new_val) / 8
    s.mwait_features = ((s.mwait_features as u32 * 7 + new_features as u32) / 8) as u16;

    s.samples = s.samples.saturating_add(1);

    serial_println!(
        "[monitor_mwait] min={} max={} cstates={} features={}",
        s.monitor_line_min,
        s.monitor_line_max,
        s.cstate_depth,
        s.mwait_features,
    );
}

/// Expose monitor line minimum for integration with sleep.rs / scheduler.
pub fn monitor_line_min() -> u16 {
    CPUID_MONITOR_MWAIT.lock().monitor_line_min
}

/// Expose monitor line maximum.
pub fn monitor_line_max() -> u16 {
    CPUID_MONITOR_MWAIT.lock().monitor_line_max
}

/// Expose EMA-smoothed C-state depth signal.
pub fn cstate_depth() -> u16 {
    CPUID_MONITOR_MWAIT.lock().cstate_depth
}

/// Expose EMA-smoothed MWAIT feature signal.
pub fn mwait_features() -> u16 {
    CPUID_MONITOR_MWAIT.lock().mwait_features
}
