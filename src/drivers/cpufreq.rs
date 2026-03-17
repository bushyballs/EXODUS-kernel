use crate::sync::Mutex;
/// CPU frequency scaling driver for Genesis — no-heap, fixed-size arrays
///
/// Manages per-CPU (or per-cluster) frequency scaling policies. Supports
/// six governor types: Performance, Powersave, Ondemand, Conservative,
/// Schedutil, and Userspace. Each policy tracks available P-state
/// frequencies, current CPU load, and a governor-computed target.
///
/// This module sits above the low-level `power_mgmt::cpufreq` P-state
/// driver and provides a Linux cpufreq-compatible policy interface.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for counters
///   - Wrapping arithmetic for sequence numbers
///   - Division always guarded (divisor checked != 0 before use)
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of simultaneous cpufreq policies
pub const MAX_CPUFREQ_POLICIES: usize = 8;

/// Maximum number of frequency table entries (P-states) per policy
pub const MAX_FREQ_TABLE_ENTRIES: usize = 16;

// ---------------------------------------------------------------------------
// Governor type constants
// ---------------------------------------------------------------------------

/// Always run at maximum frequency
pub const GOV_PERFORMANCE: u8 = 0;
/// Always run at minimum frequency
pub const GOV_POWERSAVE: u8 = 1;
/// Scale aggressively based on CPU load
pub const GOV_ONDEMAND: u8 = 2;
/// Scale gradually (step one P-state at a time)
pub const GOV_CONSERVATIVE: u8 = 3;
/// Scheduler-driven — use CFS utilisation signal
pub const GOV_SCHEDUTIL: u8 = 4;
/// User manually controls target frequency
pub const GOV_USERSPACE: u8 = 5;

// ---------------------------------------------------------------------------
// CpufreqPolicy struct
// ---------------------------------------------------------------------------

/// Per-CPU (or per-core-cluster) frequency scaling policy.
#[derive(Copy, Clone)]
pub struct CpufreqPolicy {
    /// Representative CPU id for this policy (e.g. CPU 0 for a cluster)
    pub cpu: u32,
    /// Minimum allowed frequency in kHz
    pub min_freq_khz: u32,
    /// Maximum allowed frequency in kHz
    pub max_freq_khz: u32,
    /// Currently active frequency in kHz (as set by the last governor tick)
    pub cur_freq_khz: u32,
    /// Active governor (GOV_* constant)
    pub governor: u8,
    /// Target frequency computed or set by the governor / user (kHz)
    pub target_freq_khz: u32,
    /// Available frequencies in ascending order (kHz); entries 0..freq_table_len valid
    pub freq_table: [u32; MAX_FREQ_TABLE_ENTRIES],
    /// Number of valid entries in freq_table
    pub freq_table_len: u8,
    /// Current CPU load estimate 0–100
    pub cpu_load_pct: u8,
    /// Total number of frequency transitions (saturating)
    pub transitions: u64,
    /// True when this policy slot is in use
    pub active: bool,
}

impl CpufreqPolicy {
    /// Return a zeroed, inactive policy slot
    pub const fn empty() -> Self {
        CpufreqPolicy {
            cpu: 0,
            min_freq_khz: 0,
            max_freq_khz: 0,
            cur_freq_khz: 0,
            governor: GOV_ONDEMAND,
            target_freq_khz: 0,
            freq_table: [0u32; MAX_FREQ_TABLE_ENTRIES],
            freq_table_len: 0,
            cpu_load_pct: 0,
            transitions: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CPUFREQ_POLICIES: Mutex<[CpufreqPolicy; MAX_CPUFREQ_POLICIES]> =
    Mutex::new([CpufreqPolicy::empty(); MAX_CPUFREQ_POLICIES]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to MAX_FREQ_TABLE_ENTRIES frequencies from `src` into `dst`.
/// Returns the number of entries copied.
fn copy_freq_table(dst: &mut [u32; MAX_FREQ_TABLE_ENTRIES], src: &[u32]) -> u8 {
    let len = if src.len() < MAX_FREQ_TABLE_ENTRIES {
        src.len()
    } else {
        MAX_FREQ_TABLE_ENTRIES
    };
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len as u8
}

/// Clamp `freq_khz` to [min_freq_khz, max_freq_khz].
/// Returns min_freq_khz if max < min (degenerate range).
fn clamp_freq(freq_khz: u32, min_freq_khz: u32, max_freq_khz: u32) -> u32 {
    if max_freq_khz < min_freq_khz {
        return min_freq_khz;
    }
    if freq_khz < min_freq_khz {
        min_freq_khz
    } else if freq_khz > max_freq_khz {
        max_freq_khz
    } else {
        freq_khz
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new cpufreq policy for `cpu`.
///
/// Fills the frequency table with the seven standard P-states and sets the
/// initial frequency to `max_khz`. Returns the allocated policy index on
/// success, or `None` if the table is full.
pub fn cpufreq_register_policy(cpu: u32, min_khz: u32, max_khz: u32, governor: u8) -> Option<u32> {
    let mut policies = CPUFREQ_POLICIES.lock();
    for (i, p) in policies.iter_mut().enumerate() {
        if !p.active {
            *p = CpufreqPolicy::empty();
            p.cpu = cpu;
            p.min_freq_khz = min_khz;
            p.max_freq_khz = max_khz;
            p.cur_freq_khz = max_khz;
            p.target_freq_khz = max_khz;
            p.governor = governor;
            p.active = true;
            return Some(i as u32);
        }
    }
    None
}

/// Change the governor for an existing policy.
///
/// Returns `true` on success, `false` if `policy_idx` is out of range or
/// the policy slot is not active.
pub fn cpufreq_set_governor(policy_idx: u32, governor: u8) -> bool {
    if policy_idx as usize >= MAX_CPUFREQ_POLICIES {
        return false;
    }
    let mut policies = CPUFREQ_POLICIES.lock();
    let p = &mut policies[policy_idx as usize];
    if !p.active {
        return false;
    }
    p.governor = governor;
    true
}

/// Set the operating frequency for a policy.
///
/// The frequency is clamped to [min_freq_khz, max_freq_khz] and written to
/// both `cur_freq_khz` and `target_freq_khz`. `transitions` is incremented
/// (saturating) whenever the frequency actually changes.
///
/// Returns `true` on success.
pub fn cpufreq_set_freq(policy_idx: u32, freq_khz: u32) -> bool {
    if policy_idx as usize >= MAX_CPUFREQ_POLICIES {
        return false;
    }
    let mut policies = CPUFREQ_POLICIES.lock();
    let p = &mut policies[policy_idx as usize];
    if !p.active {
        return false;
    }
    let clamped = clamp_freq(freq_khz, p.min_freq_khz, p.max_freq_khz);
    if clamped != p.cur_freq_khz {
        p.transitions = p.transitions.saturating_add(1);
    }
    p.cur_freq_khz = clamped;
    p.target_freq_khz = clamped;
    true
}

/// Return the current operating frequency for a policy (kHz).
///
/// Returns `None` if `policy_idx` is out of range or inactive.
pub fn cpufreq_get_freq(policy_idx: u32) -> Option<u32> {
    if policy_idx as usize >= MAX_CPUFREQ_POLICIES {
        return None;
    }
    let policies = CPUFREQ_POLICIES.lock();
    let p = &policies[policy_idx as usize];
    if !p.active {
        return None;
    }
    Some(p.cur_freq_khz)
}

/// Update the CPU load estimate for a policy.
///
/// `load_pct` is clamped to 0–100.
pub fn cpufreq_update_load(policy_idx: u32, load_pct: u8) {
    if policy_idx as usize >= MAX_CPUFREQ_POLICIES {
        return;
    }
    let mut policies = CPUFREQ_POLICIES.lock();
    let p = &mut policies[policy_idx as usize];
    if !p.active {
        return;
    }
    p.cpu_load_pct = load_pct.min(100);
}

/// Apply governor logic for one policy and update `cur_freq_khz`.
///
/// Governor behaviours:
///   - `GOV_PERFORMANCE`  → always select max_freq_khz
///   - `GOV_POWERSAVE`    → always select min_freq_khz
///   - `GOV_ONDEMAND`     → jump to max if load > 80, min if load < 20,
///                          otherwise linear interpolation (integer only)
///   - `GOV_CONSERVATIVE` → same step-based logic (stub: same as ONDEMAND)
///   - `GOV_SCHEDUTIL`    → proportional to load (same formula as ONDEMAND)
///   - `GOV_USERSPACE`    → leave target unchanged (user controls it)
///
/// Linear interpolation formula (no floats):
///   target = min + (max - min) * load / 100
///   The divisor 100 is a compile-time constant and is never zero.
pub fn cpufreq_governor_tick(policy_idx: u32) {
    if policy_idx as usize >= MAX_CPUFREQ_POLICIES {
        return;
    }
    let mut policies = CPUFREQ_POLICIES.lock();
    let p = &mut policies[policy_idx as usize];
    if !p.active {
        return;
    }

    let min = p.min_freq_khz;
    let max = p.max_freq_khz;
    let load = p.cpu_load_pct as u32;

    let target: u32 = match p.governor {
        GOV_PERFORMANCE => max,

        GOV_POWERSAVE => min,

        GOV_ONDEMAND | GOV_CONSERVATIVE | GOV_SCHEDUTIL => {
            if load > 80 {
                max
            } else if load < 20 {
                min
            } else {
                // Linear interpolation: min + (max - min) * load / 100
                // 100 is a constant — never zero.
                let range = max.saturating_sub(min);
                min.saturating_add(range.saturating_mul(load) / 100)
            }
        }

        GOV_USERSPACE => {
            // User controls target; just clamp to valid range.
            clamp_freq(p.target_freq_khz, min, max)
        }

        _ => {
            // Unknown governor: fall back to performance
            max
        }
    };

    let clamped = clamp_freq(target, min, max);
    if clamped != p.cur_freq_khz {
        p.transitions = p.transitions.saturating_add(1);
    }
    p.cur_freq_khz = clamped;
    p.target_freq_khz = clamped;
}

/// Run `cpufreq_governor_tick` for all active policies.
pub fn cpufreq_tick_all() {
    for i in 0..MAX_CPUFREQ_POLICIES {
        // Check active flag under the lock, then call governor tick which
        // re-acquires the lock internally.  To avoid nested locking we
        // check active in a brief critical section first.
        let is_active = {
            let policies = CPUFREQ_POLICIES.lock();
            policies[i].active
        };
        if is_active {
            cpufreq_governor_tick(i as u32);
        }
    }
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Standard P-state frequency table for CPUs 0-3 (kHz).
const INIT_FREQ_TABLE: [u32; 7] = [
    800_000,   // 0.8 GHz
    1_200_000, // 1.2 GHz
    1_600_000, // 1.6 GHz
    2_000_000, // 2.0 GHz
    2_400_000, // 2.4 GHz
    3_200_000, // 3.2 GHz
    4_200_000, // 4.2 GHz
];

/// Initialize the cpufreq driver.
///
/// Registers policies for CPUs 0–3, each with the seven standard P-states
/// listed above and the Ondemand governor. Initial frequency is the maximum
/// (4.2 GHz).
pub fn init() {
    let mut policies = CPUFREQ_POLICIES.lock();

    for cpu in 0u32..4 {
        // Find a free slot
        let mut found = false;
        for p in policies.iter_mut() {
            if !p.active {
                *p = CpufreqPolicy::empty();
                p.cpu = cpu;
                p.min_freq_khz = INIT_FREQ_TABLE[0];
                p.max_freq_khz = INIT_FREQ_TABLE[6];
                p.cur_freq_khz = INIT_FREQ_TABLE[6];
                p.target_freq_khz = INIT_FREQ_TABLE[6];
                p.governor = GOV_ONDEMAND;
                p.freq_table_len = copy_freq_table(&mut p.freq_table, &INIT_FREQ_TABLE);
                p.active = true;
                found = true;
                break;
            }
        }
        if !found {
            serial_println!("  [cpufreq] WARNING: no free slot for CPU {}", cpu);
        }
    }

    drop(policies);

    super::register("cpufreq", super::DeviceType::Other);
    serial_println!("[cpufreq] CPU frequency scaling initialized");
}
