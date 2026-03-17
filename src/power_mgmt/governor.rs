/// CPU frequency governors (performance, powersave, schedutil).
///
/// Part of the AIOS power_mgmt subsystem.
/// Implements frequency governor policies that evaluate CPU load and decide
/// target frequencies. The active governor is consulted by the scheduler
/// on each tick to adjust CPU frequency for power/performance trade-offs.
use crate::sync::Mutex;

/// Available CPU frequency governor policies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GovernorPolicy {
    Performance,
    Powersave,
    Ondemand,
    Conservative,
    Schedutil,
}

/// Manages the active CPU frequency governor.
pub struct Governor {
    policy: GovernorPolicy,
    sampling_rate_ms: u32,
    up_threshold: u32,
    down_threshold: u32,
    min_freq_khz: u64,
    max_freq_khz: u64,
    current_target_khz: u64,
}

static GOVERNOR: Mutex<Option<Governor>> = Mutex::new(None);

/// Default thresholds for Ondemand governor
const DEFAULT_UP_THRESHOLD: u32 = 80;
const DEFAULT_DOWN_THRESHOLD: u32 = 20;
const DEFAULT_SAMPLING_MS: u32 = 50;

/// Default frequency range (will be updated from cpufreq driver)
const DEFAULT_MIN_KHZ: u64 = 800_000;
const DEFAULT_MAX_KHZ: u64 = 3_000_000;

impl Governor {
    pub fn new(policy: GovernorPolicy) -> Self {
        let (sampling, up_thr, down_thr) = match policy {
            GovernorPolicy::Performance => (100, 100, 100),
            GovernorPolicy::Powersave => (100, 100, 100),
            GovernorPolicy::Ondemand => (
                DEFAULT_SAMPLING_MS,
                DEFAULT_UP_THRESHOLD,
                DEFAULT_DOWN_THRESHOLD,
            ),
            GovernorPolicy::Conservative => (DEFAULT_SAMPLING_MS * 2, 75, 30),
            GovernorPolicy::Schedutil => (
                DEFAULT_SAMPLING_MS,
                DEFAULT_UP_THRESHOLD,
                DEFAULT_DOWN_THRESHOLD,
            ),
        };

        let target = match policy {
            GovernorPolicy::Performance => DEFAULT_MAX_KHZ,
            GovernorPolicy::Powersave => DEFAULT_MIN_KHZ,
            _ => DEFAULT_MIN_KHZ,
        };

        Governor {
            policy,
            sampling_rate_ms: sampling,
            up_threshold: up_thr,
            down_threshold: down_thr,
            min_freq_khz: DEFAULT_MIN_KHZ,
            max_freq_khz: DEFAULT_MAX_KHZ,
            current_target_khz: target,
        }
    }

    /// Evaluate current load and decide target frequency (in kHz).
    /// load_percent: CPU utilization percentage (0-100).
    pub fn evaluate(&self, load_percent: u32) -> u64 {
        match self.policy {
            GovernorPolicy::Performance => {
                // Always run at maximum
                self.max_freq_khz
            }
            GovernorPolicy::Powersave => {
                // Always run at minimum
                self.min_freq_khz
            }
            GovernorPolicy::Ondemand => {
                // Jump to max if above threshold, drop to proportional below
                if load_percent >= self.up_threshold {
                    self.max_freq_khz
                } else if load_percent <= self.down_threshold {
                    self.min_freq_khz
                } else {
                    // Scale linearly between min and max
                    let range = self.max_freq_khz - self.min_freq_khz;
                    self.min_freq_khz + (range * load_percent as u64) / 100
                }
            }
            GovernorPolicy::Conservative => {
                // Step frequency up/down gradually (5% steps)
                let step = (self.max_freq_khz - self.min_freq_khz) / 20;
                if load_percent >= self.up_threshold {
                    let new_freq = self.current_target_khz + step;
                    if new_freq > self.max_freq_khz {
                        self.max_freq_khz
                    } else {
                        new_freq
                    }
                } else if load_percent <= self.down_threshold {
                    if self.current_target_khz <= self.min_freq_khz + step {
                        self.min_freq_khz
                    } else {
                        self.current_target_khz - step
                    }
                } else {
                    self.current_target_khz
                }
            }
            GovernorPolicy::Schedutil => {
                // Proportional to utilization (schedutil-style)
                // target = max_freq * load / 100, clamped to [min, max]
                let target = (self.max_freq_khz * load_percent as u64) / 100;
                target.clamp(self.min_freq_khz, self.max_freq_khz)
            }
        }
    }

    /// Switch to a different governor policy.
    pub fn set_policy(&mut self, policy: GovernorPolicy) {
        let old = self.policy;
        self.policy = policy;

        // Reset target based on new policy
        match policy {
            GovernorPolicy::Performance => {
                self.current_target_khz = self.max_freq_khz;
            }
            GovernorPolicy::Powersave => {
                self.current_target_khz = self.min_freq_khz;
            }
            _ => {}
        }

        crate::serial_println!("  governor: policy changed {:?} -> {:?}", old, policy);
    }
}

pub fn init() {
    // Default to Ondemand governor (balanced power/performance)
    let gov = Governor::new(GovernorPolicy::Ondemand);
    crate::serial_println!("  governor: initialized with {:?} policy", gov.policy);
    *GOVERNOR.lock() = Some(gov);
}
