use crate::sync::Mutex;
/// Energy-aware scheduling model.
///
/// Part of the AIOS power_mgmt subsystem.
/// Models the energy cost of running workloads at different frequencies
/// across CPU clusters. Supports heterogeneous (big.LITTLE / hybrid) topologies
/// where cores have different performance and power characteristics.
use alloc::vec::Vec;

/// Energy cost at a given performance level.
pub struct EnergyLevel {
    pub frequency_khz: u64,
    pub capacity: u32,
    pub power_mw: u32,
}

/// A performance domain groups cores with identical frequency/voltage scaling.
struct PerfDomain {
    cpu_mask: u64, // bitmask of CPUs in this domain
    levels: Vec<EnergyLevel>,
}

/// Energy model for a CPU cluster (big.LITTLE / hybrid).
pub struct EnergyModel {
    domains: Vec<PerfDomain>,
}

static MODEL: Mutex<Option<EnergyModel>> = Mutex::new(None);

impl EnergyModel {
    pub fn new() -> Self {
        // Build a default single-domain model for homogeneous CPUs.
        // A heterogeneous platform would register multiple domains with
        // different capacity/power curves.
        let default_levels = Vec::from([
            EnergyLevel {
                frequency_khz: 800_000,
                capacity: 200,
                power_mw: 100,
            },
            EnergyLevel {
                frequency_khz: 1_200_000,
                capacity: 400,
                power_mw: 250,
            },
            EnergyLevel {
                frequency_khz: 1_800_000,
                capacity: 650,
                power_mw: 600,
            },
            EnergyLevel {
                frequency_khz: 2_400_000,
                capacity: 850,
                power_mw: 1100,
            },
            EnergyLevel {
                frequency_khz: 3_000_000,
                capacity: 1024,
                power_mw: 2000,
            },
        ]);

        let domain = PerfDomain {
            cpu_mask: 0xFF, // CPUs 0-7
            levels: default_levels,
        };

        EnergyModel {
            domains: Vec::from([domain]),
        }
    }

    /// Find the most energy-efficient CPU for a given task utilization.
    /// Returns the CPU index that can run the task at the lowest energy cost.
    /// Utilization is on a 0-1024 scale matching Linux's capacity units.
    pub fn find_efficient_cpu(&self, utilization: u32) -> u32 {
        let mut best_cpu: u32 = 0;
        let mut best_cost: u64 = u64::MAX;

        for domain in &self.domains {
            // Find the smallest capacity level that can handle the utilization
            let mut level_cost: Option<u32> = None;
            for level in &domain.levels {
                if level.capacity >= utilization {
                    // Energy cost = power * utilization / capacity
                    // (energy per unit of work done)
                    let cost = (level.power_mw as u64 * 1024) / (level.capacity as u64);
                    if cost < best_cost {
                        best_cost = cost;
                        level_cost = Some(level.power_mw);
                    }
                    break;
                }
            }

            // Pick first CPU in this domain
            if level_cost.is_some() {
                for bit in 0..64u32 {
                    if domain.cpu_mask & (1u64 << bit) != 0 {
                        best_cpu = bit;
                        break;
                    }
                }
            }
        }

        best_cpu
    }

    /// Get the energy cost (mW) of running at a given frequency on a given CPU.
    /// Returns the power draw at the matching frequency level, or 0 if not found.
    pub fn energy_cost(&self, cpu: u32, freq_khz: u64) -> u32 {
        for domain in &self.domains {
            if domain.cpu_mask & (1u64 << cpu) == 0 {
                continue;
            }
            // Find the closest frequency level
            let mut best_power: u32 = 0;
            let mut best_diff: u64 = u64::MAX;
            for level in &domain.levels {
                let diff = if freq_khz > level.frequency_khz {
                    freq_khz - level.frequency_khz
                } else {
                    level.frequency_khz - freq_khz
                };
                if diff < best_diff {
                    best_diff = diff;
                    best_power = level.power_mw;
                }
            }
            return best_power;
        }
        0
    }
}

pub fn init() {
    let model = EnergyModel::new();
    let num_domains = model.domains.len();
    let total_levels: usize = model.domains.iter().map(|d| d.levels.len()).sum();
    crate::serial_println!(
        "  energy_model: {} domain(s), {} OPP levels",
        num_domains,
        total_levels
    );
    *MODEL.lock() = Some(model);
}
