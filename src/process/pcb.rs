use super::context::CpuContext;
use super::{KERNEL_STACK_SIZE, MAX_PROCESSES};
/// Process Control Block — the kernel's record of each process
///
/// Every running process has a PCB that stores its state, register context,
/// memory mappings, metadata, credentials, file descriptors, signal handlers,
/// environment variables, resource usage, and scheduling parameters.
///
/// Inspired by: Linux task_struct, FreeBSD proc, seL4 TCB. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Process states
// ---------------------------------------------------------------------------

/// Process lifecycle states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Not yet started or just created
    New,
    /// Ready to run, in the scheduler queue
    Ready,
    /// Currently executing on a CPU
    Running,
    /// Waiting for an event (I/O, sleep, etc.)
    Blocked,
    /// Terminated, waiting for parent to collect exit code
    Dead,
    /// Stopped by signal (e.g. SIGTSTP / Ctrl+Z)
    Stopped,
    /// Traced by debugger (ptrace attach)
    Traced,
}

// ---------------------------------------------------------------------------
// Signal constants
// ---------------------------------------------------------------------------

/// Signal numbers (POSIX-compatible subset)
pub mod signal {
    pub const SIGHUP: u8 = 1;
    pub const SIGINT: u8 = 2;
    pub const SIGQUIT: u8 = 3;
    pub const SIGILL: u8 = 4;
    pub const SIGTRAP: u8 = 5;
    pub const SIGABRT: u8 = 6;
    pub const SIGBUS: u8 = 7;
    pub const SIGFPE: u8 = 8;
    pub const SIGKILL: u8 = 9;
    pub const SIGUSR1: u8 = 10;
    pub const SIGSEGV: u8 = 11;
    pub const SIGUSR2: u8 = 12;
    pub const SIGPIPE: u8 = 13;
    pub const SIGALRM: u8 = 14;
    pub const SIGTERM: u8 = 15;
    pub const SIGSTKFLT: u8 = 16;
    pub const SIGCHLD: u8 = 17;
    pub const SIGCONT: u8 = 18;
    pub const SIGSTOP: u8 = 19;
    pub const SIGTSTP: u8 = 20;
    pub const SIGTTIN: u8 = 21;
    pub const SIGTTOU: u8 = 22;
    pub const SIGURG: u8 = 23;
    pub const SIGXCPU: u8 = 24;
    pub const SIGXFSZ: u8 = 25;
    pub const SIGVTALRM: u8 = 26;
    pub const SIGPROF: u8 = 27;
    pub const SIGWINCH: u8 = 28;
    pub const SIGIO: u8 = 29;
    pub const SIGPWR: u8 = 30;
    pub const SIGSYS: u8 = 31;
    pub const MAX_SIGNALS: usize = 32;

    /// Return a human-readable name for a signal number
    pub fn name(sig: u8) -> &'static str {
        match sig {
            1 => "SIGHUP",
            2 => "SIGINT",
            3 => "SIGQUIT",
            4 => "SIGILL",
            5 => "SIGTRAP",
            6 => "SIGABRT",
            7 => "SIGBUS",
            8 => "SIGFPE",
            9 => "SIGKILL",
            10 => "SIGUSR1",
            11 => "SIGSEGV",
            12 => "SIGUSR2",
            13 => "SIGPIPE",
            14 => "SIGALRM",
            15 => "SIGTERM",
            16 => "SIGSTKFLT",
            17 => "SIGCHLD",
            18 => "SIGCONT",
            19 => "SIGSTOP",
            20 => "SIGTSTP",
            21 => "SIGTTIN",
            22 => "SIGTTOU",
            23 => "SIGURG",
            24 => "SIGXCPU",
            25 => "SIGXFSZ",
            26 => "SIGVTALRM",
            27 => "SIGPROF",
            28 => "SIGWINCH",
            29 => "SIGIO",
            30 => "SIGPWR",
            31 => "SIGSYS",
            _ => "UNKNOWN",
        }
    }

    /// Returns true if a signal terminates the process by default
    pub fn is_fatal_default(sig: u8) -> bool {
        matches!(
            sig,
            1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 11 | 13 | 14 | 15 | 24 | 25 | 31
        )
    }

    /// Returns true if a signal produces a core dump by default
    pub fn is_coredump_default(sig: u8) -> bool {
        matches!(sig, 3 | 4 | 5 | 6 | 7 | 8 | 11 | 24 | 25 | 31)
    }

    /// Returns true if a signal cannot be caught or ignored
    pub fn is_uncatchable(sig: u8) -> bool {
        sig == SIGKILL || sig == SIGSTOP
    }

    /// Returns true if the signal stops the process by default
    pub fn is_stop_default(sig: u8) -> bool {
        matches!(sig, 19 | 20 | 21 | 22)
    }
}

// ---------------------------------------------------------------------------
// Signal handling
// ---------------------------------------------------------------------------

/// What to do when a signal is delivered
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAction {
    /// Default action (terminate, stop, continue, or ignore depending on signal)
    Default,
    /// Ignore the signal entirely
    Ignore,
    /// Call a user-space handler at the given address
    Custom { handler: usize },
    /// Call a handler with siginfo_t (SA_SIGINFO style)
    CustomInfo { handler: usize },
}

/// Per-signal disposition flags (similar to sa_flags)
#[derive(Debug, Clone, Copy)]
pub struct SignalFlags {
    /// Restart interrupted syscalls (SA_RESTART)
    pub restart: bool,
    /// Don't generate SIGCHLD when child stops (SA_NOCLDSTOP)
    pub no_child_stop: bool,
    /// Don't create zombie children (SA_NOCLDWAIT)
    pub no_child_wait: bool,
    /// Reset handler to SIG_DFL after delivery (SA_RESETHAND)
    pub reset_hand: bool,
    /// Use alternate signal stack (SA_ONSTACK)
    pub on_stack: bool,
}

impl SignalFlags {
    pub const fn empty() -> Self {
        SignalFlags {
            restart: false,
            no_child_stop: false,
            no_child_wait: false,
            reset_hand: false,
            on_stack: false,
        }
    }
}

/// Full signal handler table entry
#[derive(Debug, Clone, Copy)]
pub struct SignalHandlerEntry {
    pub action: SignalAction,
    pub flags: SignalFlags,
    /// Mask of signals to block during handler execution
    pub sa_mask: u32,
}

impl SignalHandlerEntry {
    pub const fn default_action() -> Self {
        SignalHandlerEntry {
            action: SignalAction::Default,
            flags: SignalFlags::empty(),
            sa_mask: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// File descriptors
// ---------------------------------------------------------------------------

/// File descriptor entry in a process's FD table
#[derive(Debug, Clone)]
pub struct FileDescriptor {
    pub fd_type: FdType,
    pub offset: usize,
    pub flags: u32,
}

/// File descriptor type variants
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdType {
    Stdin,
    Stdout,
    Stderr,
    File { ino: u64 },
    Pipe { pipe_id: usize, read_end: bool },
    Socket { sock_id: u32 },
    Directory { ino: u64 },
    Epoll { epoll_id: u32 },
    EventFd { event_id: u32 },
    Timer { timer_id: u32 },
    Signal { sigset: u32 },
}

/// Per-process file descriptor table entry with close-on-exec flag
#[derive(Debug, Clone)]
pub struct FdEntry {
    /// The underlying file descriptor
    pub fd: FileDescriptor,
    /// Close this FD on exec (FD_CLOEXEC)
    pub close_on_exec: bool,
}

/// File descriptor open flags
pub mod fd_flags {
    pub const O_RDONLY: u32 = 0;
    pub const O_WRONLY: u32 = 1;
    pub const O_RDWR: u32 = 2;
    pub const O_APPEND: u32 = 0x400;
    pub const O_CREAT: u32 = 0x40;
    pub const O_TRUNC: u32 = 0x200;
    pub const O_EXCL: u32 = 0x80;
    pub const O_NONBLOCK: u32 = 0x800;
    pub const O_CLOEXEC: u32 = 0x80000;
    pub const O_DIRECTORY: u32 = 0x10000;
}

// ---------------------------------------------------------------------------
// Process credentials
// ---------------------------------------------------------------------------

/// Process credentials (real, effective, saved, filesystem)
#[derive(Debug, Clone)]
pub struct Credentials {
    /// Real user ID (who actually started the process)
    pub uid: u32,
    /// Real group ID
    pub gid: u32,
    /// Effective user ID (used for permission checks)
    pub euid: u32,
    /// Effective group ID
    pub egid: u32,
    /// Saved set-user-ID (from setuid binary)
    pub suid: u32,
    /// Saved set-group-ID
    pub sgid: u32,
    /// Filesystem user ID (for filesystem access, usually == euid)
    pub fsuid: u32,
    /// Filesystem group ID
    pub fsgid: u32,
    /// Supplementary group IDs
    pub groups: Vec<u32>,
    /// Linux capability bitmask (permitted | effective | inheritable)
    pub capabilities: u64,
}

impl Credentials {
    /// Root credentials (all zero)
    pub fn root() -> Self {
        Credentials {
            uid: 0,
            gid: 0,
            euid: 0,
            egid: 0,
            suid: 0,
            sgid: 0,
            fsuid: 0,
            fsgid: 0,
            groups: Vec::new(),
            capabilities: !0u64, // root has all capabilities
        }
    }

    /// Create credentials for a specific uid/gid
    pub fn new(uid: u32, gid: u32) -> Self {
        Credentials {
            uid,
            gid,
            euid: uid,
            egid: gid,
            suid: uid,
            sgid: gid,
            fsuid: uid,
            fsgid: gid,
            groups: Vec::new(),
            capabilities: 0,
        }
    }

    /// Check if the process has root privileges
    pub fn is_root(&self) -> bool {
        self.euid == 0
    }

    /// Check if process is in a specific group
    pub fn in_group(&self, gid: u32) -> bool {
        self.egid == gid || self.groups.contains(&gid)
    }

    /// Set real and effective UID (like setuid syscall)
    pub fn set_uid(&mut self, uid: u32) -> Result<(), &'static str> {
        if self.euid == 0 {
            self.uid = uid;
            self.euid = uid;
            self.suid = uid;
            self.fsuid = uid;
            Ok(())
        } else if uid == self.uid || uid == self.suid {
            self.euid = uid;
            self.fsuid = uid;
            Ok(())
        } else {
            Err("permission denied")
        }
    }

    /// Set real and effective GID (like setgid syscall)
    pub fn set_gid(&mut self, gid: u32) -> Result<(), &'static str> {
        if self.euid == 0 {
            self.gid = gid;
            self.egid = gid;
            self.sgid = gid;
            self.fsgid = gid;
            Ok(())
        } else if gid == self.gid || gid == self.sgid {
            self.egid = gid;
            self.fsgid = gid;
            Ok(())
        } else {
            Err("permission denied")
        }
    }

    /// Set only effective UID (like seteuid)
    pub fn set_euid(&mut self, euid: u32) -> Result<(), &'static str> {
        if self.euid == 0 || euid == self.uid || euid == self.suid {
            self.euid = euid;
            self.fsuid = euid;
            Ok(())
        } else {
            Err("permission denied")
        }
    }

    /// Set only effective GID (like setegid)
    pub fn set_egid(&mut self, egid: u32) -> Result<(), &'static str> {
        if self.euid == 0 || egid == self.gid || egid == self.sgid {
            self.egid = egid;
            self.fsgid = egid;
            Ok(())
        } else {
            Err("permission denied")
        }
    }

    /// Set supplementary groups (like setgroups)
    pub fn set_groups(&mut self, groups: Vec<u32>) -> Result<(), &'static str> {
        if self.euid == 0 {
            self.groups = groups;
            Ok(())
        } else {
            Err("permission denied: only root can set groups")
        }
    }
}

// ---------------------------------------------------------------------------
// Resource usage tracking
// ---------------------------------------------------------------------------

/// Per-process resource usage counters (like struct rusage)
#[derive(Debug, Clone)]
pub struct ResourceUsage {
    /// CPU ticks spent in user mode
    pub ticks_user: u64,
    /// CPU ticks spent in kernel mode
    pub ticks_kernel: u64,
    /// Total wall-clock ticks since creation
    pub ticks_wall: u64,
    /// Number of physical memory pages currently mapped
    pub memory_pages: u64,
    /// Peak resident set size in pages (high-water mark)
    pub max_rss: u64,
    /// Number of page faults (minor, satisfied from page cache)
    pub minor_faults: u64,
    /// Number of page faults (major, required I/O)
    pub major_faults: u64,
    /// Number of voluntary context switches (process yielded/blocked)
    pub voluntary_switches: u64,
    /// Number of involuntary context switches (preempted by scheduler)
    pub involuntary_switches: u64,
    /// Number of filesystem reads
    pub block_reads: u64,
    /// Number of filesystem writes
    pub block_writes: u64,
    /// Number of bytes read
    pub bytes_read: u64,
    /// Number of bytes written
    pub bytes_written: u64,
    /// Number of signals delivered
    pub signals_delivered: u64,
    /// Number of syscalls made
    pub syscall_count: u64,
    /// Tick when process was created
    pub start_tick: u64,
}

impl ResourceUsage {
    pub const fn new() -> Self {
        ResourceUsage {
            ticks_user: 0,
            ticks_kernel: 0,
            ticks_wall: 0,
            memory_pages: 0,
            max_rss: 0,
            minor_faults: 0,
            major_faults: 0,
            voluntary_switches: 0,
            involuntary_switches: 0,
            block_reads: 0,
            block_writes: 0,
            bytes_read: 0,
            bytes_written: 0,
            signals_delivered: 0,
            syscall_count: 0,
            start_tick: 0,
        }
    }

    /// Record a voluntary context switch
    pub fn record_voluntary_switch(&mut self) {
        self.voluntary_switches = self.voluntary_switches.saturating_add(1);
    }

    /// Record an involuntary context switch
    pub fn record_involuntary_switch(&mut self) {
        self.involuntary_switches = self.involuntary_switches.saturating_add(1);
    }

    /// Update user-mode ticks
    pub fn charge_user(&mut self, ticks: u64) {
        self.ticks_user += ticks;
    }

    /// Update kernel-mode ticks
    pub fn charge_kernel(&mut self, ticks: u64) {
        self.ticks_kernel += ticks;
    }

    /// Update memory page count and high-water mark
    pub fn update_memory(&mut self, pages: u64) {
        self.memory_pages = pages;
        if pages > self.max_rss {
            self.max_rss = pages;
        }
    }

    /// Record a minor page fault
    pub fn record_minor_fault(&mut self) {
        self.minor_faults = self.minor_faults.saturating_add(1);
    }

    /// Record a major page fault
    pub fn record_major_fault(&mut self) {
        self.major_faults = self.major_faults.saturating_add(1);
    }

    /// Record a syscall invocation
    pub fn record_syscall(&mut self) {
        self.syscall_count = self.syscall_count.saturating_add(1);
    }

    /// Total CPU time (user + kernel) in ticks
    pub fn total_cpu_ticks(&self) -> u64 {
        self.ticks_user + self.ticks_kernel
    }

    /// CPU utilization in per-mille (0-1000) since process start
    pub fn cpu_permille(&self) -> u32 {
        if self.ticks_wall == 0 {
            return 0;
        }
        let total = self.ticks_user + self.ticks_kernel;
        ((total * 1000) / self.ticks_wall) as u32
    }
}

// ---------------------------------------------------------------------------
// Process priority / nice value
// ---------------------------------------------------------------------------

/// Scheduling priority and nice value for a process
#[derive(Debug, Clone, Copy)]
pub struct ProcessPriority {
    /// Nice value: -20 (highest priority) to +19 (lowest)
    pub nice: i8,
    /// Static priority: 0-39 for normal, 1-99 for real-time
    pub static_priority: u8,
    /// Dynamic priority (computed by scheduler, may fluctuate)
    pub dynamic_priority: u8,
    /// Scheduling policy (normal, FIFO, round-robin, etc.)
    pub policy: u8,
}

/// Scheduling policy constants
pub mod sched_policy {
    pub const SCHED_NORMAL: u8 = 0;
    pub const SCHED_FIFO: u8 = 1;
    pub const SCHED_RR: u8 = 2;
    pub const SCHED_BATCH: u8 = 3;
    pub const SCHED_IDLE: u8 = 5;
    pub const SCHED_DEADLINE: u8 = 6;
}

impl ProcessPriority {
    pub const fn default_normal() -> Self {
        ProcessPriority {
            nice: 0,
            static_priority: 20,
            dynamic_priority: 20,
            policy: sched_policy::SCHED_NORMAL,
        }
    }

    /// Set nice value and recalculate static priority
    pub fn set_nice(&mut self, nice: i8) {
        let clamped = if nice < -20 {
            -20
        } else if nice > 19 {
            19
        } else {
            nice
        };
        self.nice = clamped;
        self.static_priority = (20 + clamped as i16) as u8;
    }

    /// Set real-time priority (1-99)
    pub fn set_rt_priority(&mut self, prio: u8, policy: u8) {
        let clamped = if prio < 1 {
            1
        } else if prio > 99 {
            99
        } else {
            prio
        };
        self.static_priority = clamped;
        self.dynamic_priority = clamped;
        self.policy = policy;
    }
}

// ---------------------------------------------------------------------------
// Alternate signal stack
// ---------------------------------------------------------------------------

/// Alternate signal stack info (for sigaltstack syscall)
#[derive(Debug, Clone, Copy)]
pub struct SignalStack {
    /// Base address of the alternate stack
    pub base: usize,
    /// Size of the alternate stack
    pub size: usize,
    /// Flags (SS_DISABLE, SS_ONSTACK)
    pub flags: u32,
}

pub mod ss_flags {
    pub const SS_ONSTACK: u32 = 1;
    pub const SS_DISABLE: u32 = 2;
}

impl SignalStack {
    pub const fn disabled() -> Self {
        SignalStack {
            base: 0,
            size: 0,
            flags: ss_flags::SS_DISABLE,
        }
    }
}

// ---------------------------------------------------------------------------
// Timer state
// ---------------------------------------------------------------------------

/// Per-process interval timers (ITIMER_REAL, ITIMER_VIRTUAL, ITIMER_PROF)
#[derive(Debug, Clone, Copy)]
pub struct IntervalTimer {
    /// Interval between timer fires (0 = one-shot)
    pub interval_ns: u64,
    /// Next fire time (absolute nanoseconds)
    pub next_fire_ns: u64,
    /// Whether this timer is armed
    pub armed: bool,
}

impl IntervalTimer {
    pub const fn inactive() -> Self {
        IntervalTimer {
            interval_ns: 0,
            next_fire_ns: 0,
            armed: false,
        }
    }
}

/// Process timers
#[derive(Debug, Clone, Copy)]
pub struct ProcessTimers {
    /// ITIMER_REAL: decrements in real time, delivers SIGALRM
    pub real: IntervalTimer,
    /// ITIMER_VIRTUAL: decrements in user CPU time, delivers SIGVTALRM
    pub virt: IntervalTimer,
    /// ITIMER_PROF: decrements in user+kernel CPU time, delivers SIGPROF
    pub prof: IntervalTimer,
}

impl ProcessTimers {
    pub const fn new() -> Self {
        ProcessTimers {
            real: IntervalTimer::inactive(),
            virt: IntervalTimer::inactive(),
            prof: IntervalTimer::inactive(),
        }
    }
}

// ---------------------------------------------------------------------------
// Kernel stack
// ---------------------------------------------------------------------------

/// Kernel stack allocation (fixed size per process)
pub struct KernelStack {
    /// Base address of the stack allocation
    pub base: usize,
    /// Size of the stack
    pub size: usize,
}

impl KernelStack {
    /// Get the top of the stack (stack grows downward on x86_64)
    pub fn top(&self) -> usize {
        self.base + self.size
    }
}

// ---------------------------------------------------------------------------
// Memory region tracking
// ---------------------------------------------------------------------------

/// Flags for memory mappings
pub mod mmap_flags {
    pub const PROT_READ: u64 = 0x1;
    pub const PROT_WRITE: u64 = 0x2;
    pub const PROT_EXEC: u64 = 0x4;
    pub const MAP_PRIVATE: u64 = 0x10;
    pub const MAP_SHARED: u64 = 0x20;
    pub const MAP_ANONYMOUS: u64 = 0x40;
    pub const MAP_FIXED: u64 = 0x80;
    pub const MAP_STACK: u64 = 0x100;
    pub const MAP_GROWSDOWN: u64 = 0x200;
}

/// A memory mapping region in the process address space
#[derive(Debug, Clone)]
pub struct MmapRegion {
    /// Virtual start address (page-aligned)
    pub virt_start: usize,
    /// Number of pages in this mapping
    pub num_pages: usize,
    /// Protection and mapping flags
    pub flags: u64,
    /// Optional name/description (e.g., "[stack]", "[heap]", "/lib/libc.so")
    pub name: String,
    /// Offset into file (if file-backed)
    pub file_offset: usize,
    /// Inode number (if file-backed, 0 for anonymous)
    pub inode: u64,
}

// ---------------------------------------------------------------------------
// Wait queue entry
// ---------------------------------------------------------------------------

/// What a blocked process is waiting for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    /// Not waiting
    None,
    /// Waiting for a child to exit (waitpid)
    Child,
    /// Waiting for I/O to complete
    Io,
    /// Sleeping for a duration
    Sleep { wake_tick: u64 },
    /// Waiting on a futex
    Futex { addr: usize },
    /// Waiting for a pipe to be readable/writable
    Pipe { pipe_id: usize },
    /// Waiting for a socket event
    Socket { sock_id: u32 },
    /// Waiting for a mutex/semaphore
    Mutex { addr: usize },
    /// Waiting for a signal
    Signal,
}

// ---------------------------------------------------------------------------
// The Process (PCB) itself
// ---------------------------------------------------------------------------

/// A single process
pub struct Process {
    // ---- identity ----
    /// Process ID (unique)
    pub pid: u32,
    /// Human-readable name
    pub name: String,
    /// Parent PID (0 = no parent / kernel)
    pub parent_pid: u32,
    /// Process group ID (for job control)
    pub pgid: u32,
    /// Session ID
    pub sid: u32,
    /// Child PIDs
    pub children: Vec<u32>,

    // ---- state ----
    /// Current state
    pub state: ProcessState,
    /// Exit code (set when process terminates)
    pub exit_code: i32,
    /// Whether this is a kernel-mode or user-mode process
    pub is_kernel: bool,
    /// Is this process stopped (Ctrl+Z)?
    pub stopped: bool,
    /// What the process is waiting for (when Blocked)
    pub wait_reason: WaitReason,

    // ---- CPU context ----
    /// Saved CPU register context (for context switching)
    pub context: CpuContext,
    /// Physical address of this process's PML4 page table
    pub page_table: usize,
    /// Kernel stack for this process (allocated from heap)
    pub kernel_stack: KernelStack,

    // ---- credentials ----
    /// Full process credentials (uid, gid, euid, egid, groups)
    pub creds: Credentials,
    /// Legacy uid field (kept for backward compatibility)
    pub uid: u32,
    /// Legacy gid field (kept for backward compatibility)
    pub gid: u32,

    // ---- signals ----
    /// Pending signals bitmask (bit N = signal N is pending)
    pub pending_signals: u32,
    /// Blocked signals bitmask (bit N = signal N is blocked/masked)
    pub signal_mask: u32,
    /// Saved signal mask (restored after signal handler returns)
    pub saved_signal_mask: u32,
    /// Signal handler table (signal number -> action)
    pub signal_handlers: BTreeMap<u8, SignalHandlerEntry>,
    /// Alternate signal stack
    pub signal_stack: SignalStack,
    /// Whether currently executing a signal handler
    pub in_signal_handler: bool,
    /// Nested signal handler depth
    pub signal_depth: u32,

    // ---- file descriptors ----
    /// File descriptor table (fd number -> FdEntry)
    pub fd_table: BTreeMap<i32, FdEntry>,
    /// Next FD number to allocate
    pub next_fd: i32,
    /// Maximum number of open FDs (rlimit)
    pub max_fds: i32,

    // ---- environment ----
    /// Environment variables
    pub environ: Vec<(String, String)>,
    /// Current working directory
    pub cwd: String,
    /// Command line arguments
    pub argv: Vec<String>,
    /// Process umask (default file permissions mask)
    pub umask: u32,

    // ---- memory ----
    /// Memory mappings (virt_start, num_pages, flags) -- legacy field
    pub mmaps: Vec<(usize, usize, u64)>,
    /// Detailed memory regions
    pub mmap_regions: Vec<MmapRegion>,
    /// Program break (top of heap, for brk/sbrk)
    pub brk: usize,
    /// Initial brk value (set at load time)
    pub brk_start: usize,

    // ---- scheduling ----
    /// Scheduling priority and nice value
    pub priority: ProcessPriority,
    /// CPU affinity mask (bit per CPU, u64::MAX = any CPU)
    pub cpu_affinity: u64,
    /// CPU this process last ran on
    pub last_cpu: u32,

    // ---- resource accounting ----
    /// Resource usage counters
    pub rusage: ResourceUsage,
    /// Children's accumulated resource usage
    pub children_rusage: ResourceUsage,

    // ---- timers ----
    /// Process interval timers
    pub timers: ProcessTimers,
}

impl Process {
    /// Create a new kernel-mode process
    pub fn new_kernel(pid: u32, name: &str) -> Self {
        // Allocate kernel stack from heap
        // Safety: KERNEL_STACK_SIZE is a non-zero compile-time constant (16 KiB) and 16 is a
        // valid power-of-two alignment, so from_size_align can never fail here.
        let stack_layout =
            unsafe { alloc::alloc::Layout::from_size_align_unchecked(KERNEL_STACK_SIZE, 16) };
        let stack_base = unsafe { alloc::alloc::alloc_zeroed(stack_layout) } as usize;

        // Set up default FD table with stdin/stdout/stderr
        let mut fd_table = BTreeMap::new();
        fd_table.insert(
            0,
            FdEntry {
                fd: FileDescriptor {
                    fd_type: FdType::Stdin,
                    offset: 0,
                    flags: fd_flags::O_RDONLY,
                },
                close_on_exec: false,
            },
        );
        fd_table.insert(
            1,
            FdEntry {
                fd: FileDescriptor {
                    fd_type: FdType::Stdout,
                    offset: 0,
                    flags: fd_flags::O_WRONLY,
                },
                close_on_exec: false,
            },
        );
        fd_table.insert(
            2,
            FdEntry {
                fd: FileDescriptor {
                    fd_type: FdType::Stderr,
                    offset: 0,
                    flags: fd_flags::O_WRONLY,
                },
                close_on_exec: false,
            },
        );

        Process {
            pid,
            name: String::from(name),
            parent_pid: 0,
            pgid: pid,
            sid: pid,
            children: Vec::new(),

            state: ProcessState::New,
            exit_code: 0,
            is_kernel: true,
            stopped: false,
            wait_reason: WaitReason::None,

            context: CpuContext::new(),
            page_table: crate::memory::paging::read_cr3(),
            kernel_stack: KernelStack {
                base: stack_base,
                size: KERNEL_STACK_SIZE,
            },

            creds: Credentials::root(),
            uid: 0,
            gid: 0,

            pending_signals: 0,
            signal_mask: 0,
            saved_signal_mask: 0,
            signal_handlers: BTreeMap::new(),
            signal_stack: SignalStack::disabled(),
            in_signal_handler: false,
            signal_depth: 0,

            fd_table,
            next_fd: 3,
            max_fds: 1024,

            environ: Vec::new(),
            cwd: String::from("/"),
            argv: Vec::new(),
            umask: 0o022,

            mmaps: Vec::new(),
            mmap_regions: Vec::new(),
            brk: 0,
            brk_start: 0,

            priority: ProcessPriority::default_normal(),
            cpu_affinity: u64::MAX,
            last_cpu: 0,

            rusage: ResourceUsage::new(),
            children_rusage: ResourceUsage::new(),

            timers: ProcessTimers::new(),
        }
    }

    // ----- fork -----

    /// Create a child process by forking (deep copy of all state)
    pub fn fork(&self, child_pid: u32) -> Self {
        // Safety: KERNEL_STACK_SIZE is a non-zero compile-time constant (16 KiB) and 16 is a
        // valid power-of-two alignment, so from_size_align can never fail here.
        let stack_layout =
            unsafe { alloc::alloc::Layout::from_size_align_unchecked(KERNEL_STACK_SIZE, 16) };
        let stack_base = unsafe { alloc::alloc::alloc_zeroed(stack_layout) } as usize;

        // Copy kernel stack contents
        unsafe {
            core::ptr::copy_nonoverlapping(
                self.kernel_stack.base as *const u8,
                stack_base as *mut u8,
                KERNEL_STACK_SIZE,
            );
        }

        // Clone the FD table (all descriptors inherited by child)
        let child_fd_table = self.fd_table.clone();

        // Clone signal handlers (inherited across fork)
        let child_signal_handlers = self.signal_handlers.clone();

        // Clone environment
        let child_environ = self.environ.clone();

        // Clone argv
        let child_argv = self.argv.clone();

        // Clone memory regions
        let child_mmap_regions = self.mmap_regions.clone();

        Process {
            pid: child_pid,
            name: self.name.clone(),
            parent_pid: self.pid,
            pgid: self.pgid,
            sid: self.sid,
            children: Vec::new(),

            state: ProcessState::Ready,
            exit_code: 0,
            is_kernel: self.is_kernel,
            stopped: false,
            wait_reason: WaitReason::None,

            context: self.context.clone(),
            page_table: self.page_table, // COW: share page table initially
            kernel_stack: KernelStack {
                base: stack_base,
                size: KERNEL_STACK_SIZE,
            },

            creds: self.creds.clone(),
            uid: self.uid,
            gid: self.gid,

            pending_signals: 0,
            signal_mask: self.signal_mask,
            saved_signal_mask: 0,
            signal_handlers: child_signal_handlers,
            signal_stack: SignalStack::disabled(),
            in_signal_handler: false,
            signal_depth: 0,

            fd_table: child_fd_table,
            next_fd: self.next_fd,
            max_fds: self.max_fds,

            environ: child_environ,
            cwd: self.cwd.clone(),
            argv: child_argv,
            umask: self.umask,

            mmaps: self.mmaps.clone(),
            mmap_regions: child_mmap_regions,
            brk: self.brk,
            brk_start: self.brk_start,

            priority: self.priority,
            cpu_affinity: self.cpu_affinity,
            last_cpu: self.last_cpu,

            rusage: ResourceUsage::new(),
            children_rusage: ResourceUsage::new(),

            timers: ProcessTimers::new(),
        }
    }

    // ----- signals -----

    /// Send a signal to this process
    pub fn send_signal(&mut self, sig: u8) {
        if sig < 32 {
            self.pending_signals |= 1 << sig;
        }
    }

    /// Check and clear a pending signal (returns highest priority pending)
    /// Respects the signal mask: blocked signals remain pending.
    pub fn dequeue_signal(&mut self) -> Option<u8> {
        let deliverable = self.pending_signals & !self.signal_mask;
        if deliverable == 0 {
            return None;
        }
        for sig in 1u8..32 {
            if deliverable & (1 << sig) != 0 {
                self.pending_signals &= !(1 << sig);
                return Some(sig);
            }
        }
        None
    }

    /// Check if a specific signal is pending
    pub fn has_signal(&self, sig: u8) -> bool {
        sig < 32 && (self.pending_signals & (1 << sig)) != 0
    }

    /// Check if a signal is blocked
    pub fn is_signal_blocked(&self, sig: u8) -> bool {
        if signal::is_uncatchable(sig) {
            return false;
        }
        sig < 32 && (self.signal_mask & (1 << sig)) != 0
    }

    /// Block additional signals (add to mask)
    pub fn block_signals(&mut self, mask: u32) {
        let safe_mask = mask & !((1 << signal::SIGKILL) | (1 << signal::SIGSTOP));
        self.signal_mask |= safe_mask;
    }

    /// Unblock signals (remove from mask)
    pub fn unblock_signals(&mut self, mask: u32) {
        self.signal_mask &= !mask;
    }

    /// Set signal mask to a specific value
    pub fn set_signal_mask(&mut self, mask: u32) {
        let safe_mask = mask & !((1 << signal::SIGKILL) | (1 << signal::SIGSTOP));
        self.signal_mask = safe_mask;
    }

    /// Install a signal handler
    pub fn set_signal_handler(
        &mut self,
        sig: u8,
        entry: SignalHandlerEntry,
    ) -> Result<(), &'static str> {
        if sig == 0 || sig >= 32 {
            return Err("invalid signal number");
        }
        if signal::is_uncatchable(sig) {
            return Err("cannot catch SIGKILL or SIGSTOP");
        }
        self.signal_handlers.insert(sig, entry);
        Ok(())
    }

    /// Get the signal handler for a signal
    pub fn get_signal_handler(&self, sig: u8) -> SignalHandlerEntry {
        self.signal_handlers
            .get(&sig)
            .copied()
            .unwrap_or(SignalHandlerEntry::default_action())
    }

    /// Count pending signals
    pub fn pending_signal_count(&self) -> u32 {
        self.pending_signals.count_ones()
    }

    // ----- file descriptors -----

    /// Allocate the lowest available file descriptor number
    pub fn alloc_fd(&mut self) -> Option<i32> {
        for fd in 0..self.max_fds {
            if !self.fd_table.contains_key(&fd) {
                return Some(fd);
            }
        }
        None
    }

    /// Allocate a specific fd number, or return None if already in use
    pub fn alloc_fd_at(&mut self, fd: i32) -> Option<i32> {
        if fd < 0 || fd >= self.max_fds {
            return None;
        }
        if self.fd_table.contains_key(&fd) {
            return None;
        }
        Some(fd)
    }

    /// Insert an FD entry at a specific number
    pub fn insert_fd(&mut self, fd: i32, entry: FdEntry) {
        self.fd_table.insert(fd, entry);
        if fd >= self.next_fd {
            self.next_fd = fd + 1;
        }
    }

    /// Remove (close) a file descriptor
    pub fn close_fd(&mut self, fd: i32) -> Option<FdEntry> {
        self.fd_table.remove(&fd)
    }

    /// Get a reference to a file descriptor entry
    pub fn get_fd(&self, fd: i32) -> Option<&FdEntry> {
        self.fd_table.get(&fd)
    }

    /// Get a mutable reference to a file descriptor entry
    pub fn get_fd_mut(&mut self, fd: i32) -> Option<&mut FdEntry> {
        self.fd_table.get_mut(&fd)
    }

    /// Duplicate a file descriptor (dup syscall)
    pub fn dup_fd(&mut self, old_fd: i32) -> Option<i32> {
        let entry = self.fd_table.get(&old_fd)?.clone();
        let new_fd = self.alloc_fd()?;
        let mut new_entry = entry;
        new_entry.close_on_exec = false;
        self.fd_table.insert(new_fd, new_entry);
        Some(new_fd)
    }

    /// Duplicate a file descriptor to a specific number (dup2 syscall)
    pub fn dup2_fd(&mut self, old_fd: i32, new_fd: i32) -> Option<i32> {
        if new_fd < 0 || new_fd >= self.max_fds {
            return None;
        }
        let entry = self.fd_table.get(&old_fd)?.clone();
        self.fd_table.remove(&new_fd);
        let mut new_entry = entry;
        new_entry.close_on_exec = false;
        self.fd_table.insert(new_fd, new_entry);
        Some(new_fd)
    }

    /// Count open file descriptors
    pub fn open_fd_count(&self) -> usize {
        self.fd_table.len()
    }

    /// Close all FDs marked close-on-exec (called during exec)
    pub fn close_cloexec_fds(&mut self) {
        let cloexec_fds: Vec<i32> = self
            .fd_table
            .iter()
            .filter(|(_, entry)| entry.close_on_exec)
            .map(|(&fd, _)| fd)
            .collect();
        for fd in cloexec_fds {
            self.fd_table.remove(&fd);
        }
    }

    /// Close all file descriptors (called on process exit)
    pub fn close_all_fds(&mut self) {
        self.fd_table.clear();
    }

    // ----- environment variables -----

    /// Get an environment variable by name
    pub fn getenv(&self, key: &str) -> Option<&String> {
        self.environ
            .iter()
            .find(|(k, _)| k.as_str() == key)
            .map(|(_, v)| v)
    }

    /// Set an environment variable (overwrite if exists)
    pub fn setenv(&mut self, key: &str, value: &str) {
        if let Some(entry) = self.environ.iter_mut().find(|(k, _)| k.as_str() == key) {
            entry.1 = String::from(value);
        } else {
            self.environ.push((String::from(key), String::from(value)));
        }
    }

    /// Remove an environment variable
    pub fn unsetenv(&mut self, key: &str) {
        self.environ.retain(|(k, _)| k.as_str() != key);
    }

    /// Clear all environment variables
    pub fn clearenv(&mut self) {
        self.environ.clear();
    }

    // ----- process groups / sessions -----

    /// Set the process group ID
    pub fn set_pgid(&mut self, pgid: u32) {
        self.pgid = pgid;
    }

    /// Create a new session (setsid syscall)
    pub fn create_session(&mut self) -> u32 {
        self.sid = self.pid;
        self.pgid = self.pid;
        self.sid
    }

    /// Check if this process is a session leader
    pub fn is_session_leader(&self) -> bool {
        self.sid == self.pid
    }

    /// Check if this process is a process group leader
    pub fn is_group_leader(&self) -> bool {
        self.pgid == self.pid
    }

    // ----- memory mapping helpers -----

    /// Add a detailed memory mapping region
    pub fn add_mmap_region(&mut self, region: MmapRegion) {
        self.mmaps
            .push((region.virt_start, region.num_pages, region.flags));
        self.mmap_regions.push(region);
    }

    /// Remove a memory mapping at the given virtual address
    pub fn remove_mmap_region(&mut self, virt_start: usize) -> Option<MmapRegion> {
        if let Some(pos) = self
            .mmap_regions
            .iter()
            .position(|r| r.virt_start == virt_start)
        {
            self.mmaps.retain(|&(vs, _, _)| vs != virt_start);
            Some(self.mmap_regions.remove(pos))
        } else {
            None
        }
    }

    /// Find the memory region containing a virtual address
    pub fn find_mmap_region(&self, virt_addr: usize) -> Option<&MmapRegion> {
        self.mmap_regions.iter().find(|r| {
            let end = r.virt_start + r.num_pages * 4096;
            virt_addr >= r.virt_start && virt_addr < end
        })
    }

    /// Total mapped memory in pages
    pub fn total_mapped_pages(&self) -> usize {
        self.mmap_regions.iter().map(|r| r.num_pages).sum()
    }

    // ----- misc -----

    /// Get the top of this process's kernel stack
    pub fn kernel_stack_top(&self) -> usize {
        self.kernel_stack.top()
    }

    /// Generate a status report (like /proc/[pid]/status)
    pub fn status_report(&self) -> String {
        use alloc::format;

        let state_str = match self.state {
            ProcessState::New => "N (new)",
            ProcessState::Ready => "R (ready)",
            ProcessState::Running => "R (running)",
            ProcessState::Blocked => "S (sleeping)",
            ProcessState::Dead => "Z (zombie)",
            ProcessState::Stopped => "T (stopped)",
            ProcessState::Traced => "t (traced)",
        };

        let mut report = String::new();
        report.push_str(&format!("Name:\t{}\n", self.name));
        report.push_str(&format!("State:\t{}\n", state_str));
        report.push_str(&format!("Pid:\t{}\n", self.pid));
        report.push_str(&format!("PPid:\t{}\n", self.parent_pid));
        report.push_str(&format!("Pgid:\t{}\n", self.pgid));
        report.push_str(&format!("Sid:\t{}\n", self.sid));
        report.push_str(&format!(
            "Uid:\t{}\t{}\t{}\t{}\n",
            self.creds.uid, self.creds.euid, self.creds.suid, self.creds.fsuid
        ));
        report.push_str(&format!(
            "Gid:\t{}\t{}\t{}\t{}\n",
            self.creds.gid, self.creds.egid, self.creds.sgid, self.creds.fsgid
        ));
        if !self.creds.groups.is_empty() {
            report.push_str("Groups:\t");
            for (i, g) in self.creds.groups.iter().enumerate() {
                if i > 0 {
                    report.push(' ');
                }
                report.push_str(&format!("{}", g));
            }
            report.push('\n');
        }
        report.push_str(&format!("FDSize:\t{}\n", self.fd_table.len()));
        report.push_str(&format!("VmSize:\t{} pages\n", self.total_mapped_pages()));
        report.push_str(&format!("VmRSS:\t{} pages\n", self.rusage.memory_pages));
        report.push_str(&format!("VmPeak:\t{} pages\n", self.rusage.max_rss));
        report.push_str(&format!("SigPnd:\t{:08x}\n", self.pending_signals));
        report.push_str(&format!("SigBlk:\t{:08x}\n", self.signal_mask));
        let caught: u32 = self
            .signal_handlers
            .keys()
            .filter(|&&sig| {
                matches!(
                    self.signal_handlers.get(&sig).map(|h| h.action),
                    Some(SignalAction::Custom { .. }) | Some(SignalAction::CustomInfo { .. })
                )
            })
            .fold(0u32, |acc, &sig| acc | (1 << sig));
        report.push_str(&format!("SigCgt:\t{:08x}\n", caught));
        let ignored: u32 = self
            .signal_handlers
            .keys()
            .filter(|&&sig| {
                matches!(
                    self.signal_handlers.get(&sig).map(|h| h.action),
                    Some(SignalAction::Ignore)
                )
            })
            .fold(0u32, |acc, &sig| acc | (1 << sig));
        report.push_str(&format!("SigIgn:\t{:08x}\n", ignored));
        report.push_str(&format!("Threads:\t1\n"));
        report.push_str(&format!("Nice:\t{}\n", self.priority.nice));
        report.push_str(&format!("Policy:\t{}\n", self.priority.policy));
        report.push_str(&format!("CpuAffinity:\t{:016x}\n", self.cpu_affinity));
        report.push_str(&format!("Cwd:\t{}\n", self.cwd));
        report.push_str(&format!("UserTicks:\t{}\n", self.rusage.ticks_user));
        report.push_str(&format!("KernelTicks:\t{}\n", self.rusage.ticks_kernel));
        report.push_str(&format!("VolCtxSw:\t{}\n", self.rusage.voluntary_switches));
        report.push_str(&format!(
            "InvolCtxSw:\t{}\n",
            self.rusage.involuntary_switches
        ));
        report.push_str(&format!("MinFlt:\t{}\n", self.rusage.minor_faults));
        report.push_str(&format!("MajFlt:\t{}\n", self.rusage.major_faults));
        report.push_str(&format!("Syscalls:\t{}\n", self.rusage.syscall_count));
        report.push_str(&format!("Children:\t{}\n", self.children.len()));
        report
    }

    /// Check if the process can send a signal to another process
    pub fn can_signal(&self, target: &Process, _sig: u8) -> bool {
        if self.creds.euid == 0 {
            return true;
        }
        if self.creds.uid == target.creds.uid || self.creds.uid == target.creds.suid {
            return true;
        }
        if self.creds.euid == target.creds.uid || self.creds.euid == target.creds.suid {
            return true;
        }
        false
    }

    /// Prepare the process for exec -- reset signal handlers, close CLOEXEC fds
    pub fn prepare_exec(&mut self) {
        let handlers_to_reset: Vec<u8> = self.signal_handlers.keys().copied().collect();
        for sig in handlers_to_reset {
            if let Some(entry) = self.signal_handlers.get(&sig) {
                match entry.action {
                    SignalAction::Ignore => {}
                    _ => {
                        self.signal_handlers.remove(&sig);
                    }
                }
            }
        }
        self.close_cloexec_fds();
        self.pending_signals = 0;
        self.signal_mask = 0;
        self.saved_signal_mask = 0;
        self.in_signal_handler = false;
        self.signal_depth = 0;
        self.signal_stack = SignalStack::disabled();
        self.mmaps.clear();
        self.mmap_regions.clear();
    }
}

// ---------------------------------------------------------------------------
// Global process table
// ---------------------------------------------------------------------------

/// Global process table -- fixed-size array of optional PCBs
pub static PROCESS_TABLE: Mutex<[Option<Process>; MAX_PROCESSES]> = {
    const NONE: Option<Process> = None;
    Mutex::new([NONE; MAX_PROCESSES])
};

// ---------------------------------------------------------------------------
// Helper functions operating on the process table
// ---------------------------------------------------------------------------

/// Find all PIDs in a given process group
pub fn pids_in_group(pgid: u32) -> Vec<u32> {
    let table = PROCESS_TABLE.lock();
    let mut result = Vec::new();
    for slot in table.iter() {
        if let Some(proc) = slot {
            if proc.pgid == pgid && proc.state != ProcessState::Dead {
                result.push(proc.pid);
            }
        }
    }
    result
}

/// Find all PIDs in a given session
pub fn pids_in_session(sid: u32) -> Vec<u32> {
    let table = PROCESS_TABLE.lock();
    let mut result = Vec::new();
    for slot in table.iter() {
        if let Some(proc) = slot {
            if proc.sid == sid && proc.state != ProcessState::Dead {
                result.push(proc.pid);
            }
        }
    }
    result
}

/// Count the total number of live processes
pub fn process_count() -> usize {
    let table = PROCESS_TABLE.lock();
    table
        .iter()
        .filter(|slot| {
            slot.as_ref()
                .map(|p| p.state != ProcessState::Dead)
                .unwrap_or(false)
        })
        .count()
}

/// Count zombie processes
pub fn zombie_count() -> usize {
    let table = PROCESS_TABLE.lock();
    table
        .iter()
        .filter(|slot| {
            slot.as_ref()
                .map(|p| p.state == ProcessState::Dead)
                .unwrap_or(false)
        })
        .count()
}

/// Get a summary of all processes (pid, name, state, ppid)
pub fn process_list() -> Vec<(u32, String, ProcessState, u32)> {
    let table = PROCESS_TABLE.lock();
    let mut result = Vec::new();
    for slot in table.iter() {
        if let Some(proc) = slot {
            result.push((proc.pid, proc.name.clone(), proc.state, proc.parent_pid));
        }
    }
    result
}
