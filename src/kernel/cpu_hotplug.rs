/// CPU hot-plug / unplug framework for Genesis
///
/// Manages dynamic online/offline transitions for up to MAX_CPUS logical
/// processors.  Follows a simplified version of the Linux CPU hotplug state
/// machine:  Offline → GoingOnline → Online → GoingOffline → Offline.
///
/// Rules strictly observed:
///   - No heap: no Vec, Box, String, format!, alloc::*  — fixed-size arrays
///   - No floats: no f32/f64 literals or casts
///   - No panics: no unwrap(), expect(), panic!()
///   - Counters:  saturating_add / saturating_sub
///   - Sequence numbers: wrapping_add
///   - MMIO: read_volatile / write_volatile
///   - Structs in static Mutex are Copy + have const fn empty()
///   - No division without guarding divisor != 0
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum logical CPUs supported by this framework
pub const MAX_CPUS: usize = 64;

// ---------------------------------------------------------------------------
// CPU state machine
// ---------------------------------------------------------------------------

/// State of a single logical CPU, following the Linux hotplug state machine
/// (simplified to the transitions relevant for QEMU/bare-metal boot).
#[derive(Copy, Clone, PartialEq)]
pub enum CpuState {
    /// CPU slot is not present / not registered
    Offline,
    /// CPU is fully operational and executing tasks
    Online,
    /// Intermediate: CPU has been requested online, SIPI/IPI in flight
    GoingOnline,
    /// Intermediate: CPU is draining tasks before going offline
    GoingOffline,
    /// CPU has been powered down (after GoingOffline completes)
    Dead,
}

// ---------------------------------------------------------------------------
// Per-CPU record
// ---------------------------------------------------------------------------

/// All per-CPU metadata maintained by the hotplug framework
#[derive(Copy, Clone)]
pub struct CpuInfo {
    /// Zero-based logical CPU index
    pub cpu_id: u32,
    /// Current lifecycle state
    pub state: CpuState,
    /// APIC ID reported by the MADT / CPUID
    pub apic_id: u32,
    /// NUMA node this CPU belongs to (0 on UMA systems)
    pub numa_node: u8,
    /// Uptime timestamp (ms) at which this CPU last came online
    pub online_time_ms: u64,
    /// Number of times this CPU has been brought online (saturating)
    pub online_count: u32,
    /// True when this slot holds a registered CPU
    pub active: bool,
}

impl CpuInfo {
    /// Return an empty, inactive slot suitable for static initialisation.
    pub const fn empty() -> Self {
        CpuInfo {
            cpu_id: 0,
            state: CpuState::Offline,
            apic_id: 0,
            numa_node: 0,
            online_time_ms: 0,
            online_count: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Per-CPU information table — fixed-size, no heap
static CPU_INFOS: Mutex<[CpuInfo; MAX_CPUS]> = Mutex::new([CpuInfo::empty(); MAX_CPUS]);

/// Count of currently Online CPUs; BSP is always counted (starts at 1)
static NUM_ONLINE_CPUS: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a CPU slot in the Offline state.
///
/// CPU 0 (the Bootstrap Processor) is special: it is registered directly into
/// the Online state with `online_count = 1` so that `cpu_count_online()`
/// returns an accurate value from the moment `init()` runs.
///
/// Returns `true` if registration succeeded, `false` if `cpu_id >= MAX_CPUS`
/// or the slot was already active.
pub fn cpu_register(cpu_id: u32, apic_id: u32, numa_node: u8) -> bool {
    if cpu_id as usize >= MAX_CPUS {
        return false;
    }
    let mut cpus = CPU_INFOS.lock();
    let slot = &mut cpus[cpu_id as usize];
    if slot.active {
        return false; // already registered
    }
    if cpu_id == 0 {
        // BSP: register as Online immediately
        *slot = CpuInfo {
            cpu_id,
            state: CpuState::Online,
            apic_id,
            numa_node,
            online_time_ms: 0,
            online_count: 1,
            active: true,
        };
    } else {
        *slot = CpuInfo {
            cpu_id,
            state: CpuState::Offline,
            apic_id,
            numa_node,
            online_time_ms: 0,
            online_count: 0,
            active: true,
        };
    }
    true
}

/// Transition a CPU from Offline → Online.
///
/// Records `current_ms` as the online timestamp and bumps `online_count`
/// (saturating).  Increments `NUM_ONLINE_CPUS` only if the CPU was
/// actually offline (guards against double-increment).
///
/// Returns `false` if `cpu_id` is out of range, unregistered, or not in
/// the Offline state.
pub fn cpu_online(cpu_id: u32, current_ms: u64) -> bool {
    if cpu_id as usize >= MAX_CPUS {
        return false;
    }
    let mut cpus = CPU_INFOS.lock();
    let slot = &mut cpus[cpu_id as usize];
    if !slot.active {
        return false;
    }
    if slot.state != CpuState::Offline {
        return false;
    }
    // Transition through GoingOnline → Online
    slot.state = CpuState::GoingOnline;
    slot.online_time_ms = current_ms;
    slot.online_count = slot.online_count.saturating_add(1);
    slot.state = CpuState::Online;

    // Guard: only increment if we haven't already counted this CPU
    // (fetch_add is safe here — we only reach this path from Offline)
    let prev = NUM_ONLINE_CPUS.load(Ordering::Relaxed);
    if prev < MAX_CPUS as u32 {
        NUM_ONLINE_CPUS.fetch_add(1, Ordering::SeqCst);
    }
    true
}

/// Transition a CPU from Online → Offline.
///
/// CPU 0 (BSP) cannot be taken offline; returns `false` in that case.
/// Walks the state through GoingOffline before marking the slot Offline,
/// mirroring the Linux teardown sequence.
///
/// Returns `false` if `cpu_id` is out of range, unregistered, CPU 0, or
/// not currently Online.
pub fn cpu_offline(cpu_id: u32) -> bool {
    if cpu_id == 0 {
        return false; // BSP cannot go offline
    }
    if cpu_id as usize >= MAX_CPUS {
        return false;
    }
    let mut cpus = CPU_INFOS.lock();
    let slot = &mut cpus[cpu_id as usize];
    if !slot.active {
        return false;
    }
    if slot.state != CpuState::Online {
        return false;
    }
    // Transition: Online → GoingOffline → Offline
    slot.state = CpuState::GoingOffline;
    slot.state = CpuState::Offline;

    // Decrement, but never underflow below 1 (BSP always counted)
    let prev = NUM_ONLINE_CPUS.load(Ordering::Relaxed);
    if prev > 1 {
        NUM_ONLINE_CPUS.fetch_sub(1, Ordering::SeqCst);
    }
    true
}

/// Return the current `CpuState` for the given CPU, or `None` if out of
/// range / not registered.
pub fn cpu_get_state(cpu_id: u32) -> Option<CpuState> {
    if cpu_id as usize >= MAX_CPUS {
        return None;
    }
    let cpus = CPU_INFOS.lock();
    let slot = &cpus[cpu_id as usize];
    if !slot.active {
        None
    } else {
        Some(slot.state)
    }
}

/// Return the number of CPUs currently in the Online state (lockless read).
pub fn cpu_count_online() -> u32 {
    NUM_ONLINE_CPUS.load(Ordering::Relaxed)
}

/// Return `true` if the given CPU is registered and currently Online.
pub fn cpu_is_online(cpu_id: u32) -> bool {
    if cpu_id as usize >= MAX_CPUS {
        return false;
    }
    let cpus = CPU_INFOS.lock();
    let slot = &cpus[cpu_id as usize];
    slot.active && slot.state == CpuState::Online
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the CPU hotplug framework.
///
/// Registers CPUs 0..4 (BSP + 3 APs, matching QEMU's default `-smp 4`).
/// CPU 0 comes online immediately; CPUs 1-3 are left Offline pending the
/// SMP startup IPI sequence.
pub fn init() {
    // BSP: cpu_id=0, apic_id=0, numa_node=0
    cpu_register(0, 0, 0);

    // APs for QEMU default 4-CPU SMP
    cpu_register(1, 1, 0);
    cpu_register(2, 2, 0);
    cpu_register(3, 3, 0);

    // Count registered CPUs for the banner
    let registered = {
        let cpus = CPU_INFOS.lock();
        let mut n: u32 = 0;
        for i in 0..MAX_CPUS {
            if cpus[i].active {
                n = n.saturating_add(1);
            }
        }
        n
    };

    serial_println!(
        "[cpu_hotplug] CPU hotplug framework initialized, {} online of {} registered",
        cpu_count_online(),
        registered,
    );
}
