/// Hardware Optimization Engine for Genesis
///
/// Provides low-level hardware tuning subsystems:
///   - CPU frequency scaling, core parking, turbo boost, C-states
///   - Memory compaction, deduplication, huge pages, NUMA, balloon
///   - I/O scheduling (CFQ, deadline, BFQ, priorities, merging)
///   - Thermal management (temperature monitoring, throttle, fan)
///   - DMA engine (scatter-gather, ring buffers, IOMMU, channels)
///
/// All code is original. Built from scratch for Hoags Inc.

use crate::{serial_print, serial_println};

pub mod cpu_tune;
pub mod memory_opt;
pub mod io_scheduler;
pub mod thermal;
pub mod dma;

pub fn init() {
    cpu_tune::init();
    memory_opt::init();
    io_scheduler::init();
    thermal::init();
    dma::init();
    serial_println!("  Optimization: CPU tuning, memory opt, I/O scheduler, thermal, DMA engine");
}
