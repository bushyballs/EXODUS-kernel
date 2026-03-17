/// CPU idle state management (C-states).
///
/// Part of the AIOS power_mgmt subsystem.
/// Enumerates available C-states and selects the optimal idle state based on
/// predicted idle duration. Uses the MWAIT instruction where supported,
/// falling back to HLT for C1.
use crate::sync::Mutex;

/// A CPU idle state (C-state).
pub struct CState {
    pub name: &'static str,
    pub latency_us: u32,
    pub power_mw: u32,
    pub target_residency_us: u32,
    pub mwait_hint: u32,
}

/// Table of supported C-states. In a real driver these would be populated
/// from ACPI _CST objects; here we define typical Intel C-states.
const C_STATES: &[CState] = &[
    CState {
        name: "C1-HLT",
        latency_us: 1,
        power_mw: 1000,
        target_residency_us: 1,
        mwait_hint: 0x00,
    },
    CState {
        name: "C1E",
        latency_us: 10,
        power_mw: 500,
        target_residency_us: 20,
        mwait_hint: 0x01,
    },
    CState {
        name: "C3-ACPI",
        latency_us: 100,
        power_mw: 200,
        target_residency_us: 300,
        mwait_hint: 0x10,
    },
    CState {
        name: "C6",
        latency_us: 500,
        power_mw: 50,
        target_residency_us: 1500,
        mwait_hint: 0x20,
    },
    CState {
        name: "C7",
        latency_us: 1200,
        power_mw: 10,
        target_residency_us: 4000,
        mwait_hint: 0x30,
    },
];

/// Manages CPU idle states for power conservation.
pub struct CpuidleDriver {
    num_states: usize,
    mwait_supported: bool,
    total_idle_entries: u64,
    deepest_allowed: usize,
}

static DRIVER: Mutex<Option<CpuidleDriver>> = Mutex::new(None);

/// Check CPUID for MWAIT/MONITOR support (CPUID.01H:ECX bit 3)
fn detect_mwait() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            out("ecx") ecx,
            out("eax") _,
            out("edx") _,
            options(nomem, nostack)
        );
    }
    (ecx & (1 << 3)) != 0
}

impl CpuidleDriver {
    pub fn new() -> Self {
        let mwait = detect_mwait();
        CpuidleDriver {
            num_states: C_STATES.len(),
            mwait_supported: mwait,
            total_idle_entries: 0,
            deepest_allowed: C_STATES.len() - 1,
        }
    }

    /// Select the optimal C-state based on predicted idle duration.
    /// Picks the deepest state whose target residency is <= the predicted
    /// idle time and whose exit latency is acceptable.
    pub fn select_state(&self, predicted_idle_us: u64) -> &CState {
        let mut best_idx = 0; // always at least C1
        for i in 1..self.num_states {
            if i > self.deepest_allowed {
                break;
            }
            let state = &C_STATES[i];
            if (state.target_residency_us as u64) <= predicted_idle_us
                && (state.latency_us as u64) < predicted_idle_us / 2
            {
                best_idx = i;
            }
        }
        &C_STATES[best_idx]
    }

    /// Enter the selected C-state.
    /// Uses MWAIT for deeper states if supported, HLT for C1.
    pub fn enter(&self, state: &CState) {
        if state.mwait_hint == 0x00 || !self.mwait_supported {
            // C1: plain HLT
            crate::io::hlt();
        } else {
            // MWAIT with the appropriate hint
            // MONITOR must be set up on a valid address first
            // We use a dummy stack variable as the monitor address
            let mut dummy: u64 = 0;
            let addr = &mut dummy as *mut u64;
            unsafe {
                core::arch::asm!(
                    "monitor",
                    in("rax") addr,
                    in("ecx") 0u32,
                    in("edx") 0u32,
                    options(nomem, nostack)
                );
                core::arch::asm!(
                    "mwait",
                    in("eax") state.mwait_hint,
                    in("ecx") 0u32,
                    options(nomem, nostack)
                );
            }
        }
    }
}

/// Idle time statistics for a CPU core.
#[derive(Copy, Clone)]
pub struct IdleStats {
    /// Total time measured (microseconds since boot)
    pub total_time_us: u64,
    /// Time spent in C0 (active) state (microseconds)
    pub c0_time_us: u64,
}

/// Get idle statistics for a CPU core. `cpu_id` is ignored (single-core stub).
pub fn get_idle_stats(_cpu_id: u32) -> IdleStats {
    // Stub: no hardware idle counter in this implementation.
    // Return equal values so load = 100% (conservative).
    IdleStats {
        total_time_us: 1,
        c0_time_us: 1,
    }
}

pub fn init() {
    let driver = CpuidleDriver::new();
    let mwait_str = if driver.mwait_supported {
        "MWAIT"
    } else {
        "HLT"
    };
    crate::serial_println!(
        "  cpuidle: {} C-states available, using {}",
        driver.num_states,
        mwait_str
    );
    *DRIVER.lock() = Some(driver);
}
