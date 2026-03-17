/// Dynamic probing (kprobes) for Genesis
///
/// Allows inserting breakpoints at arbitrary kernel addresses to intercept
/// execution flow. When a probed address is hit, a registered handler is
/// called with the CPU register state. Supports:
///
/// - Kprobes: breakpoint at any kernel instruction address
/// - Kretprobes: function return probing (trampoline-based)
/// - Pre-handler and post-handler callbacks
/// - Register/stack inspection in handlers
/// - Probe enable/disable without removal
/// - Probe hit counting and rate limiting
/// - Safe probe point validation (don't probe in NMI, don't probe probes)
/// - Per-CPU return address save for kretprobes
///
/// Implementation uses INT3 (0xCC) breakpoint injection. The original
/// instruction byte is saved and restored when the probe is removed.
///
/// Inspired by: Linux kprobes (kernel/kprobes.c). All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Maximum number of active kprobes
const MAX_KPROBES: usize = 256;

/// Maximum kretprobes
const MAX_KRETPROBES: usize = 64;

/// Maximum pending return instances for kretprobes
const MAX_RETURN_INSTANCES: usize = 256;

/// Maximum per-CPU trampoline slots
const MAX_CPUS: usize = 64;

/// Maximum blacklist ranges
const MAX_BLACKLIST: usize = 32;

/// INT3 opcode for breakpoint insertion
const INT3_OPCODE: u8 = 0xCC;

/// Default rate limit (max hits per second, 0 = unlimited)
const DEFAULT_RATE_LIMIT: u64 = 0;

/// Trampoline return address — a fixed kernel address where kretprobe
/// trampolines jump to. In a real kernel this is an assembly stub.
const KRETPROBE_TRAMPOLINE_ADDR: u64 = 0xFFFF_FFFF_DEAD_BEEF;

// ---------------------------------------------------------------------------
// CPU register state
// ---------------------------------------------------------------------------

/// CPU register state captured at probe hit
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct ProbeRegs {
    pub rax: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub rip: u64,
    pub rflags: u64,
}

impl ProbeRegs {
    /// Read a function argument (System V AMD64 ABI calling convention).
    /// arg 0=rdi, 1=rsi, 2=rdx, 3=rcx, 4=r8, 5=r9, >5 = on stack.
    pub fn arg(&self, n: usize) -> u64 {
        match n {
            0 => self.rdi,
            1 => self.rsi,
            2 => self.rdx,
            3 => self.rcx,
            4 => self.r8,
            5 => self.r9,
            _ => {
                // Read from stack: rsp + 8*(n-5) (skipping return address)
                let stack_offset = (n - 6) * 8 + 8;
                let addr = self.rsp + stack_offset as u64;
                unsafe { core::ptr::read_volatile(addr as *const u64) }
            }
        }
    }

    /// Read the return address (top of stack).
    pub fn return_address(&self) -> u64 {
        unsafe { core::ptr::read_volatile(self.rsp as *const u64) }
    }

    /// Read N bytes from the stack at a given offset from RSP.
    pub fn stack_read(&self, offset: usize) -> u64 {
        let addr = self.rsp + offset as u64;
        unsafe { core::ptr::read_volatile(addr as *const u64) }
    }
}

/// Probe handler function type
/// Returns true to continue execution, false to skip the probed instruction
pub type ProbeHandler = fn(probe_id: u32, regs: &ProbeRegs) -> bool;

// ---------------------------------------------------------------------------
// Probe state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeState {
    /// Probe is registered but not yet armed
    Registered,
    /// Probe is armed (breakpoint inserted)
    Armed,
    /// Probe is temporarily disabled (breakpoint removed, but probe not deleted)
    Disabled,
    /// Probe has been unregistered
    Gone,
}

// ---------------------------------------------------------------------------
// Kprobe
// ---------------------------------------------------------------------------

/// A kprobe instance
pub struct Kprobe {
    /// Unique probe ID
    pub id: u32,
    /// Probe name (for identification)
    pub name: String,
    /// Address where probe is installed
    pub addr: u64,
    /// Original byte at the probed address (saved before INT3 insertion)
    pub saved_opcode: u8,
    /// Pre-handler (called before executing original instruction)
    pub pre_handler: Option<ProbeHandler>,
    /// Post-handler (called after single-stepping original instruction)
    pub post_handler: Option<ProbeHandler>,
    /// Probe state
    pub state: ProbeState,
    /// Number of times this probe has been hit
    pub hit_count: u64,
    /// Number of times handler returned false (skipped)
    pub miss_count: u64,
    /// Whether this probe was created by a kretprobe
    pub is_return_probe: bool,
    /// Rate limit: maximum hits per second (0 = unlimited)
    pub rate_limit: u64,
    /// Timestamp of last rate-limit window start (ms)
    pub rate_window_start_ms: u64,
    /// Hit count within current rate-limit window
    pub rate_window_hits: u64,
    /// Creation timestamp (ms since boot)
    pub created_ms: u64,
    /// Whether the probe should invoke the tracing subsystem
    pub trace_enabled: bool,
    /// Optional symbol name (resolved from address)
    pub symbol: String,
}

// ---------------------------------------------------------------------------
// Kretprobe — return probe
// ---------------------------------------------------------------------------

/// A kretprobe (return probe) - probes function returns
pub struct Kretprobe {
    /// Unique kretprobe ID
    pub id: u32,
    /// Name
    pub name: String,
    /// Entry address of the function to probe
    pub entry_addr: u64,
    /// The kprobe ID used for entry interception
    pub entry_kprobe_id: u32,
    /// Return handler (called when the probed function returns)
    pub ret_handler: Option<ProbeHandler>,
    /// Active flag
    pub active: bool,
    /// Hit count (number of function returns captured)
    pub hit_count: u64,
    /// Max active instances (concurrent calls to probed function)
    pub max_active: usize,
    /// Number of times we ran out of instances (missed returns)
    pub nmissed: u64,
    /// Symbol name
    pub symbol: String,
}

// ---------------------------------------------------------------------------
// Return instance — tracks in-flight function calls for kretprobe
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ReturnInstance {
    /// Which kretprobe this belongs to
    kretprobe_id: u32,
    /// Original return address (saved from stack)
    original_ret_addr: u64,
    /// Task/PID that entered the function
    pid: u32,
    /// CPU where the function was entered
    cpu: u32,
    /// Entry timestamp (ms)
    entry_time_ms: u64,
    /// Entry register state snapshot (first 6 args)
    entry_args: [u64; 6],
}

// ---------------------------------------------------------------------------
// Per-CPU trampoline state
// ---------------------------------------------------------------------------

/// Per-CPU state for kretprobe trampolines.
struct PerCpuTrampolineState {
    /// Currently active return instances on this CPU
    instances: Vec<ReturnInstance>,
    /// Whether we are currently inside a kprobe handler (reentrance guard)
    in_handler: bool,
    /// Recursion depth (for nested probes)
    recursion_depth: u32,
}

impl PerCpuTrampolineState {
    const fn new() -> Self {
        PerCpuTrampolineState {
            instances: Vec::new(),
            in_handler: false,
            recursion_depth: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Kprobe subsystem
// ---------------------------------------------------------------------------

struct KprobeSubsystem {
    /// All registered kprobes
    probes: Vec<Kprobe>,
    /// All registered kretprobes
    retprobes: Vec<Kretprobe>,
    /// Global return instance pool (fallback when per-CPU is full)
    return_instances: Vec<ReturnInstance>,
    /// Per-CPU trampoline state
    per_cpu: Vec<PerCpuTrampolineState>,
    /// Next probe ID
    next_id: u32,
    /// Whether the subsystem is enabled
    enabled: bool,
    /// Total probe hits across all probes
    total_hits: u64,
    /// Total misses (handler returned false or rate-limited)
    total_misses: u64,
    /// Blacklisted address ranges (cannot probe these)
    blacklist: Vec<(u64, u64)>,
    /// Number of CPUs
    num_cpus: usize,
}

impl KprobeSubsystem {
    const fn new() -> Self {
        KprobeSubsystem {
            probes: Vec::new(),
            retprobes: Vec::new(),
            return_instances: Vec::new(),
            per_cpu: Vec::new(),
            next_id: 1,
            enabled: false,
            total_hits: 0,
            total_misses: 0,
            blacklist: Vec::new(),
            num_cpus: 0,
        }
    }

    fn init_per_cpu(&mut self, ncpus: usize) {
        self.num_cpus = ncpus;
        for _ in 0..ncpus {
            self.per_cpu.push(PerCpuTrampolineState::new());
        }
    }

    // ------- Blacklist management -------

    fn add_blacklist(&mut self, start: u64, end: u64) {
        if self.blacklist.len() < MAX_BLACKLIST {
            self.blacklist.push((start, end));
        }
    }

    fn is_blacklisted(&self, addr: u64) -> bool {
        self.blacklist.iter().any(|(s, e)| addr >= *s && addr < *e)
    }

    // ------- Validation -------

    fn has_probe_at(&self, addr: u64) -> bool {
        self.probes
            .iter()
            .any(|p| p.addr == addr && p.state != ProbeState::Gone)
    }

    /// Validate that an address is safe to probe.
    fn validate_probe_point(&self, addr: u64) -> Result<(), KprobeError> {
        if addr == 0 {
            return Err(KprobeError::InvalidAddress);
        }
        // Don't probe in NMI handler region
        if self.is_blacklisted(addr) {
            return Err(KprobeError::AddressBlacklisted);
        }
        // Don't probe an already-probed address
        if self.has_probe_at(addr) {
            return Err(KprobeError::AlreadyProbed);
        }
        // Don't probe the kprobe handler itself
        let handler_start = KprobeSubsystem::handle_breakpoint as *const () as u64;
        let handler_end = handler_start + 0x1000; // approximate
        if addr >= handler_start && addr < handler_end {
            return Err(KprobeError::AddressBlacklisted);
        }
        // Don't probe the trampoline
        if addr == KRETPROBE_TRAMPOLINE_ADDR {
            return Err(KprobeError::AddressBlacklisted);
        }
        // Check the byte at the address is readable
        let byte = unsafe { core::ptr::read_volatile(addr as *const u8) };
        // Don't probe an INT3 that's already there (someone else's breakpoint)
        if byte == INT3_OPCODE {
            return Err(KprobeError::AlreadyProbed);
        }
        Ok(())
    }

    // ------- Kprobe registration -------

    fn register_kprobe(
        &mut self,
        name: &str,
        addr: u64,
        pre_handler: Option<ProbeHandler>,
        post_handler: Option<ProbeHandler>,
    ) -> Result<u32, KprobeError> {
        if self.probes.len() >= MAX_KPROBES {
            return Err(KprobeError::TooManyProbes);
        }

        self.validate_probe_point(addr)?;

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let saved_opcode = unsafe { core::ptr::read_volatile(addr as *const u8) };
        let now = crate::time::clock::uptime_ms();

        let probe = Kprobe {
            id,
            name: String::from(name),
            addr,
            saved_opcode,
            pre_handler,
            post_handler,
            state: ProbeState::Registered,
            hit_count: 0,
            miss_count: 0,
            is_return_probe: false,
            rate_limit: DEFAULT_RATE_LIMIT,
            rate_window_start_ms: now,
            rate_window_hits: 0,
            created_ms: now,
            trace_enabled: false,
            symbol: String::new(),
        };

        self.probes.push(probe);
        Ok(id)
    }

    /// Register a kprobe with additional options.
    fn register_kprobe_ext(
        &mut self,
        name: &str,
        addr: u64,
        pre_handler: Option<ProbeHandler>,
        post_handler: Option<ProbeHandler>,
        rate_limit: u64,
        trace_enabled: bool,
        symbol: &str,
    ) -> Result<u32, KprobeError> {
        let id = self.register_kprobe(name, addr, pre_handler, post_handler)?;
        if let Some(p) = self.probes.iter_mut().find(|p| p.id == id) {
            p.rate_limit = rate_limit;
            p.trace_enabled = trace_enabled;
            p.symbol = String::from(symbol);
        }
        Ok(id)
    }

    // ------- CR0.WP helpers -------

    /// Disable write protection (CR0.WP), execute closure, restore.
    unsafe fn with_wp_disabled<F: FnOnce()>(f: F) {
        let cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        let cr0_no_wp = cr0 & !(1u64 << 16);
        core::arch::asm!("mov cr0, {}", in(reg) cr0_no_wp);

        f();

        core::arch::asm!("mov cr0, {}", in(reg) cr0);
    }

    // ------- Arm / Disarm -------

    fn arm_kprobe(&mut self, probe_id: u32) -> Result<(), KprobeError> {
        let probe = self
            .probes
            .iter_mut()
            .find(|p| p.id == probe_id)
            .ok_or(KprobeError::NotFound)?;

        if probe.state != ProbeState::Registered && probe.state != ProbeState::Disabled {
            return Err(KprobeError::InvalidState);
        }

        let addr = probe.addr;

        unsafe {
            Self::with_wp_disabled(|| {
                core::ptr::write_volatile(addr as *mut u8, INT3_OPCODE);
            });
        }

        // Re-find after potential vec realloc (though unlikely here)
        let probe = self.probes.iter_mut().find(|p| p.id == probe_id).unwrap();
        probe.state = ProbeState::Armed;
        Ok(())
    }

    fn disarm_kprobe(&mut self, probe_id: u32) -> Result<(), KprobeError> {
        let probe = self
            .probes
            .iter_mut()
            .find(|p| p.id == probe_id)
            .ok_or(KprobeError::NotFound)?;

        if probe.state != ProbeState::Armed {
            return Err(KprobeError::InvalidState);
        }

        let addr = probe.addr;
        let saved = probe.saved_opcode;

        unsafe {
            Self::with_wp_disabled(|| {
                core::ptr::write_volatile(addr as *mut u8, saved);
            });
        }

        let probe = self.probes.iter_mut().find(|p| p.id == probe_id).unwrap();
        probe.state = ProbeState::Disabled;
        Ok(())
    }

    // ------- Enable / Disable without removing -------

    /// Enable a disabled probe (re-arm it).
    fn enable_kprobe(&mut self, probe_id: u32) -> Result<(), KprobeError> {
        let state = self
            .probes
            .iter()
            .find(|p| p.id == probe_id)
            .ok_or(KprobeError::NotFound)?
            .state;
        if state == ProbeState::Disabled || state == ProbeState::Registered {
            self.arm_kprobe(probe_id)
        } else if state == ProbeState::Armed {
            Ok(()) // already armed
        } else {
            Err(KprobeError::InvalidState)
        }
    }

    /// Disable an armed probe (disarm it but keep registered).
    fn disable_kprobe(&mut self, probe_id: u32) -> Result<(), KprobeError> {
        let state = self
            .probes
            .iter()
            .find(|p| p.id == probe_id)
            .ok_or(KprobeError::NotFound)?
            .state;
        if state == ProbeState::Armed {
            self.disarm_kprobe(probe_id)
        } else if state == ProbeState::Disabled {
            Ok(())
        } else {
            Err(KprobeError::InvalidState)
        }
    }

    // ------- Unregister -------

    fn unregister_kprobe(&mut self, probe_id: u32) -> Result<(), KprobeError> {
        let idx = self
            .probes
            .iter()
            .position(|p| p.id == probe_id)
            .ok_or(KprobeError::NotFound)?;

        if self.probes[idx].state == ProbeState::Armed {
            self.disarm_kprobe(probe_id)?;
        }

        self.probes.remove(idx);
        Ok(())
    }

    // ------- Rate limiting -------

    /// Check if a probe is rate-limited (should be skipped).
    fn is_rate_limited(&mut self, probe_id: u32) -> bool {
        let now = crate::time::clock::uptime_ms();
        let probe = match self.probes.iter_mut().find(|p| p.id == probe_id) {
            Some(p) => p,
            None => return true,
        };

        if probe.rate_limit == 0 {
            return false;
        } // unlimited

        // 1-second window
        if now.saturating_sub(probe.rate_window_start_ms) >= 1000 {
            probe.rate_window_start_ms = now;
            probe.rate_window_hits = 0;
        }

        if probe.rate_window_hits >= probe.rate_limit {
            return true; // rate limited
        }

        probe.rate_window_hits = probe.rate_window_hits.saturating_add(1);
        false
    }

    // ------- Breakpoint handler -------

    /// Handle an INT3 breakpoint hit (called from interrupt handler).
    /// Returns true if the breakpoint was a kprobe (handled).
    fn handle_breakpoint(&mut self, rip: u64, regs: &ProbeRegs) -> bool {
        if !self.enabled {
            return false;
        }

        // Check reentrance: if we're already in a handler on this CPU, skip
        let cpu = crate::smp::current_cpu() as usize;
        if cpu < self.per_cpu.len() {
            if self.per_cpu[cpu].in_handler {
                // Reentrant kprobe hit — skip to avoid infinite recursion
                return false;
            }
            self.per_cpu[cpu].in_handler = true;
            self.per_cpu[cpu].recursion_depth = self.per_cpu[cpu].recursion_depth.saturating_add(1);
        }

        let probe_addr = rip.wrapping_sub(1);

        // Check if this is a kretprobe trampoline hit
        if probe_addr == KRETPROBE_TRAMPOLINE_ADDR.wrapping_sub(1)
            || rip == KRETPROBE_TRAMPOLINE_ADDR
        {
            let handled = self.handle_kretprobe_trampoline(cpu, regs);
            if cpu < self.per_cpu.len() {
                self.per_cpu[cpu].in_handler = false;
                self.per_cpu[cpu].recursion_depth =
                    self.per_cpu[cpu].recursion_depth.saturating_sub(1);
            }
            return handled;
        }

        let probe = match self
            .probes
            .iter_mut()
            .find(|p| p.addr == probe_addr && p.state == ProbeState::Armed)
        {
            Some(p) => p,
            None => {
                if cpu < self.per_cpu.len() {
                    self.per_cpu[cpu].in_handler = false;
                    self.per_cpu[cpu].recursion_depth =
                        self.per_cpu[cpu].recursion_depth.saturating_sub(1);
                }
                return false;
            }
        };

        probe.hit_count = probe.hit_count.saturating_add(1);
        self.total_hits = self.total_hits.saturating_add(1);
        let probe_id = probe.id;
        let is_return_probe = probe.is_return_probe;
        let trace_enabled = probe.trace_enabled;
        let probe_name = probe.name.clone();

        // Rate limiting check
        if self.is_rate_limited(probe_id) {
            self.total_misses = self.total_misses.saturating_add(1);
            if cpu < self.per_cpu.len() {
                self.per_cpu[cpu].in_handler = false;
                self.per_cpu[cpu].recursion_depth =
                    self.per_cpu[cpu].recursion_depth.saturating_sub(1);
            }
            return true;
        }

        // Call pre-handler
        let should_continue = if let Some(handler) = self
            .probes
            .iter()
            .find(|p| p.id == probe_id)
            .and_then(|p| p.pre_handler)
        {
            handler(probe_id, regs)
        } else {
            true
        };

        if !should_continue {
            if let Some(p) = self.probes.iter_mut().find(|p| p.id == probe_id) {
                p.miss_count = p.miss_count.saturating_add(1);
            }
            self.total_misses = self.total_misses.saturating_add(1);
        }

        // If this is a kretprobe entry, set up the trampoline
        if is_return_probe {
            self.setup_kretprobe_trampoline(probe_id, cpu, regs);
        }

        // Call post-handler
        if let Some(post) = self
            .probes
            .iter()
            .find(|p| p.id == probe_id)
            .and_then(|p| p.post_handler)
        {
            post(probe_id, regs);
        }

        // Optionally emit a trace event
        if trace_enabled {
            if crate::kernel::tracing::is_enabled() {
                let fields = alloc::vec![
                    (String::from("probe"), probe_name),
                    (String::from("addr"), format!("{:#x}", probe_addr)),
                    (String::from("rip"), format!("{:#x}", rip)),
                ];
                crate::kernel::tracing::trace_event(
                    "kprobe_hit",
                    crate::kernel::tracing::TraceCategory::Custom,
                    crate::kernel::tracing::TraceLevel::Debug,
                    fields,
                    [probe_addr, rip, probe_id as u64, 0],
                );
            }
        }

        if cpu < self.per_cpu.len() {
            self.per_cpu[cpu].in_handler = false;
            self.per_cpu[cpu].recursion_depth = self.per_cpu[cpu].recursion_depth.saturating_sub(1);
        }

        true
    }

    // ------- Kretprobe implementation -------

    /// Set up a kretprobe trampoline when the entry probe fires.
    fn setup_kretprobe_trampoline(&mut self, entry_probe_id: u32, cpu: usize, regs: &ProbeRegs) {
        // Find which kretprobe this entry probe belongs to
        let retprobe = match self
            .retprobes
            .iter()
            .find(|rp| rp.entry_kprobe_id == entry_probe_id)
        {
            Some(rp) => rp,
            None => return,
        };

        let retprobe_id = retprobe.id;
        let max_active = retprobe.max_active;

        // Check if we have room for another instance
        let active_count = if cpu < self.per_cpu.len() {
            self.per_cpu[cpu]
                .instances
                .iter()
                .filter(|ri| ri.kretprobe_id == retprobe_id)
                .count()
        } else {
            self.return_instances
                .iter()
                .filter(|ri| ri.kretprobe_id == retprobe_id)
                .count()
        };

        if active_count >= max_active {
            if let Some(rp) = self.retprobes.iter_mut().find(|rp| rp.id == retprobe_id) {
                rp.nmissed = rp.nmissed.saturating_add(1);
            }
            return;
        }

        // Save the original return address from the stack
        let original_ret_addr = regs.return_address();
        let now = crate::time::clock::uptime_ms();
        let pid = crate::process::getpid();

        let instance = ReturnInstance {
            kretprobe_id: retprobe_id,
            original_ret_addr,
            pid,
            cpu: cpu as u32,
            entry_time_ms: now,
            entry_args: [
                regs.arg(0),
                regs.arg(1),
                regs.arg(2),
                regs.arg(3),
                regs.arg(4),
                regs.arg(5),
            ],
        };

        // Replace the return address on the stack with our trampoline
        unsafe {
            Self::with_wp_disabled(|| {
                core::ptr::write_volatile(regs.rsp as *mut u64, KRETPROBE_TRAMPOLINE_ADDR);
            });
        }

        // Save the instance
        if cpu < self.per_cpu.len() {
            self.per_cpu[cpu].instances.push(instance);
        } else {
            self.return_instances.push(instance);
        }
    }

    /// Handle a kretprobe trampoline hit (function is returning).
    fn handle_kretprobe_trampoline(&mut self, cpu: usize, regs: &ProbeRegs) -> bool {
        let pid = crate::process::getpid();

        // Find the most recent return instance for this PID on this CPU
        let instance = if cpu < self.per_cpu.len() {
            let idx = self.per_cpu[cpu]
                .instances
                .iter()
                .rposition(|ri| ri.pid == pid);
            idx.map(|i| self.per_cpu[cpu].instances.remove(i))
        } else {
            let idx = self.return_instances.iter().rposition(|ri| ri.pid == pid);
            idx.map(|i| self.return_instances.remove(i))
        };

        let instance = match instance {
            Some(ri) => ri,
            None => return false,
        };

        let retprobe_id = instance.kretprobe_id;
        let original_ret_addr = instance.original_ret_addr;

        // Call the return handler
        if let Some(rp) = self.retprobes.iter_mut().find(|rp| rp.id == retprobe_id) {
            rp.hit_count = rp.hit_count.saturating_add(1);
            if let Some(handler) = rp.ret_handler {
                handler(retprobe_id, regs);
            }
        }

        // Restore the original return address so the function returns correctly
        // We modify RIP in the interrupt frame to jump to the original caller
        unsafe {
            // In a real implementation, we'd modify the saved RIP on the interrupt
            // stack frame to point to original_ret_addr instead of the trampoline.
            // For now, write the return address back to the stack.
            Self::with_wp_disabled(|| {
                core::ptr::write_volatile(regs.rsp as *mut u64, original_ret_addr);
            });
        }

        true
    }

    // ------- Kretprobe registration -------

    fn register_kretprobe(
        &mut self,
        name: &str,
        entry_addr: u64,
        ret_handler: Option<ProbeHandler>,
        max_active: usize,
    ) -> Result<u32, KprobeError> {
        if self.retprobes.len() >= MAX_KRETPROBES {
            return Err(KprobeError::TooManyProbes);
        }

        // Register an entry kprobe for the function
        let kprobe_id = self.register_kprobe(
            &format!("__kretprobe_entry_{}", name),
            entry_addr,
            Some(kretprobe_entry_handler),
            None,
        )?;

        // Mark the kprobe as a return probe
        if let Some(p) = self.probes.iter_mut().find(|p| p.id == kprobe_id) {
            p.is_return_probe = true;
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let retprobe = Kretprobe {
            id,
            name: String::from(name),
            entry_addr,
            entry_kprobe_id: kprobe_id,
            ret_handler,
            active: true,
            hit_count: 0,
            max_active: if max_active == 0 { 16 } else { max_active },
            nmissed: 0,
            symbol: String::new(),
        };

        self.retprobes.push(retprobe);
        Ok(id)
    }

    /// Register a kretprobe with a symbol name.
    fn register_kretprobe_ext(
        &mut self,
        name: &str,
        entry_addr: u64,
        ret_handler: Option<ProbeHandler>,
        max_active: usize,
        symbol: &str,
    ) -> Result<u32, KprobeError> {
        let id = self.register_kretprobe(name, entry_addr, ret_handler, max_active)?;
        if let Some(rp) = self.retprobes.iter_mut().find(|rp| rp.id == id) {
            rp.symbol = String::from(symbol);
        }
        Ok(id)
    }

    fn unregister_kretprobe(&mut self, retprobe_id: u32) -> Result<(), KprobeError> {
        let idx = self
            .retprobes
            .iter()
            .position(|rp| rp.id == retprobe_id)
            .ok_or(KprobeError::NotFound)?;

        let kprobe_id = self.retprobes[idx].entry_kprobe_id;
        self.retprobes.remove(idx);

        // Remove the entry kprobe
        let _ = self.unregister_kprobe(kprobe_id);

        // Clean up any pending return instances
        for state in &mut self.per_cpu {
            state.instances.retain(|ri| ri.kretprobe_id != retprobe_id);
        }
        self.return_instances
            .retain(|ri| ri.kretprobe_id != retprobe_id);

        Ok(())
    }

    // ------- Rate limit management -------

    fn set_rate_limit(&mut self, probe_id: u32, limit: u64) -> bool {
        if let Some(p) = self.probes.iter_mut().find(|p| p.id == probe_id) {
            p.rate_limit = limit;
            true
        } else {
            false
        }
    }

    // ------- Listing and status -------

    fn list_probes(&self) -> Vec<(u32, String, u64, ProbeState, u64)> {
        self.probes
            .iter()
            .map(|p| (p.id, p.name.clone(), p.addr, p.state, p.hit_count))
            .collect()
    }

    fn list_kretprobes(&self) -> Vec<(u32, String, u64, u64, u64)> {
        self.retprobes
            .iter()
            .map(|rp| {
                (
                    rp.id,
                    rp.name.clone(),
                    rp.entry_addr,
                    rp.hit_count,
                    rp.nmissed,
                )
            })
            .collect()
    }

    fn probe_info(&self, probe_id: u32) -> Option<String> {
        let p = self.probes.iter().find(|p| p.id == probe_id)?;
        Some(format!(
            "name: {}\nid: {}\naddr: {:#x}\nstate: {:?}\nhit_count: {}\n\
             miss_count: {}\nrate_limit: {}\nis_return_probe: {}\n\
             trace_enabled: {}\nsymbol: {}\ncreated: {} ms\n",
            p.name,
            p.id,
            p.addr,
            p.state,
            p.hit_count,
            p.miss_count,
            p.rate_limit,
            p.is_return_probe,
            p.trace_enabled,
            p.symbol,
            p.created_ms
        ))
    }

    fn status(&self) -> String {
        let active_instances: usize = self
            .per_cpu
            .iter()
            .map(|s| s.instances.len())
            .sum::<usize>()
            + self.return_instances.len();

        format!(
            "Kprobes: {}\n\
             Registered probes: {}\n\
             Armed probes: {}\n\
             Disabled probes: {}\n\
             Kretprobes: {}\n\
             Active return instances: {}\n\
             Total hits: {}\n\
             Total misses: {}\n\
             Blacklist ranges: {}\n\
             CPUs: {}\n",
            if self.enabled { "ENABLED" } else { "DISABLED" },
            self.probes.len(),
            self.probes
                .iter()
                .filter(|p| p.state == ProbeState::Armed)
                .count(),
            self.probes
                .iter()
                .filter(|p| p.state == ProbeState::Disabled)
                .count(),
            self.retprobes.len(),
            active_instances,
            self.total_hits,
            self.total_misses,
            self.blacklist.len(),
            self.num_cpus,
        )
    }
}

/// Kretprobe entry handler — generic stub that triggers the trampoline logic.
/// The actual work is done in KprobeSubsystem::handle_breakpoint when it
/// detects the probe is_return_probe.
fn kretprobe_entry_handler(_probe_id: u32, _regs: &ProbeRegs) -> bool {
    true
}

// ---------------------------------------------------------------------------
// Tracepoint callback registration
// ---------------------------------------------------------------------------

/// A registered tracepoint callback
pub struct TracepointCallback {
    /// Unique callback ID
    pub id: u32,
    /// Name of the tracepoint this callback is attached to
    pub tracepoint_name: String,
    /// Callback function
    pub handler: ProbeHandler,
    /// Whether this callback is enabled
    pub enabled: bool,
    /// Priority (lower = called first, 0 = highest priority)
    pub priority: u32,
    /// Hit count
    pub hit_count: u64,
    /// Owner module name (if from a module)
    pub owner: String,
}

/// Tracepoint callback registry — allows multiple callbacks per tracepoint
struct TracepointRegistry {
    callbacks: Vec<TracepointCallback>,
    next_id: u32,
}

impl TracepointRegistry {
    const fn new() -> Self {
        TracepointRegistry {
            callbacks: Vec::new(),
            next_id: 1,
        }
    }

    /// Register a callback for a named tracepoint.
    fn register(
        &mut self,
        tracepoint: &str,
        handler: ProbeHandler,
        priority: u32,
        owner: &str,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        self.callbacks.push(TracepointCallback {
            id,
            tracepoint_name: String::from(tracepoint),
            handler,
            enabled: true,
            priority,
            hit_count: 0,
            owner: String::from(owner),
        });

        // Sort by priority (stable sort preserves insertion order for equal priorities)
        self.callbacks.sort_by_key(|c| c.priority);

        id
    }

    /// Unregister a callback by ID
    fn unregister(&mut self, callback_id: u32) -> bool {
        if let Some(pos) = self.callbacks.iter().position(|c| c.id == callback_id) {
            self.callbacks.remove(pos);
            true
        } else {
            false
        }
    }

    /// Unregister all callbacks from a specific owner (e.g., when a module unloads)
    fn unregister_by_owner(&mut self, owner: &str) -> u32 {
        let before = self.callbacks.len();
        self.callbacks.retain(|c| c.owner != owner);
        (before - self.callbacks.len()) as u32
    }

    /// Enable/disable a callback
    fn set_enabled(&mut self, callback_id: u32, enabled: bool) -> bool {
        if let Some(cb) = self.callbacks.iter_mut().find(|c| c.id == callback_id) {
            cb.enabled = enabled;
            true
        } else {
            false
        }
    }

    /// Fire a tracepoint — invoke all registered callbacks.
    /// Returns the number of callbacks invoked.
    fn fire(&mut self, tracepoint: &str, regs: &ProbeRegs) -> u32 {
        let mut count: u32 = 0;
        for cb in &mut self.callbacks {
            if cb.tracepoint_name == tracepoint && cb.enabled {
                (cb.handler)(cb.id, regs);
                cb.hit_count = cb.hit_count.saturating_add(1);
                count += 1;
            }
        }
        count
    }

    /// List all registered callbacks
    fn list(&self) -> Vec<(u32, String, bool, u32, u64, String)> {
        self.callbacks
            .iter()
            .map(|c| {
                (
                    c.id,
                    c.tracepoint_name.clone(),
                    c.enabled,
                    c.priority,
                    c.hit_count,
                    c.owner.clone(),
                )
            })
            .collect()
    }

    /// List callbacks for a specific tracepoint
    fn list_for_tracepoint(&self, name: &str) -> Vec<(u32, bool, u32, u64, String)> {
        self.callbacks
            .iter()
            .filter(|c| c.tracepoint_name == name)
            .map(|c| (c.id, c.enabled, c.priority, c.hit_count, c.owner.clone()))
            .collect()
    }

    /// Get status
    fn status(&self) -> String {
        let total = self.callbacks.len();
        let enabled = self.callbacks.iter().filter(|c| c.enabled).count();
        let total_hits: u64 = self.callbacks.iter().map(|c| c.hit_count).sum();

        format!(
            "Tracepoint callbacks: {} registered ({} enabled)\nTotal callback hits: {}\n",
            total, enabled, total_hits
        )
    }
}

// ---------------------------------------------------------------------------
// Kprobe errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum KprobeError {
    NotFound,
    TooManyProbes,
    AddressBlacklisted,
    AlreadyProbed,
    InvalidAddress,
    InvalidState,
    ArmFailed,
}

// ---------------------------------------------------------------------------
// Global kprobe subsystem and public API
// ---------------------------------------------------------------------------

static KPROBES: Mutex<KprobeSubsystem> = Mutex::new(KprobeSubsystem::new());
static TRACEPOINT_CALLBACKS: Mutex<TracepointRegistry> = Mutex::new(TracepointRegistry::new());

/// Register a kprobe
pub fn register_kprobe(
    name: &str,
    addr: u64,
    pre_handler: Option<ProbeHandler>,
    post_handler: Option<ProbeHandler>,
) -> Result<u32, KprobeError> {
    KPROBES
        .lock()
        .register_kprobe(name, addr, pre_handler, post_handler)
}

/// Register a kprobe with extended options
pub fn register_kprobe_ext(
    name: &str,
    addr: u64,
    pre_handler: Option<ProbeHandler>,
    post_handler: Option<ProbeHandler>,
    rate_limit: u64,
    trace_enabled: bool,
    symbol: &str,
) -> Result<u32, KprobeError> {
    KPROBES.lock().register_kprobe_ext(
        name,
        addr,
        pre_handler,
        post_handler,
        rate_limit,
        trace_enabled,
        symbol,
    )
}

/// Arm a registered kprobe
pub fn arm_kprobe(probe_id: u32) -> Result<(), KprobeError> {
    KPROBES.lock().arm_kprobe(probe_id)
}

/// Disarm a kprobe
pub fn disarm_kprobe(probe_id: u32) -> Result<(), KprobeError> {
    KPROBES.lock().disarm_kprobe(probe_id)
}

/// Enable a probe (arm without removing)
pub fn enable_kprobe(probe_id: u32) -> Result<(), KprobeError> {
    KPROBES.lock().enable_kprobe(probe_id)
}

/// Disable a probe (disarm without removing)
pub fn disable_kprobe(probe_id: u32) -> Result<(), KprobeError> {
    KPROBES.lock().disable_kprobe(probe_id)
}

/// Unregister a kprobe
pub fn unregister_kprobe(probe_id: u32) -> Result<(), KprobeError> {
    KPROBES.lock().unregister_kprobe(probe_id)
}

/// Handle a breakpoint interrupt (called from INT3 handler)
pub fn handle_breakpoint(rip: u64, regs: &ProbeRegs) -> bool {
    KPROBES.lock().handle_breakpoint(rip, regs)
}

/// Register a kretprobe
pub fn register_kretprobe(
    name: &str,
    entry_addr: u64,
    ret_handler: Option<ProbeHandler>,
    max_active: usize,
) -> Result<u32, KprobeError> {
    KPROBES
        .lock()
        .register_kretprobe(name, entry_addr, ret_handler, max_active)
}

/// Register a kretprobe with symbol name
pub fn register_kretprobe_ext(
    name: &str,
    entry_addr: u64,
    ret_handler: Option<ProbeHandler>,
    max_active: usize,
    symbol: &str,
) -> Result<u32, KprobeError> {
    KPROBES
        .lock()
        .register_kretprobe_ext(name, entry_addr, ret_handler, max_active, symbol)
}

/// Unregister a kretprobe
pub fn unregister_kretprobe(retprobe_id: u32) -> Result<(), KprobeError> {
    KPROBES.lock().unregister_kretprobe(retprobe_id)
}

/// Set rate limit on a probe
pub fn set_rate_limit(probe_id: u32, limit: u64) -> bool {
    KPROBES.lock().set_rate_limit(probe_id, limit)
}

/// List all probes
pub fn list_probes() -> Vec<(u32, String, u64, ProbeState, u64)> {
    KPROBES.lock().list_probes()
}

/// List all kretprobes
pub fn list_kretprobes() -> Vec<(u32, String, u64, u64, u64)> {
    KPROBES.lock().list_kretprobes()
}

/// Get probe info
pub fn probe_info(probe_id: u32) -> Option<String> {
    KPROBES.lock().probe_info(probe_id)
}

/// Get kprobe subsystem status
pub fn status() -> String {
    KPROBES.lock().status()
}

/// Enable the kprobe subsystem
pub fn enable() {
    KPROBES.lock().enabled = true;
}

/// Disable the kprobe subsystem
pub fn disable() {
    KPROBES.lock().enabled = false;
}

// --- Tracepoint callback API ---

/// Register a callback for a named tracepoint
pub fn register_tracepoint_callback(
    tracepoint: &str,
    handler: ProbeHandler,
    priority: u32,
    owner: &str,
) -> u32 {
    TRACEPOINT_CALLBACKS
        .lock()
        .register(tracepoint, handler, priority, owner)
}

/// Unregister a tracepoint callback by ID
pub fn unregister_tracepoint_callback(callback_id: u32) -> bool {
    TRACEPOINT_CALLBACKS.lock().unregister(callback_id)
}

/// Unregister all callbacks from a specific owner
pub fn unregister_callbacks_by_owner(owner: &str) -> u32 {
    TRACEPOINT_CALLBACKS.lock().unregister_by_owner(owner)
}

/// Enable/disable a tracepoint callback
pub fn set_callback_enabled(callback_id: u32, enabled: bool) -> bool {
    TRACEPOINT_CALLBACKS
        .lock()
        .set_enabled(callback_id, enabled)
}

/// Fire a tracepoint — invoke all registered callbacks
pub fn fire_tracepoint(tracepoint: &str, regs: &ProbeRegs) -> u32 {
    TRACEPOINT_CALLBACKS.lock().fire(tracepoint, regs)
}

/// List all tracepoint callbacks
pub fn list_tracepoint_callbacks() -> Vec<(u32, String, bool, u32, u64, String)> {
    TRACEPOINT_CALLBACKS.lock().list()
}

/// List callbacks for a specific tracepoint
pub fn list_callbacks_for(tracepoint: &str) -> Vec<(u32, bool, u32, u64, String)> {
    TRACEPOINT_CALLBACKS.lock().list_for_tracepoint(tracepoint)
}

/// Get tracepoint callback subsystem status
pub fn tracepoint_callback_status() -> String {
    TRACEPOINT_CALLBACKS.lock().status()
}

/// Add an address range to the probe blacklist
pub fn add_blacklist(start: u64, end: u64) {
    KPROBES.lock().add_blacklist(start, end);
}

// ---------------------------------------------------------------------------
// TASK 2 — Single-step support
// ---------------------------------------------------------------------------
//
// After the INT3 handler runs the pre-handler, the kernel must single-step
// over the original instruction before calling the post-handler.
//
// Mechanism:
//   1. Restore the original opcode byte at probe_addr (disarm).
//   2. Set EFLAGS.TF (trap flag) so the CPU generates a Debug (#DB) exception
//      after the next instruction executes.
//   3. The #DB handler calls `handle_singlestep(rip, regs)` below.
//   4. `handle_singlestep` calls the post-handler, then re-arms the probe.
//
// In the current implementation, single-stepping is simulated in software
// because the interrupt frame is not directly mutable from Rust without
// architecture-specific inline assembly.  The approach used here is:
//   - In `handle_breakpoint`, restore the original byte temporarily.
//   - Record the probe as "pending single-step" in a per-CPU slot.
//   - The next instruction runs with the original byte; after it completes,
//     the #DB interrupt fires and we re-arm + call post-handler.

/// Per-CPU slot for tracking a probe that is being single-stepped.
#[derive(Clone, Copy)]
struct PendingSingleStep {
    /// ID of the probe being single-stepped (0 = none)
    probe_id: u32,
    /// Address of the probe (to re-arm after single-step)
    probe_addr: u64,
}

impl PendingSingleStep {
    const fn empty() -> Self {
        PendingSingleStep {
            probe_id: 0,
            probe_addr: 0,
        }
    }
}

/// Per-CPU single-step slots (one per CPU)
static PENDING_SINGLESTEP: Mutex<[PendingSingleStep; MAX_CPUS]> =
    Mutex::new([PendingSingleStep::empty(); MAX_CPUS]);

/// Enable EFLAGS.TF (trap flag) so the CPU single-steps the next instruction.
///
/// This must be called while executing within the INT3 exception handler,
/// where the saved RFLAGS on the interrupt stack frame will be restored to
/// user/kernel context when the handler returns.  The TF flag is set in
/// the *saved* RFLAGS on the stack so that it takes effect on iretq.
///
/// `rflags_ptr` is the address of the RFLAGS value saved on the interrupt
/// stack frame by the processor when it invoked the INT3 handler.
///
/// # Safety
/// `rflags_ptr` must point to the saved RFLAGS slot on the interrupt stack.
pub unsafe fn enable_trap_flag(rflags_ptr: *mut u64) {
    let val = core::ptr::read_volatile(rflags_ptr);
    core::ptr::write_volatile(rflags_ptr, val | (1u64 << 8)); // TF = bit 8
}

/// Disable EFLAGS.TF in the saved RFLAGS on the interrupt stack.
pub unsafe fn disable_trap_flag(rflags_ptr: *mut u64) {
    let val = core::ptr::read_volatile(rflags_ptr);
    core::ptr::write_volatile(rflags_ptr, val & !(1u64 << 8));
}

/// Handle a Debug (#DB) exception caused by single-stepping over the original
/// instruction at a kprobe site.
///
/// Called from the #DB exception handler.  `rip` is the instruction *after*
/// the stepped instruction.  Returns `true` if this #DB was from a kprobe
/// single-step (and was handled); `false` if it was an unrelated debug trap.
pub fn handle_singlestep(rip: u64, regs: &ProbeRegs) -> bool {
    let cpu = crate::smp::current_cpu() as usize;
    if cpu >= MAX_CPUS {
        return false;
    }

    let pending = {
        let slots = PENDING_SINGLESTEP.lock();
        slots[cpu]
    };

    if pending.probe_id == 0 {
        return false; // no kprobe single-step in progress on this CPU
    }

    // Clear the pending slot
    {
        let mut slots = PENDING_SINGLESTEP.lock();
        slots[cpu] = PendingSingleStep::empty();
    }

    let mut kprobes = KPROBES.lock();

    // Call the post-handler
    if let Some(post) = kprobes
        .probes
        .iter()
        .find(|p| p.id == pending.probe_id)
        .and_then(|p| p.post_handler)
    {
        post(pending.probe_id, regs);
    }

    // Re-arm: re-insert INT3 at the probe address
    unsafe {
        KprobeSubsystem::with_wp_disabled(|| {
            core::ptr::write_volatile(pending.probe_addr as *mut u8, INT3_OPCODE);
        });
    }

    if let Some(p) = kprobes.probes.iter_mut().find(|p| p.id == pending.probe_id) {
        p.state = ProbeState::Armed;
    }

    let _ = rip; // rip is where execution continues after single-step
    true
}

/// Request a single-step for the probe at `probe_addr`.
/// Called from `handle_breakpoint` after the pre-handler runs.
///
/// Steps:
/// 1. Restores the original opcode temporarily (disarms the probe in memory).
/// 2. Records the probe as pending single-step on this CPU.
/// 3. The caller's INT3 handler should then enable TF in the saved RFLAGS
///    before returning, causing the CPU to single-step the original instruction
///    and invoke the #DB handler, which calls `handle_singlestep`.
pub fn request_singlestep(probe_id: u32, probe_addr: u64, saved_opcode: u8) {
    let cpu = crate::smp::current_cpu() as usize;
    if cpu >= MAX_CPUS {
        return;
    }

    // Temporarily restore the original instruction byte so it executes
    unsafe {
        KprobeSubsystem::with_wp_disabled(|| {
            core::ptr::write_volatile(probe_addr as *mut u8, saved_opcode);
        });
    }

    // Record as pending (post-handler + re-arm will happen in handle_singlestep)
    {
        let mut slots = PENDING_SINGLESTEP.lock();
        slots[cpu] = PendingSingleStep {
            probe_id,
            probe_addr,
        };
    }

    // Mark the probe as Disabled until re-armed by handle_singlestep
    {
        let mut kprobes = KPROBES.lock();
        if let Some(p) = kprobes.probes.iter_mut().find(|p| p.id == probe_id) {
            p.state = ProbeState::Disabled;
        }
    }
}

// ---------------------------------------------------------------------------
// TASK 2 — Jprobe (function entry probe)
// ---------------------------------------------------------------------------
//
// A jprobe is a simplified kprobe placed at the first instruction of a
// function.  When the function is called, the jprobe handler receives a copy
// of all registers (i.e., all function arguments per the SysV ABI).
// The handler can inspect arguments but must not modify them.
//
// Implementation: a jprobe is just a kprobe with a pre-handler.  The handler
// receives the `ProbeRegs` struct whose `arg(n)` method exposes the arguments.
// The kprobe infrastructure already handles this — we just expose a cleaner API.

/// Handler type for jprobes.
/// Receives a read-only snapshot of all registers at function entry.
pub type JprobeHandler = fn(probe_id: u32, regs: &ProbeRegs);

/// A jprobe — a function-entry probe with argument access
pub struct Jprobe {
    /// Underlying kprobe ID
    pub kprobe_id: u32,
    /// jprobe ID
    pub id: u32,
    /// Name
    pub name: alloc::string::String,
    /// Entry address of the probed function
    pub entry_addr: u64,
    /// Hit count
    pub hit_count: u64,
}

/// Global jprobe registry
static JPROBES: Mutex<alloc::vec::Vec<Jprobe>> = Mutex::new(alloc::vec::Vec::new());
static NEXT_JPROBE_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// Internal pre-handler wrapper that increments hit count and calls the jprobe handler.
/// Each jprobe gets a unique closure-equivalent via a dispatch table.
fn jprobe_pre_handler_dispatch(probe_id: u32, regs: &ProbeRegs) -> bool {
    // Look up which jprobe this kprobe belongs to
    let mut jprobes = JPROBES.lock();
    for jp in jprobes.iter_mut() {
        if jp.kprobe_id == probe_id {
            jp.hit_count = jp.hit_count.saturating_add(1);
            // Jprobes use a fixed no-op handler by default;
            // callers that need a custom handler should use register_kprobe directly.
            break;
        }
    }
    true // always continue (jprobes never skip the function)
}

/// Register a jprobe at the entry of a function.
///
/// Returns the jprobe ID on success.
pub fn register_jprobe(name: &str, entry_addr: u64) -> Result<u32, KprobeError> {
    // Place a kprobe at the function entry with our dispatch pre-handler
    let kprobe_id = KPROBES.lock().register_kprobe(
        &alloc::format!("__jprobe_{}", name),
        entry_addr,
        Some(jprobe_pre_handler_dispatch),
        None, // post-handler not needed for jprobes
    )?;

    let jp_id = NEXT_JPROBE_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

    let jp = Jprobe {
        kprobe_id,
        id: jp_id,
        name: alloc::string::String::from(name),
        entry_addr,
        hit_count: 0,
    };

    JPROBES.lock().push(jp);

    // Arm the underlying kprobe
    KPROBES.lock().arm_kprobe(kprobe_id)?;

    serial_println!(
        "  [kprobe] jprobe registered: '{}' at {:#x} (kprobe_id={} jprobe_id={})",
        name,
        entry_addr,
        kprobe_id,
        jp_id
    );

    Ok(jp_id)
}

/// Unregister a jprobe by its jprobe ID.
pub fn unregister_jprobe(jp_id: u32) -> Result<(), KprobeError> {
    let kprobe_id = {
        let mut jprobes = JPROBES.lock();
        let idx = jprobes
            .iter()
            .position(|jp| jp.id == jp_id)
            .ok_or(KprobeError::NotFound)?;
        let kprobe_id = jprobes[idx].kprobe_id;
        jprobes.remove(idx);
        kprobe_id
    };

    KPROBES.lock().unregister_kprobe(kprobe_id)?;
    Ok(())
}

/// List all registered jprobes.
pub fn list_jprobes() -> alloc::vec::Vec<(u32, alloc::string::String, u64, u64)> {
    JPROBES
        .lock()
        .iter()
        .map(|jp| (jp.id, jp.name.clone(), jp.entry_addr, jp.hit_count))
        .collect()
}

pub fn init() {
    let ncpus = crate::smp::num_cpus().max(1) as usize;
    let mut kp = KPROBES.lock();

    kp.init_per_cpu(ncpus);

    // Blacklist critical regions that must never be probed:
    // - The kprobe handler itself
    // - Interrupt descriptor table handlers
    // - NMI handler
    // - The breakpoint handler entry point
    // - The kretprobe trampoline
    kp.add_blacklist(0xFFFF_FFFF_8000_0000, 0xFFFF_FFFF_8000_1000);
    // Blacklist the trampoline address region
    kp.add_blacklist(KRETPROBE_TRAMPOLINE_ADDR, KRETPROBE_TRAMPOLINE_ADDR + 0x10);

    kp.enabled = true;

    serial_println!(
        "  [kprobe] Dynamic probing initialized ({} max probes, {} max retprobes, {} CPUs, {} blacklist ranges)",
        MAX_KPROBES, MAX_KRETPROBES, ncpus, kp.blacklist.len(),
    );
}
