/// Crash dump / kdump subsystem for Genesis
///
/// Captures diagnostic information when the kernel panics or crashes.
/// Records: CPU register state, stack traces, memory regions, loaded modules,
/// and recent kernel log messages. The dump is stored in a reserved memory
/// region that survives warm reboots (pstore-like).
///
/// The crash dump extends the default panic handler with structured data
/// collection for post-mortem analysis.
///
/// Inspired by: Linux kdump/kexec (kernel/crash_core.c). All code is original.
use crate::serial_println;
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// Maximum stack frames to capture
const MAX_STACK_FRAMES: usize = 64;

/// Maximum memory regions in dump
const MAX_MEMORY_REGIONS: usize = 32;

/// Maximum log lines to capture
const MAX_LOG_LINES: usize = 128;

/// Size of the reserved crash dump region (256 KB)
const CRASH_DUMP_REGION_SIZE: usize = 256 * 1024;

/// Magic number to identify valid crash dump in memory
const CRASH_DUMP_MAGIC: u64 = 0x4745_4E45_5349_5344; // "GENESISD" in hex-ish

/// Whether a crash is currently being handled (prevent re-entry)
static CRASH_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// CPU register snapshot
#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct CpuRegisters {
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
    pub cs: u64,
    pub ss: u64,
    pub cr0: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
}

impl CpuRegisters {
    /// Capture current CPU registers
    pub fn capture() -> Self {
        let mut regs = CpuRegisters::default();

        unsafe {
            core::arch::asm!(
                "mov {rax}, rax",
                "mov {rcx}, rcx",
                "mov {rdx}, rdx",
                "mov {rsi}, rsi",
                "mov {rdi}, rdi",
                "mov {r8}, r8",
                "mov {r9}, r9",
                "mov {r10}, r10",
                "mov {r11}, r11",
                "mov {r12}, r12",
                "mov {r13}, r13",
                "mov {r14}, r14",
                "mov {r15}, r15",
                "mov {rbp}, rbp",
                "mov {rsp}, rsp",
                rax = out(reg) regs.rax,
                rcx = out(reg) regs.rcx,
                rdx = out(reg) regs.rdx,
                rsi = out(reg) regs.rsi,
                rdi = out(reg) regs.rdi,
                r8 = out(reg) regs.r8,
                r9 = out(reg) regs.r9,
                r10 = out(reg) regs.r10,
                r11 = out(reg) regs.r11,
                r12 = out(reg) regs.r12,
                r13 = out(reg) regs.r13,
                r14 = out(reg) regs.r14,
                r15 = out(reg) regs.r15,
                rbp = out(reg) regs.rbp,
                rsp = out(reg) regs.rsp,
            );

            // Read RIP via LEA on the next instruction
            core::arch::asm!(
                "lea {}, [rip]",
                out(reg) regs.rip,
            );

            // Read RFLAGS
            core::arch::asm!(
                "pushfq",
                "pop {}",
                out(reg) regs.rflags,
            );

            // Read control registers
            core::arch::asm!("mov {}, cr0", out(reg) regs.cr0);
            core::arch::asm!("mov {}, cr2", out(reg) regs.cr2);
            core::arch::asm!("mov {}, cr3", out(reg) regs.cr3);
            core::arch::asm!("mov {}, cr4", out(reg) regs.cr4);
        }

        regs
    }

    /// Format registers for display
    pub fn format(&self) -> String {
        format!(
            "RAX={:#018X}  RCX={:#018X}  RDX={:#018X}\n\
             RSI={:#018X}  RDI={:#018X}  RBP={:#018X}\n\
             RSP={:#018X}  R8 ={:#018X}  R9 ={:#018X}\n\
             R10={:#018X}  R11={:#018X}  R12={:#018X}\n\
             R13={:#018X}  R14={:#018X}  R15={:#018X}\n\
             RIP={:#018X}  RFLAGS={:#018X}\n\
             CR0={:#018X}  CR2={:#018X}  CR3={:#018X}  CR4={:#018X}",
            self.rax,
            self.rcx,
            self.rdx,
            self.rsi,
            self.rdi,
            self.rbp,
            self.rsp,
            self.r8,
            self.r9,
            self.r10,
            self.r11,
            self.r12,
            self.r13,
            self.r14,
            self.r15,
            self.rip,
            self.rflags,
            self.cr0,
            self.cr2,
            self.cr3,
            self.cr4,
        )
    }
}

/// A single stack frame in a backtrace
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// Frame number (0 = most recent)
    pub frame_num: usize,
    /// Return address (instruction pointer)
    pub rip: u64,
    /// Frame pointer (RBP at this frame)
    pub rbp: u64,
    /// Symbol name if resolved
    pub symbol: Option<String>,
    /// Offset within symbol
    pub offset: u64,
}

/// Memory region snapshot
#[derive(Debug, Clone)]
pub struct MemoryRegionDump {
    /// Start address
    pub start: u64,
    /// Size in bytes
    pub size: usize,
    /// Description
    pub description: String,
    /// First 64 bytes of the region (preview)
    pub preview: [u8; 64],
}

/// Crash reason
#[derive(Debug, Clone)]
pub enum CrashReason {
    Panic(String),
    DoubleFault,
    PageFault { addr: u64, error_code: u64 },
    GeneralProtectionFault { error_code: u64 },
    StackOverflow,
    InvalidOpcode { addr: u64 },
    NMI,
    MachineCheck,
    UserTriggered,
    Unknown,
}

impl CrashReason {
    fn description(&self) -> String {
        match self {
            CrashReason::Panic(msg) => format!("Kernel panic: {}", msg),
            CrashReason::DoubleFault => String::from("Double fault"),
            CrashReason::PageFault { addr, error_code } => {
                format!("Page fault at {:#X} (error={:#X})", addr, error_code)
            }
            CrashReason::GeneralProtectionFault { error_code } => {
                format!("General protection fault (error={:#X})", error_code)
            }
            CrashReason::StackOverflow => String::from("Stack overflow"),
            CrashReason::InvalidOpcode { addr } => format!("Invalid opcode at {:#X}", addr),
            CrashReason::NMI => String::from("Non-maskable interrupt"),
            CrashReason::MachineCheck => String::from("Machine check exception"),
            CrashReason::UserTriggered => String::from("User-triggered crash dump"),
            CrashReason::Unknown => String::from("Unknown crash reason"),
        }
    }
}

/// Complete crash dump record
pub struct CrashDump {
    /// Magic number for identification
    pub magic: u64,
    /// Timestamp (ms since boot)
    pub timestamp_ms: u64,
    /// CPU that crashed
    pub cpu: u32,
    /// PID of crashing task
    pub pid: u32,
    /// Crash reason
    pub reason: CrashReason,
    /// CPU registers at crash time
    pub registers: CpuRegisters,
    /// Stack trace
    pub stack_frames: Vec<StackFrame>,
    /// Memory region snapshots
    pub memory_regions: Vec<MemoryRegionDump>,
    /// Recent kernel log lines
    pub log_lines: Vec<String>,
    /// Loaded kernel modules at crash time
    pub loaded_modules: Vec<String>,
    /// Uptime at crash
    pub uptime_ms: u64,
    /// Number of online CPUs
    pub num_cpus: u32,
    /// Kernel version string
    pub kernel_version: String,
}

/// Crash dump subsystem
struct CrashDumpSubsystem {
    /// Reserved memory region for crash data (start address)
    reserved_region: u64,
    /// Size of reserved region
    reserved_size: usize,
    /// Last crash dump (if any)
    last_dump: Option<CrashDump>,
    /// Whether to dump memory regions
    dump_memory: bool,
    /// Whether to capture kernel log
    dump_log: bool,
    /// Whether system is initialized
    initialized: bool,
    /// Pre-registered memory regions of interest
    watched_regions: Vec<(u64, usize, String)>,
}

impl CrashDumpSubsystem {
    const fn new() -> Self {
        CrashDumpSubsystem {
            reserved_region: 0,
            reserved_size: 0,
            last_dump: None,
            dump_memory: true,
            dump_log: true,
            initialized: false,
            watched_regions: Vec::new(),
        }
    }

    /// Walk the stack using frame pointers (RBP chain)
    fn walk_stack(&self, initial_rbp: u64, initial_rip: u64) -> Vec<StackFrame> {
        let mut frames = Vec::new();

        // First frame is the crash point
        frames.push(StackFrame {
            frame_num: 0,
            rip: initial_rip,
            rbp: initial_rbp,
            symbol: None,
            offset: 0,
        });

        let mut rbp = initial_rbp;

        for i in 1..MAX_STACK_FRAMES {
            // Validate RBP is in a reasonable range (kernel space)
            if rbp == 0 || rbp < 0x1000 || rbp > 0xFFFF_FFFF_FFFF_F000 {
                break;
            }

            // Read return address and previous RBP from stack frame
            let (prev_rbp, ret_addr) = unsafe {
                let p = rbp as *const u64;
                // Check pointer validity before dereferencing
                if (p as u64) < 0x1000 {
                    break;
                }
                let prev = core::ptr::read_volatile(p);
                let ret = core::ptr::read_volatile(p.add(1));
                (prev, ret)
            };

            if ret_addr == 0 {
                break;
            }

            frames.push(StackFrame {
                frame_num: i,
                rip: ret_addr,
                rbp: prev_rbp,
                symbol: None, // Symbol resolution would happen here
                offset: 0,
            });

            rbp = prev_rbp;
        }

        frames
    }

    /// Capture a memory region snapshot
    fn capture_region(&self, addr: u64, size: usize, desc: &str) -> MemoryRegionDump {
        let mut preview = [0u8; 64];
        let copy_len = if size < 64 { size } else { 64 };

        if addr >= 0x1000 {
            unsafe {
                for i in 0..copy_len {
                    preview[i] = core::ptr::read_volatile((addr + i as u64) as *const u8);
                }
            }
        }

        MemoryRegionDump {
            start: addr,
            size,
            description: String::from(desc),
            preview,
        }
    }

    /// Register a memory region to capture during crash
    fn watch_region(&mut self, addr: u64, size: usize, description: &str) {
        if self.watched_regions.len() < MAX_MEMORY_REGIONS {
            self.watched_regions
                .push((addr, size, String::from(description)));
        }
    }

    /// Perform a complete crash dump
    fn perform_dump(&mut self, reason: CrashReason, regs: Option<CpuRegisters>) -> CrashDump {
        let registers = regs.unwrap_or_else(CpuRegisters::capture);

        // Walk the stack
        let stack_frames = self.walk_stack(registers.rbp, registers.rip);

        // Capture watched memory regions
        let mut memory_regions = Vec::new();
        if self.dump_memory {
            // Capture stack area around RSP
            let stack_start = registers.rsp.saturating_sub(256);
            memory_regions.push(self.capture_region(stack_start, 512, "Stack around RSP"));

            // Capture watched regions
            for (addr, size, desc) in &self.watched_regions {
                memory_regions.push(self.capture_region(*addr, *size, desc));
            }
        }

        // Get loaded modules list
        let loaded_modules: Vec<String> = crate::kernel::modules::list()
            .iter()
            .map(|(name, _size, _state)| name.clone())
            .collect();

        let dump = CrashDump {
            magic: CRASH_DUMP_MAGIC,
            timestamp_ms: crate::time::clock::uptime_ms(),
            cpu: crate::smp::current_cpu(),
            pid: crate::process::getpid(),
            reason,
            registers,
            stack_frames,
            memory_regions,
            log_lines: Vec::new(), // Would capture from kernel log buffer
            loaded_modules,
            uptime_ms: crate::time::clock::uptime_ms(),
            num_cpus: crate::smp::num_cpus(),
            kernel_version: String::from("Genesis 0.1.0"),
        };

        dump
    }

    /// Format a crash dump for display on serial console
    fn format_dump(dump: &CrashDump) -> String {
        let mut s = String::new();

        s.push_str("============================================================\n");
        s.push_str("                    GENESIS CRASH DUMP\n");
        s.push_str("============================================================\n\n");

        s.push_str(&format!("Reason: {}\n", dump.reason.description()));
        s.push_str(&format!("Time:   {} ms since boot\n", dump.timestamp_ms));
        s.push_str(&format!("CPU:    {}\n", dump.cpu));
        s.push_str(&format!("PID:    {}\n", dump.pid));
        s.push_str(&format!("CPUs:   {} online\n", dump.num_cpus));
        s.push_str(&format!("Kernel: {}\n\n", dump.kernel_version));

        // Registers
        s.push_str("--- CPU Registers ---\n");
        s.push_str(&dump.registers.format());
        s.push_str("\n\n");

        // Stack trace
        s.push_str("--- Stack Trace ---\n");
        for frame in &dump.stack_frames {
            let sym = frame.symbol.as_deref().unwrap_or("???");
            s.push_str(&format!(
                "  #{:<3} {:#018X} {}\n",
                frame.frame_num, frame.rip, sym
            ));
        }
        s.push('\n');

        // Memory regions
        if !dump.memory_regions.is_empty() {
            s.push_str("--- Memory Regions ---\n");
            for region in &dump.memory_regions {
                s.push_str(&format!(
                    "  {:#X} ({} bytes): {}\n",
                    region.start, region.size, region.description
                ));
                // Print hex preview
                s.push_str("    ");
                for (i, byte) in region.preview.iter().take(32).enumerate() {
                    if i > 0 && i % 16 == 0 {
                        s.push_str("\n    ");
                    }
                    s.push_str(&format!("{:02X} ", byte));
                }
                s.push('\n');
            }
            s.push('\n');
        }

        // Loaded modules
        if !dump.loaded_modules.is_empty() {
            s.push_str("--- Loaded Modules ---\n");
            for m in &dump.loaded_modules {
                s.push_str(&format!("  {}\n", m));
            }
            s.push('\n');
        }

        s.push_str("============================================================\n");
        s
    }
}

/// Crash dump errors
#[derive(Debug)]
pub enum CrashDumpError {
    NotInitialized,
    AlreadyInProgress,
    NoRegion,
}

/// Global crash dump subsystem
static CRASH_DUMP: Mutex<CrashDumpSubsystem> = Mutex::new(CrashDumpSubsystem::new());

/// Trigger a crash dump (called from panic handler or fault handler)
pub fn trigger_crash_dump(reason: CrashReason, regs: Option<CpuRegisters>) {
    // Prevent re-entrant crash dump
    if CRASH_IN_PROGRESS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        serial_println!("!!! Crash dump already in progress, aborting re-entry");
        return;
    }

    serial_println!("!!! CRASH DUMP TRIGGERED !!!");

    let mut subsys = CRASH_DUMP.lock();
    let dump = subsys.perform_dump(reason, regs);

    // Print to serial console
    let formatted = CrashDumpSubsystem::format_dump(&dump);
    serial_println!("{}", formatted);

    subsys.last_dump = Some(dump);
    CRASH_IN_PROGRESS.store(false, Ordering::Release);
}

/// Trigger a crash dump with current register state (for user/debug use)
pub fn trigger_manual_dump() {
    let regs = CpuRegisters::capture();
    trigger_crash_dump(CrashReason::UserTriggered, Some(regs));
}

/// Get the last crash dump (if any)
pub fn last_dump_report() -> Option<String> {
    let subsys = CRASH_DUMP.lock();
    subsys
        .last_dump
        .as_ref()
        .map(|d| CrashDumpSubsystem::format_dump(d))
}

/// Check if a previous crash dump exists
pub fn has_crash_dump() -> bool {
    CRASH_DUMP.lock().last_dump.is_some()
}

/// Clear the stored crash dump
pub fn clear_dump() {
    CRASH_DUMP.lock().last_dump = None;
}

/// Register a memory region to watch during crash dumps
pub fn watch_region(addr: u64, size: usize, description: &str) {
    CRASH_DUMP.lock().watch_region(addr, size, description);
}

/// Configure whether to dump memory regions
pub fn set_dump_memory(enabled: bool) {
    CRASH_DUMP.lock().dump_memory = enabled;
}

/// Configure whether to capture kernel log
pub fn set_dump_log(enabled: bool) {
    CRASH_DUMP.lock().dump_log = enabled;
}

pub fn init() {
    let mut subsys = CRASH_DUMP.lock();

    // Reserve a memory region for crash dump storage
    // In a full implementation, this would be done early in boot before
    // the heap is set up, using a fixed physical address range
    match crate::memory::vmalloc::vmalloc(CRASH_DUMP_REGION_SIZE) {
        Some(ptr) => {
            subsys.reserved_region = ptr as u64;
            subsys.reserved_size = CRASH_DUMP_REGION_SIZE;
            // Zero-fill the region
            unsafe {
                core::ptr::write_bytes(ptr, 0, CRASH_DUMP_REGION_SIZE);
            }
        }
        None => {
            serial_println!("  [crash] WARNING: Could not reserve crash dump region");
        }
    }

    subsys.initialized = true;

    serial_println!(
        "  [crash] Crash dump initialized (reserved {} KB at {:#X})",
        subsys.reserved_size / 1024,
        subsys.reserved_region,
    );
}
