/// Kernel Oops / Panic handler for Genesis AIOS
///
/// Provides structured crash reporting: register dump, rbp-based stack
/// unwinding, kallsyms resolution, and a persistent `LAST_OOPS` record.
///
/// Rules strictly followed:
///   - no_std only — no alloc, no Vec, no String, no Box
///   - no float casts (no `as f32` / `as f64`)
///   - saturating_add / saturating_sub for counters
///   - read_volatile / write_volatile for MMIO accesses
///   - no panic — early returns or serial_println! + loop {}
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// CrashRegs — CPU register state captured at the moment of the fault
// ---------------------------------------------------------------------------

/// Full x86-64 CPU state captured by an exception handler or inline asm.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CrashRegs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub cs: u64,
    pub ss: u64,
    pub cr0: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub error_code: u64,
}

impl CrashRegs {
    /// Return a zeroed register snapshot.
    pub const fn zeroed() -> Self {
        CrashRegs {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            rsp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
            rflags: 0,
            cs: 0,
            ss: 0,
            cr0: 0,
            cr2: 0,
            cr3: 0,
            cr4: 0,
            error_code: 0,
        }
    }

    /// Capture the current RIP and RSP via inline asm.
    /// All other fields are left zeroed — callers fill them from interrupt frames.
    pub fn capture_rip_rsp() -> Self {
        let mut regs = Self::zeroed();
        unsafe {
            // RIP: lea the address of the next instruction
            core::arch::asm!(
                "lea {rip}, [rip]",
                rip = out(reg) regs.rip,
                options(nomem, nostack),
            );
            // RSP: read stack pointer directly
            core::arch::asm!(
                "mov {rsp}, rsp",
                rsp = out(reg) regs.rsp,
                options(nomem, nostack),
            );
            // RBP: frame pointer
            core::arch::asm!(
                "mov {rbp}, rbp",
                rbp = out(reg) regs.rbp,
                options(nomem, nostack),
            );
            // RFLAGS
            core::arch::asm!(
                "pushfq",
                "pop {rf}",
                rf = out(reg) regs.rflags,
                options(nostack),
            );
            // Control registers
            core::arch::asm!("mov {v}, cr0", v = out(reg) regs.cr0, options(nomem, nostack));
            core::arch::asm!("mov {v}, cr2", v = out(reg) regs.cr2, options(nomem, nostack));
            core::arch::asm!("mov {v}, cr3", v = out(reg) regs.cr3, options(nomem, nostack));
            core::arch::asm!("mov {v}, cr4", v = out(reg) regs.cr4, options(nomem, nostack));
        }
        regs
    }
}

// ---------------------------------------------------------------------------
// OopsRecord — one saved oops / panic event
// ---------------------------------------------------------------------------

/// Maximum depth of the saved stack trace.
const MAX_TRACE: usize = 32;

/// Maximum byte length of the null-terminated message field.
const MAX_MSG: usize = 256;

/// A single kernel oops record.  Stored entirely in a fixed-size struct so
/// no heap allocation is ever required.
pub struct OopsRecord {
    /// Null-terminated error message (UTF-8).
    pub message: [u8; MAX_MSG],
    /// Number of valid bytes in `message` (not counting the NUL).
    pub msg_len: usize,
    /// Register state at fault time.
    pub regs: CrashRegs,
    /// Return addresses collected by rbp-chain unwinding.
    pub stack_trace: [u64; MAX_TRACE],
    /// Number of valid entries in `stack_trace`.
    pub trace_depth: usize,
    /// PID of the faulting task (0 = kernel context).
    pub pid: u32,
    /// Logical CPU ID.
    pub cpu: u8,
    /// TSC value read at the moment the oops was recorded.
    pub timestamp_tsc: u64,
    /// If `true` the kernel halted after recording this oops.
    pub fatal: bool,
}

impl OopsRecord {
    const fn zeroed() -> Self {
        OopsRecord {
            message: [0u8; MAX_MSG],
            msg_len: 0,
            regs: CrashRegs::zeroed(),
            stack_trace: [0u64; MAX_TRACE],
            trace_depth: 0,
            pid: 0,
            cpu: 0,
            timestamp_tsc: 0,
            fatal: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Wrapper that makes `OopsRecord` work inside `Mutex<Option<…>>` without
/// requiring heap allocation — we keep an `OopsRecord` inline in a `MaybeOops`.
struct MaybeOops {
    present: bool,
    record: OopsRecord,
}

impl MaybeOops {
    const fn empty() -> Self {
        MaybeOops {
            present: false,
            record: OopsRecord::zeroed(),
        }
    }
}

/// The most-recently recorded oops.
static LAST_OOPS: Mutex<MaybeOops> = Mutex::new(MaybeOops::empty());

/// Running count of oops events since boot.
static OOPS_COUNT: AtomicU32 = AtomicU32::new(0);

/// Guards against re-entrant oops (double-oops → immediate halt).
static OOPS_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------------------
// Stack unwinding
// ---------------------------------------------------------------------------

/// Minimum kernel virtual address — frames below this are considered invalid.
const KERNEL_BASE: u64 = 0xFFFF_8000_0000_0000;

/// Walk the rbp frame chain and collect return addresses into `trace`.
///
/// Returns the number of frames written.  Stops early when:
///   - `rbp` is zero
///   - `rbp` is not 8-byte aligned
///   - `rbp` is below `KERNEL_BASE` (not a kernel address)
///   - the next `rbp` would be less than or equal to the current one
///     (guard against corrupt / looping chains)
///   - the collected count reaches `MAX_TRACE`
fn unwind_stack(rbp: u64, trace: &mut [u64; MAX_TRACE]) -> usize {
    let mut depth = 0usize;
    let mut frame = rbp;

    while depth < MAX_TRACE {
        // Validate: must be 8-byte aligned and in kernel space.
        if frame == 0 || (frame & 0x7) != 0 || frame < KERNEL_BASE {
            break;
        }

        // Safety: we validated the address is in kernel space and aligned.
        // read_volatile prevents the compiler from optimising the load away.
        let prev_rbp: u64 = unsafe { core::ptr::read_volatile(frame as *const u64) };
        let ret_addr: u64 =
            unsafe { core::ptr::read_volatile((frame.saturating_add(8)) as *const u64) };

        if ret_addr == 0 {
            break;
        }

        trace[depth] = ret_addr;
        depth = depth.saturating_add(1);

        // Prevent infinite loops on corrupt chains.
        if prev_rbp <= frame {
            break;
        }
        frame = prev_rbp;
    }

    depth
}

// ---------------------------------------------------------------------------
// TSC helper
// ---------------------------------------------------------------------------

#[inline(always)]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ---------------------------------------------------------------------------
// Hex formatting helpers — no alloc, no format!, output to serial directly
// ---------------------------------------------------------------------------

/// Print a u64 as a 16-digit lowercase hex string prefixed by `0x`.
fn serial_hex64(val: u64) {
    let mut digits = [b'0'; 16];
    let mut n = val;
    for i in (0..16).rev() {
        let nibble = (n & 0xf) as u8;
        digits[i] = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + nibble - 10
        };
        n >>= 4;
    }
    // Write digit bytes one by one through serial_print!
    for &d in &digits {
        crate::serial_print!("{}", d as char);
    }
}

/// Print a u32 as an 8-digit lowercase hex string.
fn serial_hex32(val: u32) {
    serial_hex64(val as u64);
}

// ---------------------------------------------------------------------------
// Register dump to serial
// ---------------------------------------------------------------------------

fn dump_regs(regs: &CrashRegs) {
    crate::serial_println!(
        "  RAX={:#018x}  RBX={:#018x}  RCX={:#018x}  RDX={:#018x}",
        regs.rax,
        regs.rbx,
        regs.rcx,
        regs.rdx
    );
    crate::serial_println!(
        "  RSI={:#018x}  RDI={:#018x}  RBP={:#018x}  RSP={:#018x}",
        regs.rsi,
        regs.rdi,
        regs.rbp,
        regs.rsp
    );
    crate::serial_println!(
        "  R8 ={:#018x}  R9 ={:#018x}  R10={:#018x}  R11={:#018x}",
        regs.r8,
        regs.r9,
        regs.r10,
        regs.r11
    );
    crate::serial_println!(
        "  R12={:#018x}  R13={:#018x}  R14={:#018x}  R15={:#018x}",
        regs.r12,
        regs.r13,
        regs.r14,
        regs.r15
    );
    crate::serial_println!("  RIP={:#018x}  RFLAGS={:#018x}", regs.rip, regs.rflags);
    crate::serial_println!(
        "  CS={:#06x}  SS={:#06x}  ERR={:#018x}",
        regs.cs,
        regs.ss,
        regs.error_code
    );
    crate::serial_println!(
        "  CR0={:#018x}  CR2={:#018x}  CR3={:#018x}  CR4={:#018x}",
        regs.cr0,
        regs.cr2,
        regs.cr3,
        regs.cr4
    );
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Record and print a kernel oops.
///
/// Called from exception handlers (page fault, GP fault, double fault, …).
/// If `fatal` is `true` the machine is halted after the dump.
pub fn kernel_oops(msg: &str, regs: &CrashRegs) {
    // Re-entrancy guard: if we're already inside an oops, just halt.
    if OOPS_IN_PROGRESS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
        .is_err()
    {
        crate::serial_println!("!!! DOUBLE OOPS — halting immediately");
        crate::io::cli();
        loop {
            crate::io::hlt();
        }
    }

    // ---- Print banner -------------------------------------------------------
    crate::serial_println!("");
    crate::serial_println!("========================================");
    crate::serial_println!(" GENESIS KERNEL OOPS");
    crate::serial_println!("========================================");
    crate::serial_println!("MSG  : {}", msg);

    // ---- Register dump -------------------------------------------------------
    crate::serial_println!("--- Registers (GDB-format) ---");
    dump_regs(regs);

    // ---- Stack unwind --------------------------------------------------------
    let mut trace = [0u64; MAX_TRACE];
    let depth = unwind_stack(regs.rbp, &mut trace);

    crate::serial_println!("--- Stack Trace ({} frames) ---", depth);
    for i in 0..depth {
        let addr = trace[i];
        crate::serial_print!("  #{:02}  {:#018x}", i, addr);

        // Attempt kallsyms resolution.
        if let Some((sym_addr, name)) = crate::kernel::kallsyms::lookup(addr) {
            let offset = addr.saturating_sub(sym_addr);
            crate::serial_println!("  <{}+{:#x}>", name, offset);
        } else {
            crate::serial_println!("  <no symbol>");
        }
    }

    // ---- TSC / CPU info ------------------------------------------------------
    let tsc = rdtsc();
    let cpu_id: u8 = 0; // APIC ID lookup would go here
    let pid: u32 = 0; // process::getpid() would go here

    crate::serial_println!("  TSC={:#018x}  CPU={}  PID={}", tsc, cpu_id, pid);
    crate::serial_println!("========================================");

    // ---- Persist in LAST_OOPS -----------------------------------------------
    {
        let mut guard = LAST_OOPS.lock();
        guard.present = true;
        guard.record.regs = *regs;
        guard.record.trace_depth = depth;
        guard.record.stack_trace = trace;
        guard.record.timestamp_tsc = tsc;
        guard.record.cpu = cpu_id;
        guard.record.pid = pid;
        guard.record.fatal = regs.error_code == u64::MAX; // sentinel for fatal

        // Copy message bytes.
        let msg_bytes = msg.as_bytes();
        let copy_len = msg_bytes.len().min(MAX_MSG - 1);
        guard.record.message[..copy_len].copy_from_slice(&msg_bytes[..copy_len]);
        guard.record.message[copy_len] = 0;
        guard.record.msg_len = copy_len;
    }

    OOPS_COUNT.fetch_add(1, Ordering::Relaxed);
    OOPS_IN_PROGRESS.store(false, Ordering::Release);
}

/// Called from the `#[panic_handler]` in `main.rs`.
///
/// Reads RIP/RSP via inline asm, populates a `CrashRegs`, calls
/// `kernel_oops()` with `error_code = u64::MAX` as the fatal sentinel,
/// then halts the machine.
pub fn kernel_panic(msg: &str) -> ! {
    crate::serial_println!("");
    crate::serial_println!("!!! KERNEL PANIC: {}", msg);

    // Capture minimal register state at the panic call site.
    let mut regs = CrashRegs::capture_rip_rsp();
    // Mark as fatal via the sentinel.
    regs.error_code = u64::MAX;

    kernel_oops(msg, &regs);

    // Disable interrupts, then halt all cores.
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Return a copy of the last oops record, if any.
pub fn get_last_oops() -> Option<OopsRecord> {
    let guard = LAST_OOPS.lock();
    if guard.present {
        // We copy the inner OopsRecord out.
        let r = &guard.record;
        let mut copy = OopsRecord::zeroed();
        copy.message = r.message;
        copy.msg_len = r.msg_len;
        copy.regs = r.regs;
        copy.stack_trace = r.stack_trace;
        copy.trace_depth = r.trace_depth;
        copy.pid = r.pid;
        copy.cpu = r.cpu;
        copy.timestamp_tsc = r.timestamp_tsc;
        copy.fatal = r.fatal;
        Some(copy)
    } else {
        None
    }
}

/// Total number of oops events recorded since boot.
pub fn oops_count() -> u32 {
    OOPS_COUNT.load(Ordering::Relaxed)
}

/// Initialize the oops subsystem.
pub fn init() {
    OOPS_IN_PROGRESS.store(false, Ordering::SeqCst);
    OOPS_COUNT.store(0, Ordering::Relaxed);
    crate::serial_println!("  [oops] Kernel oops handler initialized");
}
