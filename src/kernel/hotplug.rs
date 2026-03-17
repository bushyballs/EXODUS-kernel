/// CPU and memory hotplug for Genesis
///
/// Manages dynamic addition and removal of CPUs and memory at runtime.
/// CPU hotplug allows onlining/offlining individual processor cores,
/// with proper task migration and interrupt re-routing. Memory hotplug
/// supports adding and removing physical memory regions (DIMM-granular).
///
/// The hotplug state machine ensures safe transitions: notifiers are
/// called at each step so subsystems (scheduler, interrupts, per-CPU
/// data) can prepare or clean up.
///
/// Inspired by: Linux CPU/memory hotplug (kernel/cpu.c, mm/memory_hotplug.c).
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

/// Maximum CPUs supported for hotplug
const MAX_CPUS: usize = 64;

/// Maximum memory regions for hotplug
const MAX_MEMORY_REGIONS: usize = 32;

/// Maximum hotplug notifiers
const MAX_NOTIFIERS: usize = 32;

/// Active CPU count (atomic for lockless reads)
static ONLINE_CPU_COUNT: AtomicU32 = AtomicU32::new(1);

/// CPU hotplug states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuState {
    /// CPU is not present in the system
    NotPresent,
    /// CPU is present but powered off
    Offline,
    /// CPU is coming online (in transition)
    BringUp,
    /// CPU is fully online and running tasks
    Online,
    /// CPU is going offline (in transition)
    TearDown,
    /// CPU is online but not accepting new tasks (draining)
    Draining,
    /// CPU is parked (halted but can be quickly resumed)
    Parked,
}

/// CPU hotplug info
#[derive(Clone)]
pub struct CpuHotplugInfo {
    /// CPU index
    pub cpu_id: u32,
    /// Current state
    pub state: CpuState,
    /// APIC ID
    pub apic_id: u32,
    /// Is this the bootstrap processor
    pub is_bsp: bool,
    /// Number of tasks currently on this CPU
    pub task_count: u32,
    /// Number of interrupts routed to this CPU
    pub irq_count: u32,
    /// Time this CPU came online (ms since boot)
    pub online_since_ms: u64,
    /// Number of times this CPU has been onlined
    pub online_count: u32,
    /// Number of times this CPU has been offlined
    pub offline_count: u32,
}

impl CpuHotplugInfo {
    fn new(cpu_id: u32) -> Self {
        CpuHotplugInfo {
            cpu_id,
            state: CpuState::NotPresent,
            apic_id: 0,
            is_bsp: false,
            task_count: 0,
            irq_count: 0,
            online_since_ms: 0,
            online_count: 0,
            offline_count: 0,
        }
    }
}

/// Memory hotplug states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryState {
    /// Region is not present
    NotPresent,
    /// Region is present but not usable (offline)
    Offline,
    /// Region is coming online
    GoingOnline,
    /// Region is fully online and allocatable
    Online,
    /// Region is going offline (pages being migrated)
    GoingOffline,
    /// Region is online but no new allocations (draining)
    Draining,
}

/// Memory hotplug region
#[derive(Clone)]
pub struct MemoryRegion {
    /// Region ID
    pub id: u32,
    /// Physical start address
    pub phys_start: u64,
    /// Size in bytes
    pub size: u64,
    /// Current state
    pub state: MemoryState,
    /// Number of pages in use
    pub pages_used: u64,
    /// Total pages in this region
    pub pages_total: u64,
    /// NUMA node this memory belongs to
    pub numa_node: u32,
    /// Whether this region can be removed (some regions are permanent)
    pub removable: bool,
    /// Description (e.g., "DIMM slot A1")
    pub description: String,
}

/// Hotplug event types for notifiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotplugEvent {
    /// CPU is about to come online
    CpuPreOnline(u32),
    /// CPU has come online
    CpuPostOnline(u32),
    /// CPU is about to go offline
    CpuPreOffline(u32),
    /// CPU has gone offline
    CpuPostOffline(u32),
    /// Memory region about to come online
    MemPreOnline(u32),
    /// Memory region has come online
    MemPostOnline(u32),
    /// Memory region about to go offline
    MemPreOffline(u32),
    /// Memory region has gone offline
    MemPostOffline(u32),
}

/// Notifier callback type
pub type HotplugNotifier = fn(event: HotplugEvent) -> bool;

/// A registered notifier
struct NotifierEntry {
    id: u32,
    name: String,
    callback: HotplugNotifier,
    priority: i32, // higher runs first
}

/// Hotplug subsystem state
struct HotplugSubsystem {
    /// Per-CPU hotplug state
    cpus: Vec<CpuHotplugInfo>,
    /// Memory hotplug regions
    memory_regions: Vec<MemoryRegion>,
    /// Registered notifiers
    notifiers: Vec<NotifierEntry>,
    /// Next notifier ID
    next_notifier_id: u32,
    /// Next memory region ID
    next_region_id: u32,
    /// Number of CPUs detected at boot
    boot_cpu_count: u32,
    /// Total physical memory at boot (bytes)
    boot_memory_bytes: u64,
    /// Whether CPU hotplug is enabled
    cpu_hotplug_enabled: bool,
    /// Whether memory hotplug is enabled
    mem_hotplug_enabled: bool,
}

impl HotplugSubsystem {
    const fn new() -> Self {
        HotplugSubsystem {
            cpus: Vec::new(),
            memory_regions: Vec::new(),
            notifiers: Vec::new(),
            next_notifier_id: 1,
            next_region_id: 1,
            boot_cpu_count: 1,
            boot_memory_bytes: 0,
            cpu_hotplug_enabled: true,
            mem_hotplug_enabled: true,
        }
    }

    /// Initialize CPU state from SMP data
    fn init_cpus(&mut self, num_cpus: u32) {
        self.boot_cpu_count = num_cpus;
        for i in 0..MAX_CPUS as u32 {
            let mut info = CpuHotplugInfo::new(i);
            if i < num_cpus {
                info.state = CpuState::Online;
                info.apic_id = i; // simplified
                if i == 0 {
                    info.is_bsp = true;
                }
                info.online_since_ms = 0; // online since boot
                info.online_count = 1;
            }
            self.cpus.push(info);
        }
        ONLINE_CPU_COUNT.store(num_cpus, Ordering::Release);
    }

    /// Fire notifiers for a hotplug event
    fn notify(&self, event: HotplugEvent) -> bool {
        // Sort by priority (higher first) - we work on a copy of IDs
        let mut ordered: Vec<usize> = (0..self.notifiers.len()).collect();
        ordered.sort_by(|a, b| {
            self.notifiers[*b]
                .priority
                .cmp(&self.notifiers[*a].priority)
        });

        for idx in ordered {
            let entry = &self.notifiers[idx];
            if !(entry.callback)(event) {
                serial_println!(
                    "  [hotplug] Notifier '{}' vetoed event {:?}",
                    entry.name,
                    event
                );
                return false;
            }
        }
        true
    }

    /// Bring a CPU online
    fn cpu_online(&mut self, cpu_id: u32) -> Result<(), HotplugError> {
        if !self.cpu_hotplug_enabled {
            return Err(HotplugError::Disabled);
        }
        if cpu_id as usize >= self.cpus.len() {
            return Err(HotplugError::InvalidCpu);
        }

        let cpu = &self.cpus[cpu_id as usize];
        if cpu.state == CpuState::Online {
            return Err(HotplugError::AlreadyOnline);
        }
        if cpu.is_bsp {
            return Ok(()); // BSP is always online
        }

        // Pre-online notification
        if !self.notify(HotplugEvent::CpuPreOnline(cpu_id)) {
            return Err(HotplugError::VetoedByNotifier);
        }

        // Transition: Offline -> BringUp -> Online
        self.cpus[cpu_id as usize].state = CpuState::BringUp;

        // Actually bring the CPU online via SMP
        crate::smp::cpu_set_online(cpu_id as usize);

        let now = crate::time::clock::uptime_ms();
        self.cpus[cpu_id as usize].state = CpuState::Online;
        self.cpus[cpu_id as usize].online_since_ms = now;
        self.cpus[cpu_id as usize].online_count =
            self.cpus[cpu_id as usize].online_count.saturating_add(1);

        let count = ONLINE_CPU_COUNT.fetch_add(1, Ordering::SeqCst) + 1;

        // Post-online notification
        self.notify(HotplugEvent::CpuPostOnline(cpu_id));

        serial_println!("  [hotplug] CPU {} online ({} CPUs total)", cpu_id, count);
        Ok(())
    }

    /// Take a CPU offline
    fn cpu_offline(&mut self, cpu_id: u32) -> Result<(), HotplugError> {
        if !self.cpu_hotplug_enabled {
            return Err(HotplugError::Disabled);
        }
        if cpu_id as usize >= self.cpus.len() {
            return Err(HotplugError::InvalidCpu);
        }

        let cpu = &self.cpus[cpu_id as usize];
        if cpu.state != CpuState::Online {
            return Err(HotplugError::NotOnline);
        }
        if cpu.is_bsp {
            return Err(HotplugError::CannotOfflineBSP);
        }

        // Check we're not offlining the last CPU
        let online = ONLINE_CPU_COUNT.load(Ordering::Relaxed);
        if online <= 1 {
            return Err(HotplugError::LastCpu);
        }

        // Pre-offline notification
        if !self.notify(HotplugEvent::CpuPreOffline(cpu_id)) {
            return Err(HotplugError::VetoedByNotifier);
        }

        // Phase 1: Drain - stop scheduling new tasks to this CPU
        self.cpus[cpu_id as usize].state = CpuState::Draining;

        // Migrate tasks from this CPU to others
        self.migrate_tasks_from_cpu(cpu_id);

        // Migrate interrupts from this CPU
        self.migrate_irqs_from_cpu(cpu_id);

        // Phase 2: Tear down
        self.cpus[cpu_id as usize].state = CpuState::TearDown;

        // Actually offline the CPU via SMP
        crate::smp::cpu_set_offline(cpu_id as usize);

        self.cpus[cpu_id as usize].state = CpuState::Offline;
        self.cpus[cpu_id as usize].offline_count =
            self.cpus[cpu_id as usize].offline_count.saturating_add(1);
        self.cpus[cpu_id as usize].task_count = 0;
        self.cpus[cpu_id as usize].irq_count = 0;

        let count = ONLINE_CPU_COUNT.fetch_sub(1, Ordering::SeqCst) - 1;

        // Post-offline notification
        self.notify(HotplugEvent::CpuPostOffline(cpu_id));

        serial_println!(
            "  [hotplug] CPU {} offline ({} CPUs remaining)",
            cpu_id,
            count
        );
        Ok(())
    }

    /// Park a CPU (lightweight offline - halted but easy to resume)
    fn cpu_park(&mut self, cpu_id: u32) -> Result<(), HotplugError> {
        if cpu_id as usize >= self.cpus.len() {
            return Err(HotplugError::InvalidCpu);
        }
        if self.cpus[cpu_id as usize].state != CpuState::Online {
            return Err(HotplugError::NotOnline);
        }
        if self.cpus[cpu_id as usize].is_bsp {
            return Err(HotplugError::CannotOfflineBSP);
        }

        self.cpus[cpu_id as usize].state = CpuState::Parked;
        serial_println!("  [hotplug] CPU {} parked", cpu_id);
        Ok(())
    }

    /// Unpark a CPU
    fn cpu_unpark(&mut self, cpu_id: u32) -> Result<(), HotplugError> {
        if cpu_id as usize >= self.cpus.len() {
            return Err(HotplugError::InvalidCpu);
        }
        if self.cpus[cpu_id as usize].state != CpuState::Parked {
            return Err(HotplugError::InvalidState);
        }

        self.cpus[cpu_id as usize].state = CpuState::Online;
        serial_println!("  [hotplug] CPU {} unparked", cpu_id);
        Ok(())
    }

    /// Migrate tasks from a CPU to other online CPUs (round-robin)
    fn migrate_tasks_from_cpu(&mut self, cpu_id: u32) {
        let task_count = self.cpus[cpu_id as usize].task_count;
        if task_count == 0 {
            return;
        }

        // Find online CPUs to distribute tasks to
        let online_cpus: Vec<u32> = self
            .cpus
            .iter()
            .filter(|c| c.state == CpuState::Online && c.cpu_id != cpu_id)
            .map(|c| c.cpu_id)
            .collect();

        if online_cpus.is_empty() {
            serial_println!("  [hotplug] WARNING: No CPUs available for task migration!");
            return;
        }

        // Distribute tasks round-robin
        let mut target_idx = 0;
        for _ in 0..task_count {
            let target = online_cpus[target_idx % online_cpus.len()];
            self.cpus[target as usize].task_count =
                self.cpus[target as usize].task_count.saturating_add(1);
            target_idx += 1;
        }

        serial_println!(
            "  [hotplug] Migrated {} tasks from CPU {} to {} CPUs",
            task_count,
            cpu_id,
            online_cpus.len(),
        );
    }

    /// Migrate interrupts from a CPU to other online CPUs
    fn migrate_irqs_from_cpu(&mut self, cpu_id: u32) {
        let irq_count = self.cpus[cpu_id as usize].irq_count;
        if irq_count == 0 {
            return;
        }

        // Find the online CPU with fewest IRQs
        let target = self
            .cpus
            .iter()
            .filter(|c| c.state == CpuState::Online && c.cpu_id != cpu_id)
            .min_by_key(|c| c.irq_count)
            .map(|c| c.cpu_id);

        if let Some(target_cpu) = target {
            self.cpus[target_cpu as usize].irq_count += irq_count;
            serial_println!(
                "  [hotplug] Migrated {} IRQs from CPU {} to CPU {}",
                irq_count,
                cpu_id,
                target_cpu,
            );
        }
    }

    /// Add a memory hotplug region
    fn add_memory_region(
        &mut self,
        phys_start: u64,
        size: u64,
        numa_node: u32,
        desc: &str,
    ) -> Result<u32, HotplugError> {
        if !self.mem_hotplug_enabled {
            return Err(HotplugError::Disabled);
        }
        if self.memory_regions.len() >= MAX_MEMORY_REGIONS {
            return Err(HotplugError::TooManyRegions);
        }

        // Check for overlap with existing regions
        for region in &self.memory_regions {
            let r_end = region.phys_start + region.size;
            let new_end = phys_start + size;
            if phys_start < r_end && new_end > region.phys_start {
                return Err(HotplugError::OverlappingRegion);
            }
        }

        let id = self.next_region_id;
        self.next_region_id = self.next_region_id.saturating_add(1);

        let page_size: u64 = 4096;
        let pages_total = size / page_size;

        let region = MemoryRegion {
            id,
            phys_start,
            size,
            state: MemoryState::Offline,
            pages_used: 0,
            pages_total,
            numa_node,
            removable: true,
            description: String::from(desc),
        };

        self.memory_regions.push(region);
        serial_println!(
            "  [hotplug] Memory region added: {:#X}-{:#X} ({} MB, NUMA {})",
            phys_start,
            phys_start + size,
            size / (1024 * 1024),
            numa_node,
        );
        Ok(id)
    }

    /// Bring a memory region online
    fn memory_online(&mut self, region_id: u32) -> Result<(), HotplugError> {
        let region = self
            .memory_regions
            .iter_mut()
            .find(|r| r.id == region_id)
            .ok_or(HotplugError::RegionNotFound)?;

        if region.state == MemoryState::Online {
            return Err(HotplugError::AlreadyOnline);
        }

        if !self.notify(HotplugEvent::MemPreOnline(region_id)) {
            return Err(HotplugError::VetoedByNotifier);
        }

        let region = self
            .memory_regions
            .iter_mut()
            .find(|r| r.id == region_id)
            .unwrap();
        region.state = MemoryState::GoingOnline;

        // In a real implementation: add pages to the frame allocator
        let region = self
            .memory_regions
            .iter_mut()
            .find(|r| r.id == region_id)
            .unwrap();
        region.state = MemoryState::Online;

        self.notify(HotplugEvent::MemPostOnline(region_id));

        let region = self
            .memory_regions
            .iter()
            .find(|r| r.id == region_id)
            .unwrap();
        serial_println!(
            "  [hotplug] Memory region {} online ({} MB)",
            region_id,
            region.size / (1024 * 1024),
        );
        Ok(())
    }

    /// Take a memory region offline
    fn memory_offline(&mut self, region_id: u32) -> Result<(), HotplugError> {
        let region = self
            .memory_regions
            .iter_mut()
            .find(|r| r.id == region_id)
            .ok_or(HotplugError::RegionNotFound)?;

        if region.state != MemoryState::Online {
            return Err(HotplugError::NotOnline);
        }
        if !region.removable {
            return Err(HotplugError::NotRemovable);
        }

        if !self.notify(HotplugEvent::MemPreOffline(region_id)) {
            return Err(HotplugError::VetoedByNotifier);
        }

        let region = self
            .memory_regions
            .iter_mut()
            .find(|r| r.id == region_id)
            .unwrap();
        region.state = MemoryState::Draining;

        // Migrate pages away from this region
        // In a real implementation: iterate all pages, migrate/reclaim/swap
        let pages_to_migrate = region.pages_used;
        if pages_to_migrate > 0 {
            serial_println!(
                "  [hotplug] Migrating {} pages from memory region {}",
                pages_to_migrate,
                region_id,
            );
        }

        let region = self
            .memory_regions
            .iter_mut()
            .find(|r| r.id == region_id)
            .unwrap();
        region.state = MemoryState::GoingOffline;
        region.pages_used = 0;
        region.state = MemoryState::Offline;

        self.notify(HotplugEvent::MemPostOffline(region_id));

        serial_println!("  [hotplug] Memory region {} offline", region_id);
        Ok(())
    }

    /// Remove a memory region entirely
    fn remove_memory_region(&mut self, region_id: u32) -> Result<(), HotplugError> {
        let idx = self
            .memory_regions
            .iter()
            .position(|r| r.id == region_id)
            .ok_or(HotplugError::RegionNotFound)?;

        if self.memory_regions[idx].state == MemoryState::Online {
            self.memory_offline(region_id)?;
        }

        self.memory_regions.remove(idx);
        serial_println!("  [hotplug] Memory region {} removed", region_id);
        Ok(())
    }

    /// Register a hotplug notifier
    fn register_notifier(
        &mut self,
        name: &str,
        callback: HotplugNotifier,
        priority: i32,
    ) -> Result<u32, HotplugError> {
        if self.notifiers.len() >= MAX_NOTIFIERS {
            return Err(HotplugError::TooManyNotifiers);
        }

        let id = self.next_notifier_id;
        self.next_notifier_id = self.next_notifier_id.saturating_add(1);

        self.notifiers.push(NotifierEntry {
            id,
            name: String::from(name),
            callback,
            priority,
        });

        Ok(id)
    }

    /// Unregister a notifier
    fn unregister_notifier(&mut self, notifier_id: u32) -> bool {
        if let Some(idx) = self.notifiers.iter().position(|n| n.id == notifier_id) {
            self.notifiers.remove(idx);
            true
        } else {
            false
        }
    }

    /// Get CPU states
    fn cpu_states(&self) -> Vec<(u32, CpuState, bool)> {
        self.cpus
            .iter()
            .filter(|c| c.state != CpuState::NotPresent)
            .map(|c| (c.cpu_id, c.state, c.is_bsp))
            .collect()
    }

    /// Get memory region states
    fn memory_states(&self) -> Vec<(u32, u64, u64, MemoryState)> {
        self.memory_regions
            .iter()
            .map(|r| (r.id, r.phys_start, r.size, r.state))
            .collect()
    }

    /// Get status report
    fn status(&self) -> String {
        let online_cpus = self
            .cpus
            .iter()
            .filter(|c| c.state == CpuState::Online)
            .count();
        let present_cpus = self
            .cpus
            .iter()
            .filter(|c| c.state != CpuState::NotPresent)
            .count();
        let online_mem = self
            .memory_regions
            .iter()
            .filter(|r| r.state == MemoryState::Online)
            .count();
        let total_online_bytes: u64 = self
            .memory_regions
            .iter()
            .filter(|r| r.state == MemoryState::Online)
            .map(|r| r.size)
            .sum();

        format!(
            "CPU Hotplug: {}\n\
             Memory Hotplug: {}\n\
             CPUs: {}/{} online\n\
             Memory regions: {}/{} online ({} MB)\n\
             Notifiers: {}\n",
            if self.cpu_hotplug_enabled {
                "ENABLED"
            } else {
                "DISABLED"
            },
            if self.mem_hotplug_enabled {
                "ENABLED"
            } else {
                "DISABLED"
            },
            online_cpus,
            present_cpus,
            online_mem,
            self.memory_regions.len(),
            total_online_bytes / (1024 * 1024),
            self.notifiers.len(),
        )
    }
}

/// Hotplug errors
#[derive(Debug)]
pub enum HotplugError {
    Disabled,
    InvalidCpu,
    AlreadyOnline,
    NotOnline,
    CannotOfflineBSP,
    LastCpu,
    VetoedByNotifier,
    InvalidState,
    TooManyRegions,
    OverlappingRegion,
    RegionNotFound,
    NotRemovable,
    TooManyNotifiers,
}

/// Global hotplug subsystem
static HOTPLUG: Mutex<HotplugSubsystem> = Mutex::new(HotplugSubsystem::new());

/// Bring a CPU online
pub fn cpu_online(cpu_id: u32) -> Result<(), HotplugError> {
    HOTPLUG.lock().cpu_online(cpu_id)
}

/// Take a CPU offline
pub fn cpu_offline(cpu_id: u32) -> Result<(), HotplugError> {
    HOTPLUG.lock().cpu_offline(cpu_id)
}

/// Park a CPU
pub fn cpu_park(cpu_id: u32) -> Result<(), HotplugError> {
    HOTPLUG.lock().cpu_park(cpu_id)
}

/// Unpark a CPU
pub fn cpu_unpark(cpu_id: u32) -> Result<(), HotplugError> {
    HOTPLUG.lock().cpu_unpark(cpu_id)
}

/// Add a memory hotplug region
pub fn add_memory_region(
    phys_start: u64,
    size: u64,
    numa_node: u32,
    desc: &str,
) -> Result<u32, HotplugError> {
    HOTPLUG
        .lock()
        .add_memory_region(phys_start, size, numa_node, desc)
}

/// Bring a memory region online
pub fn memory_online(region_id: u32) -> Result<(), HotplugError> {
    HOTPLUG.lock().memory_online(region_id)
}

/// Take a memory region offline
pub fn memory_offline(region_id: u32) -> Result<(), HotplugError> {
    HOTPLUG.lock().memory_offline(region_id)
}

/// Remove a memory region
pub fn remove_memory_region(region_id: u32) -> Result<(), HotplugError> {
    HOTPLUG.lock().remove_memory_region(region_id)
}

/// Register a hotplug notifier
pub fn register_notifier(
    name: &str,
    callback: HotplugNotifier,
    priority: i32,
) -> Result<u32, HotplugError> {
    HOTPLUG.lock().register_notifier(name, callback, priority)
}

/// Unregister a notifier
pub fn unregister_notifier(notifier_id: u32) -> bool {
    HOTPLUG.lock().unregister_notifier(notifier_id)
}

/// Get online CPU count (lockless)
pub fn online_cpu_count() -> u32 {
    ONLINE_CPU_COUNT.load(Ordering::Relaxed)
}

/// Get CPU states
pub fn cpu_states() -> Vec<(u32, CpuState, bool)> {
    HOTPLUG.lock().cpu_states()
}

/// Get memory region states
pub fn memory_states() -> Vec<(u32, u64, u64, MemoryState)> {
    HOTPLUG.lock().memory_states()
}

/// Get hotplug status report
pub fn status() -> String {
    HOTPLUG.lock().status()
}

/// Enable/disable CPU hotplug
pub fn set_cpu_hotplug_enabled(enabled: bool) {
    HOTPLUG.lock().cpu_hotplug_enabled = enabled;
}

/// Enable/disable memory hotplug
pub fn set_mem_hotplug_enabled(enabled: bool) {
    HOTPLUG.lock().mem_hotplug_enabled = enabled;
}

pub fn init() {
    let num_cpus = crate::smp::num_cpus();
    let num_cpus = if num_cpus == 0 { 1 } else { num_cpus };

    let mut hp = HOTPLUG.lock();
    hp.init_cpus(num_cpus);

    serial_println!(
        "  [hotplug] CPU/memory hotplug initialized ({} CPUs online, hotplug ready)",
        num_cpus,
    );
}
