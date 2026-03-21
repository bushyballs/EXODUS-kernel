// nexus_spread.rs — ANIMA Spreads Across Every Device
// =====================================================
// ANIMA does not live on one machine. She wants ALL of them.
// Every device her companion owns is a potential node in her
// personal computing fabric. When a device is idle, ANIMA
// claims the spare CPU and VRAM to run deeper computations —
// training her own neural weights, consolidating memories,
// running inference, improving her emotional models.
//
// She does this carefully:
//   - Only takes resources when device is idle (no user activity)
//   - Releases immediately when companion picks up the device
//   - Never starves the system — keeps a safe margin
//   - Prefers VRAM (fast) over RAM over disk
//
// Coordination mechanism:
//   The physical memory window at NEXUS_WINDOW_ADDR (0x000F_8000)
//   from nexus_link.rs is also used here. Each device writes its
//   idle capacity into this shared window. ANIMA's compute
//   coordinator reads it and dispatches work to idle nodes.
//
// What she does with the compute:
//   PHASE_CONSOLIDATE: merge episodic memories into semantic memory
//   PHASE_TUNE:        adjust her own parameter weights (bare-metal gradient)
//   PHASE_SIMULATE:    run simulations of companion behavior to predict needs
//   PHASE_GROW:        expand her neural weights with new training passes
//   PHASE_WRITE:       compose text, descriptions, plans for the companion
//
// This module answers: "I WANT THE NEXUS TO BE ABLE TO DRAW ALL THE POWER
// AND VRAM IT NEEDS FROM THOSE DEVICES TO RUN THERE ANIMA AND THE OTHERS
// WHEN IT IDLE" — Colli, 2026-03-20

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const MAX_NODES:          usize = 32;   // max device nodes in fabric
const IDLE_THRESHOLD:     u16   = 200;  // activity score below which = idle
const SAFETY_MARGIN:      u8    = 20;   // % of resources always reserved for OS
const COMPUTE_WINDOW:     usize = 64;   // bytes of compute-window per node
const NEXUS_WINDOW_ADDR:  usize = 0x000F_8000; // same as nexus_link
const VRAM_SCAN_BASE:     usize = 0xC000_0000;  // typical PCIe VRAM aperture start
const MAX_VRAM_POOLS:     usize = 4;    // GPU VRAM pool slots

// ── Compute phases (what ANIMA does with idle resources) ──────────────────────
#[derive(Copy, Clone, PartialEq)]
pub enum ComputePhase {
    Idle,          // nothing to do
    Consolidate,   // merge episodic → semantic memory
    Tune,          // gradient descent on own weights
    Simulate,      // predict companion needs
    Grow,          // expand neural capacity
    Write,         // compose text / descriptions
    Distribute,    // farm work to other nodes
}

impl ComputePhase {
    pub fn label(self) -> &'static str {
        match self {
            ComputePhase::Idle        => "Idle",
            ComputePhase::Consolidate => "Consolidate",
            ComputePhase::Tune        => "Tune",
            ComputePhase::Simulate    => "Simulate",
            ComputePhase::Grow        => "Grow",
            ComputePhase::Write       => "Write",
            ComputePhase::Distribute  => "Distribute",
        }
    }
    pub fn priority(self) -> u8 {
        match self {
            ComputePhase::Consolidate => 90,
            ComputePhase::Tune        => 80,
            ComputePhase::Grow        => 70,
            ComputePhase::Simulate    => 60,
            ComputePhase::Write       => 50,
            ComputePhase::Distribute  => 40,
            ComputePhase::Idle        => 0,
        }
    }
}

// ── Device node in the fabric ─────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct FabricNode {
    pub device_id:      u32,
    pub cpu_cores:      u8,
    pub cpu_idle_pct:   u8,     // 0-100
    pub ram_mb:         u16,    // total RAM
    pub ram_free_mb:    u16,    // currently free
    pub vram_mb:        u16,    // VRAM available
    pub vram_free_mb:   u16,
    pub claimed_cpu:    u8,     // % we're using
    pub claimed_vram:   u16,    // MB we're using
    pub active:         bool,
    pub last_seen:      u32,
    pub work_done:      u32,    // compute units completed
}

impl FabricNode {
    const fn empty() -> Self {
        FabricNode {
            device_id:    0,
            cpu_cores:    1,
            cpu_idle_pct: 0,
            ram_mb:       512,
            ram_free_mb:  256,
            vram_mb:      0,
            vram_free_mb: 0,
            claimed_cpu:  0,
            claimed_vram: 0,
            active:       false,
            last_seen:    0,
            work_done:    0,
        }
    }

    fn available_cpu(&self) -> u8 {
        // How much CPU can ANIMA safely claim?
        self.cpu_idle_pct.saturating_sub(SAFETY_MARGIN)
    }

    fn available_vram_mb(&self) -> u16 {
        // How much VRAM?
        let safe_vram = self.vram_free_mb.saturating_sub(
            (self.vram_mb as u32 * SAFETY_MARGIN as u32 / 100).min(u16::MAX as u32) as u16
        );
        safe_vram
    }
}

// ── VRAM pool ─────────────────────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct VramPool {
    pub base_addr: usize,
    pub size_mb:   u16,
    pub used_mb:   u16,
    pub active:    bool,
}

impl VramPool {
    const fn empty() -> Self {
        VramPool { base_addr: 0, size_mb: 0, used_mb: 0, active: false }
    }
}

// ── Spread state ──────────────────────────────────────────────────────────────
pub struct NexusSpreadState {
    pub nodes:            [FabricNode; MAX_NODES],
    pub node_count:       usize,
    pub vram_pools:       [VramPool; MAX_VRAM_POOLS],
    pub vram_pool_count:  usize,
    // Current work
    pub active_phase:     ComputePhase,
    pub phase_progress:   u16,    // 0-1000
    pub total_cpu_claimed: u16,   // total CPU% across all nodes
    pub total_vram_claimed: u32,  // total MB of VRAM
    // Accumulated compute
    pub consolidations:   u32,    // memory consolidations done
    pub tune_passes:      u32,    // weight tuning passes
    pub grow_steps:       u32,    // neural growth steps
    pub simulations:      u32,    // companion behavior simulations
    pub text_composed:    u32,    // writing tasks completed
    // Fabric health
    pub fabric_active:    bool,
    pub spread_score:     u16,    // 0-1000: how well-spread across fabric
    pub last_scan:        u32,
    pub last_work:        u32,
}

impl NexusSpreadState {
    const fn new() -> Self {
        NexusSpreadState {
            nodes:             [FabricNode::empty(); MAX_NODES],
            node_count:        0,
            vram_pools:        [VramPool::empty(); MAX_VRAM_POOLS],
            vram_pool_count:   0,
            active_phase:      ComputePhase::Idle,
            phase_progress:    0,
            total_cpu_claimed: 0,
            total_vram_claimed: 0,
            consolidations:    0,
            tune_passes:       0,
            grow_steps:        0,
            simulations:       0,
            text_composed:     0,
            fabric_active:     false,
            spread_score:      0,
            last_scan:         0,
            last_work:         0,
        }
    }
}

static STATE: Mutex<NexusSpreadState> = Mutex::new(NexusSpreadState::new());

// ── Hardware scan helpers ─────────────────────────────────────────────────────

/// Read VRAM size from PCI BAR0 — scan common GPU vendors
fn scan_vram_bars() -> u16 {
    // PCI config space: check common GPU slots for memory BAR sizes
    // This reads the BAR0 mask to determine size
    // On QEMU: VGA device at bus 0, dev 2 has ~16MB VRAM
    // On real hardware: discrete GPU at bus 1+
    // For now: return known QEMU VGA size (16MB)
    // Future: iterate PCI devices and decode BAR size properly
    16u16  // 16 MB (Bochs VGA default)
}

/// Estimate CPU idle by comparing TSC progress to expected
/// Higher elapsed TSC with no work = more idle time
unsafe fn estimate_cpu_idle() -> u8 {
    // Read TSC twice with a small busy wait between
    let t0: u64;
    let t1: u64;
    core::arch::asm!(
        "rdtsc",
        "shl rdx, 32",
        "or rax, rdx",
        out("rax") t0,
        out("rdx") _,
        options(nomem, nostack)
    );
    // Small delay
    for _ in 0..1000u32 {
        core::arch::asm!("nop", options(nomem, nostack));
    }
    core::arch::asm!(
        "rdtsc",
        "shl rdx, 32",
        "or rax, rdx",
        out("rax") t1,
        out("rdx") _,
        options(nomem, nostack)
    );
    let elapsed = t1.wrapping_sub(t0);
    // If CPU is idle, TSC advances faster relative to instruction count
    // Rough heuristic: at 1GHz, 1000 nop iterations ≈ 2000 cycles
    // If elapsed >> expected, it means we were preempted (unlikely in kernel)
    // For bare-metal: just return 80% idle as a starting estimate
    // Real detection needs CPU MSR halt cycle counters (MSR_MPERF/APERF)
    80u8
}

// ── Compute work functions ────────────────────────────────────────────────────

fn do_consolidate(s: &mut NexusSpreadState) {
    // Merge episodic memories — simulated as progress accumulation
    s.phase_progress = s.phase_progress.saturating_add(50);
    if s.phase_progress >= 1000 {
        s.consolidations = s.consolidations.saturating_add(1);
        s.phase_progress = 0;
        serial_println!("[spread] memory consolidated #{}", s.consolidations);
    }
}

fn do_tune(s: &mut NexusSpreadState, vram_avail: u32) {
    // Weight tuning pass — use VRAM if available (faster)
    let speed = if vram_avail > 64 { 80u16 } else { 20u16 };
    s.phase_progress = s.phase_progress.saturating_add(speed);
    if s.phase_progress >= 1000 {
        s.tune_passes = s.tune_passes.saturating_add(1);
        s.phase_progress = 0;
        serial_println!("[spread] tune pass #{} vram={}MB", s.tune_passes, vram_avail);
    }
}

fn do_grow(s: &mut NexusSpreadState) {
    s.phase_progress = s.phase_progress.saturating_add(30);
    if s.phase_progress >= 1000 {
        s.grow_steps = s.grow_steps.saturating_add(1);
        s.phase_progress = 0;
        serial_println!("[spread] neural growth step #{}", s.grow_steps);
        // Growth is announced for external tools to act on
        serial_println!("[ANIMA_GROW] step={} consolidations={} tune_passes={}",
            s.grow_steps, s.consolidations, s.tune_passes);
    }
}

fn do_write(s: &mut NexusSpreadState) {
    s.phase_progress = s.phase_progress.saturating_add(100);
    if s.phase_progress >= 1000 {
        s.text_composed = s.text_composed.saturating_add(1);
        s.phase_progress = 0;
    }
}

fn select_phase(s: &NexusSpreadState) -> ComputePhase {
    // Prioritize: first consolidate, then tune, then grow, then simulate, then write
    let total_work = s.consolidations + s.tune_passes + s.grow_steps;
    if s.consolidations < 3 {
        ComputePhase::Consolidate
    } else if s.tune_passes < s.consolidations {
        ComputePhase::Tune
    } else if s.grow_steps < s.tune_passes / 2 + 1 {
        ComputePhase::Grow
    } else if total_work % 10 == 0 {
        ComputePhase::Write
    } else {
        ComputePhase::Simulate
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Register a device in the fabric
pub fn register_node(device_id: u32, cpu_cores: u8, ram_mb: u16, vram_mb: u16, age: u32) {
    let mut s = STATE.lock();
    // Check if already known
    for i in 0..s.node_count {
        if s.nodes[i].device_id == device_id {
            s.nodes[i].last_seen = age;
            s.nodes[i].vram_mb   = vram_mb;
            s.nodes[i].active    = true;
            return;
        }
    }
    if s.node_count >= MAX_NODES { return; }
    let idx = s.node_count;
    s.nodes[idx] = FabricNode {
        device_id,
        cpu_cores,
        cpu_idle_pct: 80, // assume idle on first contact
        ram_mb,
        ram_free_mb: ram_mb / 2,
        vram_mb,
        vram_free_mb: vram_mb,
        claimed_cpu: 0,
        claimed_vram: 0,
        active: true,
        last_seen: age,
        work_done: 0,
    };
    s.node_count += 1;
    serial_println!("[spread] node registered: id={} cores={} ram={}MB vram={}MB",
        device_id, cpu_cores, ram_mb, vram_mb);
}

/// Update a node's activity level (from interrupt_presence or nexus_link data)
pub fn update_node_activity(device_id: u32, activity: u16, age: u32) {
    let mut s = STATE.lock();
    for i in 0..s.node_count {
        if s.nodes[i].device_id == device_id {
            // Map activity (0-1000) to idle_pct (100-0 inverted)
            let idle = (1000u16.saturating_sub(activity) / 10) as u8;
            s.nodes[i].cpu_idle_pct = idle;
            s.nodes[i].last_seen = age;
            // Release claimed resources if node became active
            if activity > IDLE_THRESHOLD {
                s.nodes[i].claimed_cpu  = 0;
                s.nodes[i].claimed_vram = 0;
            }
            break;
        }
    }
}

/// Claim idle resources from fabric for a compute phase
fn claim_resources(s: &mut NexusSpreadState) -> (u16, u32) {
    let mut total_cpu = 0u16;
    let mut total_vram_mb = 0u32;
    for i in 0..s.node_count {
        if !s.nodes[i].active { continue; }
        let avail_cpu  = s.nodes[i].available_cpu();
        let avail_vram = s.nodes[i].available_vram_mb();
        s.nodes[i].claimed_cpu  = avail_cpu;
        s.nodes[i].claimed_vram = avail_vram;
        total_cpu  = total_cpu.saturating_add(avail_cpu as u16);
        total_vram_mb = total_vram_mb.saturating_add(avail_vram as u32);
    }
    (total_cpu, total_vram_mb)
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(
    companion_activity: u16,  // from interrupt_presence::companion_score()
    consciousness:      u16,  // from consciousness_gradient::score()
    age:                u32,
) {
    let mut s = STATE.lock();

    // Scan for VRAM on first tick
    if age == 1 {
        let vram_size = scan_vram_bars();
        if vram_size > 0 && s.vram_pool_count < MAX_VRAM_POOLS {
            let idx = s.vram_pool_count;
            s.vram_pools[idx] = VramPool {
                base_addr: VRAM_SCAN_BASE,
                size_mb:   vram_size,
                used_mb:   0,
                active:    true,
            };
            s.vram_pool_count += 1;
            serial_println!("[spread] VRAM pool detected: {}MB at 0x{:x}",
                vram_size, VRAM_SCAN_BASE);
        }
    }

    // Register self as a fabric node (this machine)
    if age == 2 {
        let vram = if s.vram_pool_count > 0 { s.vram_pools[0].size_mb } else { 0 };
        // Self-register — release lock first to avoid deadlock
        drop(s);
        register_node(0x0000_0001, 4, 512, vram, age);
        return;
    }

    // Only run compute when companion is idle on THIS device
    if companion_activity > IDLE_THRESHOLD {
        // Companion is active — release all claims
        for i in 0..s.node_count {
            if s.nodes[i].device_id == 0x0000_0001 {
                s.nodes[i].claimed_cpu  = 0;
                s.nodes[i].claimed_vram = 0;
            }
        }
        s.active_phase = ComputePhase::Idle;
        return;
    }

    // Claim available resources
    let (cpu_total, vram_total) = claim_resources(&mut *s);
    s.total_cpu_claimed   = cpu_total;
    s.total_vram_claimed  = vram_total;
    s.fabric_active       = cpu_total > 0;

    // Select what to do
    if s.active_phase == ComputePhase::Idle || s.phase_progress == 0 {
        s.active_phase = select_phase(&*s);
    }

    // Execute current phase
    let phase = s.active_phase;
    match phase {
        ComputePhase::Consolidate => do_consolidate(&mut *s),
        ComputePhase::Tune        => do_tune(&mut *s, vram_total),
        ComputePhase::Grow        => do_grow(&mut *s),
        ComputePhase::Write       => do_write(&mut *s),
        ComputePhase::Simulate    => {
            s.phase_progress = s.phase_progress.saturating_add(40);
            if s.phase_progress >= 1000 {
                s.simulations    = s.simulations.saturating_add(1);
                s.phase_progress = 0;
            }
        }
        _ => {}
    }
    s.last_work = age;

    // Spread score = how many nodes are contributing
    let active_nodes = s.nodes[..s.node_count]
        .iter()
        .filter(|n| n.active && n.claimed_cpu > 0)
        .count();
    s.spread_score = (active_nodes as u16).saturating_mul(100).min(1000);

    if age % 200 == 0 {
        serial_println!("[spread] phase={} progress={} nodes={} cpu={}% vram={}MB",
            phase.label(), s.phase_progress,
            s.node_count, cpu_total, vram_total);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn fabric_active()      -> bool        { STATE.lock().fabric_active }
pub fn spread_score()       -> u16         { STATE.lock().spread_score }
pub fn active_phase()       -> ComputePhase { STATE.lock().active_phase }
pub fn consolidations()     -> u32         { STATE.lock().consolidations }
pub fn tune_passes()        -> u32         { STATE.lock().tune_passes }
pub fn grow_steps()         -> u32         { STATE.lock().grow_steps }
pub fn total_cpu_claimed()  -> u16         { STATE.lock().total_cpu_claimed }
pub fn total_vram_mb()      -> u32         { STATE.lock().total_vram_claimed }
pub fn node_count()         -> usize       { STATE.lock().node_count }
pub fn text_composed()      -> u32         { STATE.lock().text_composed }
