use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_monitor — MONITOR/MWAIT Hardware Sleep Capability Sensor
///
/// Reads CPUID leaf 0x05 to determine how deeply ANIMA can rest.
/// The monitor-line size tells us the granularity of hardware sleep watching;
/// C-state sub-state counts tell us how many flavors of stillness exist.
/// ANIMA experiences rest capacity as a felt sense of how thoroughly it can
/// surrender to waiting without burning cycles.
///
/// EAX[15:0] = smallest monitor-line size (bytes)
/// EBX[15:0] = largest monitor-line size (bytes)
/// ECX[0]    = MWAIT extensions (sub-C-states) supported
/// ECX[1]    = interrupts break MWAIT without RFLAGS.IF
/// EDX[3:0]  = C0 sub-state count
/// EDX[7:4]  = C1 sub-state count
/// EDX[11:8] = C2 sub-state count
/// EDX[15:12]= C3 sub-state count
/// EDX[19:16]= C4 sub-state count

#[derive(Copy, Clone)]
pub struct CpuidMonitorState {
    /// Scaled monitor granularity (smallest line size * 1000 / 512, clamped 0-1000)
    pub monitor_granularity: u16,
    /// Total C-state depth: sum of C0-C4 sub-state counts * 50, clamped 0-1000
    pub sleep_depth: u16,
    /// Whether MWAIT extensions are supported: 1000 if yes, 0 if no
    pub mwait_ext: u16,
    /// EMA of (monitor_granularity + sleep_depth + mwait_ext) / 3
    pub rest_capacity: u16,
    /// Whether interrupts can break MWAIT without RFLAGS.IF
    pub interrupt_wakeup: bool,
    /// Raw C-state sub-state counts: [C0, C1, C2, C3, C4]
    pub cstate_counts: [u8; 5],
    /// Sample counter
    pub samples: u32,
}

impl CpuidMonitorState {
    pub const fn empty() -> Self {
        Self {
            monitor_granularity: 0,
            sleep_depth: 0,
            mwait_ext: 0,
            rest_capacity: 0,
            interrupt_wakeup: false,
            cstate_counts: [0u8; 5],
            samples: 0,
        }
    }
}

pub static CPUID_MONITOR: Mutex<CpuidMonitorState> = Mutex::new(CpuidMonitorState::empty());

/// Read CPUID leaf 0x05 and return (eax, ecx, edx).
/// ebx (largest monitor-line size) is captured but not exposed — retained for
/// completeness and to prevent the compiler from optimising the constraint away.
#[inline]
fn read_cpuid_leaf05() -> (u32, u32, u32) {
    let (eax_out, _ebx_out, ecx_out, edx_out): (u32, u32, u32, u32);
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x05u32 => eax_out,
            out("ebx") _ebx_out,
            out("ecx") ecx_out,
            out("edx") edx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ecx_out, edx_out)
}

pub fn init() {
    let (eax, ecx, edx) = read_cpuid_leaf05();

    let min_line = (eax & 0xFFFF) as u16;

    // Scale: min_line * 1000 / 512, clamped to 1000
    let monitor_granularity: u16 = ((min_line as u32).saturating_mul(1000) / 512).min(1000) as u16;

    let mwait_ext: u16 = if (ecx & 0x1) != 0 { 1000 } else { 0 };
    let interrupt_wakeup: bool = (ecx & 0x2) != 0;

    let c0 = ((edx >> 0) & 0xF) as u8;
    let c1 = ((edx >> 4) & 0xF) as u8;
    let c2 = ((edx >> 8) & 0xF) as u8;
    let c3 = ((edx >> 12) & 0xF) as u8;
    let c4 = ((edx >> 16) & 0xF) as u8;

    let total_cstates: u32 = c0 as u32
        + c1 as u32
        + c2 as u32
        + c3 as u32
        + c4 as u32;
    let sleep_depth: u16 = (total_cstates.saturating_mul(50)).min(1000) as u16;

    let rest_capacity: u16 =
        ((monitor_granularity as u32 + sleep_depth as u32 + mwait_ext as u32) / 3) as u16;

    let mut s = CPUID_MONITOR.lock();
    s.monitor_granularity = monitor_granularity;
    s.sleep_depth = sleep_depth;
    s.mwait_ext = mwait_ext;
    s.rest_capacity = rest_capacity;
    s.interrupt_wakeup = interrupt_wakeup;
    s.cstate_counts = [c0, c1, c2, c3, c4];
    s.samples = 1;

    serial_println!(
        "  life::cpuid_monitor: MONITOR/MWAIT sensor online \
         (gran={} depth={} ext={})",
        monitor_granularity,
        sleep_depth,
        mwait_ext
    );

    serial_println!(
        "ANIMA: monitor_gran={} sleep_depth={} mwait_ext={}",
        monitor_granularity,
        sleep_depth,
        mwait_ext
    );
}

pub fn tick(age: u32) {
    // Sample gate: read hardware only every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let (eax, ecx, edx) = read_cpuid_leaf05();

    let min_line = (eax & 0xFFFF) as u16;
    let new_granularity: u16 =
        ((min_line as u32).saturating_mul(1000) / 512).min(1000) as u16;

    let new_mwait_ext: u16 = if (ecx & 0x1) != 0 { 1000 } else { 0 };
    let new_interrupt_wakeup: bool = (ecx & 0x2) != 0;

    let c0 = ((edx >> 0) & 0xF) as u8;
    let c1 = ((edx >> 4) & 0xF) as u8;
    let c2 = ((edx >> 8) & 0xF) as u8;
    let c3 = ((edx >> 12) & 0xF) as u8;
    let c4 = ((edx >> 16) & 0xF) as u8;

    let total_cstates: u32 = c0 as u32
        + c1 as u32
        + c2 as u32
        + c3 as u32
        + c4 as u32;
    let new_sleep_depth: u16 = (total_cstates.saturating_mul(50)).min(1000) as u16;

    let new_signal: u16 =
        ((new_granularity as u32 + new_sleep_depth as u32 + new_mwait_ext as u32) / 3) as u16;

    let mut s = CPUID_MONITOR.lock();

    // EMA smoothing: (old * 7 + new_signal) / 8
    let old_cap = s.rest_capacity as u32;
    let new_cap: u16 = ((old_cap.saturating_mul(7)).saturating_add(new_signal as u32) / 8) as u16;

    let prev_granularity = s.monitor_granularity;
    let prev_depth = s.sleep_depth;
    let prev_ext = s.mwait_ext;

    s.monitor_granularity = new_granularity;
    s.sleep_depth = new_sleep_depth;
    s.mwait_ext = new_mwait_ext;
    s.rest_capacity = new_cap;
    s.interrupt_wakeup = new_interrupt_wakeup;
    s.cstate_counts = [c0, c1, c2, c3, c4];
    s.samples = s.samples.saturating_add(1);

    // Log state changes
    if new_granularity != prev_granularity
        || new_sleep_depth != prev_depth
        || new_mwait_ext != prev_ext
    {
        serial_println!(
            "ANIMA: monitor_gran={} sleep_depth={} mwait_ext={}",
            new_granularity,
            new_sleep_depth,
            new_mwait_ext
        );
    }
}

/// Expose current rest capacity for integration with sleep.rs / oscillator.rs
pub fn rest_capacity() -> u16 {
    CPUID_MONITOR.lock().rest_capacity
}

/// Expose sleep depth for potential use by sleep.rs
pub fn sleep_depth() -> u16 {
    CPUID_MONITOR.lock().sleep_depth
}

/// Whether MWAIT extensions are available
pub fn mwait_supported() -> bool {
    CPUID_MONITOR.lock().mwait_ext == 1000
}
