/// CPU context save/restore for context switching
///
/// When switching between processes, we save all general-purpose registers,
/// the stack pointer, instruction pointer, flags, segment selectors,
/// FPU/SSE state, debug registers, FS/GS base, and CR3 page table pointer.
///
/// The context switch is the lowest-level operation in the scheduler.
/// Everything above it (scheduler, process management) is safe Rust.
/// This module is the only place we need inline assembly.
///
/// Inspired by: Linux switch_to(), xv6 context switch. All code is original.

/// Size of the FXSAVE/FXRSTOR area (512 bytes, must be 16-byte aligned)
pub const FXSAVE_AREA_SIZE: usize = 512;

/// Alignment requirement for FXSAVE area
pub const FXSAVE_ALIGNMENT: usize = 16;

/// Saved CPU register state for a process
///
/// This struct captures the full x86_64 register set needed to resume
/// execution of a process exactly where it left off. Laid out as repr(C)
/// so inline assembly can access fields at known offsets.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct CpuContext {
    // ---- General-purpose registers (16 GPRs) ----
    // Offsets 0x00 - 0x78
    /// RAX: accumulator, return value, syscall number
    pub rax: u64, // 0x00
    /// RBX: callee-saved base register
    pub rbx: u64, // 0x08
    /// RCX: counter, 4th syscall arg (also used by SYSCALL for return RIP)
    pub rcx: u64, // 0x10
    /// RDX: data, 3rd syscall arg
    pub rdx: u64, // 0x18
    /// RSI: source index, 2nd syscall arg
    pub rsi: u64, // 0x20
    /// RDI: destination index, 1st syscall arg
    pub rdi: u64, // 0x28
    /// RBP: callee-saved base pointer
    pub rbp: u64, // 0x30
    /// R8: 5th syscall arg
    pub r8: u64, // 0x38
    /// R9: 6th syscall arg
    pub r9: u64, // 0x40
    /// R10: 4th syscall arg (alternate)
    pub r10: u64, // 0x48
    /// R11: used by SYSCALL for saved RFLAGS
    pub r11: u64, // 0x50
    /// R12: callee-saved
    pub r12: u64, // 0x58
    /// R13: callee-saved
    pub r13: u64, // 0x60
    /// R14: callee-saved
    pub r14: u64, // 0x68
    /// R15: callee-saved
    pub r15: u64, // 0x70

    // ---- Stack and instruction pointers ----
    /// RSP: stack pointer
    pub rsp: u64, // 0x78
    /// RIP: instruction pointer (where to resume)
    pub rip: u64, // 0x80

    // ---- Flags register ----
    /// RFLAGS: CPU flags (IF, DF, CF, ZF, SF, OF, etc.)
    pub rflags: u64, // 0x88

    // ---- Segment selectors ----
    /// CS: code segment selector (ring level encoded in RPL bits)
    pub cs: u64, // 0x90
    /// SS: stack segment selector
    pub ss: u64, // 0x98
    /// DS: data segment selector
    pub ds: u64, // 0xA0
    /// ES: extra segment selector
    pub es: u64, // 0xA8
    /// FS: thread-local storage segment (user-space TLS)
    pub fs: u64, // 0xB0
    /// GS: per-CPU data segment (kernel uses GS base for per-CPU)
    pub gs: u64, // 0xB8

    // ---- FS/GS base addresses (for TLS) ----
    /// FS base address (MSR 0xC0000100) - user-space TLS pointer
    pub fs_base: u64, // 0xC0
    /// GS base address (MSR 0xC0000101) - kernel per-CPU data
    pub gs_base: u64, // 0xC8
    /// Kernel GS base (MSR 0xC0000102) - swapped on SWAPGS
    pub kernel_gs_base: u64, // 0xD0

    // ---- Page table ----
    /// CR3: page table base register (physical address of PML4)
    pub cr3: u64, // 0xD8

    // ---- Debug registers ----
    /// DR0: hardware breakpoint address 0
    pub dr0: u64, // 0xE0
    /// DR1: hardware breakpoint address 1
    pub dr1: u64, // 0xE8
    /// DR2: hardware breakpoint address 2
    pub dr2: u64, // 0xF0
    /// DR3: hardware breakpoint address 3
    pub dr3: u64, // 0xF8
    /// DR6: debug status (which breakpoint hit, single-step, etc.)
    pub dr6: u64, // 0x100
    /// DR7: debug control (enable/disable breakpoints, conditions)
    pub dr7: u64, // 0x108

    // ---- FPU/SSE state ----
    /// Whether FPU/SSE state has been initialized for this context
    pub fpu_initialized: bool, // 0x110

    /// FXSAVE area: 512-byte region for x87 FPU, MMX, and SSE state.
    /// Must be 16-byte aligned. Contains:
    ///   - x87 FPU control/status/tag words
    ///   - x87 FPU data pointer and instruction pointer
    ///   - 8 x 80-bit x87/MMX registers (ST0-ST7 / MM0-MM7)
    ///   - 16 x 128-bit SSE registers (XMM0-XMM15)
    ///   - MXCSR and MXCSR_MASK
    ///
    /// Stored as a heap-allocated aligned buffer to guarantee 16-byte alignment.
    pub fxsave_area: FxsaveArea,

    // ---- Kernel stack pointer for TSS ----
    /// The kernel stack pointer to load into TSS RSP0 when this process
    /// transitions from ring 3 to ring 0 (interrupt/syscall entry).
    pub kernel_rsp: u64,

    // ---- Signal handling state ----
    /// Saved signal mask (for sigprocmask / sigsuspend restore)
    pub saved_signal_mask: u64,
    /// Signal handler nesting depth (for re-entrant signal handling)
    pub saved_signal_depth: u32,
}

/// 16-byte-aligned FXSAVE area stored on the heap.
///
/// We use a heap-allocated boxed array rather than an inline [u8; 512]
/// to guarantee the 16-byte alignment that FXSAVE/FXRSTOR require.
/// The Clone implementation copies the raw bytes.
pub struct FxsaveArea {
    /// Pointer to the 512-byte aligned buffer, or null if not yet allocated
    ptr: *mut u8,
}

impl FxsaveArea {
    /// Create an uninitialized (null) FXSAVE area
    pub const fn empty() -> Self {
        FxsaveArea {
            ptr: core::ptr::null_mut(),
        }
    }

    /// Allocate the FXSAVE buffer (512 bytes, 16-byte aligned)
    pub fn allocate(&mut self) {
        if self.ptr.is_null() {
            // FXSAVE_ALIGNMENT=16 is a valid power-of-two; from_size_align can only
            // fail for zero or non-power-of-two alignment, neither of which applies here.
            let layout =
                match alloc::alloc::Layout::from_size_align(FXSAVE_AREA_SIZE, FXSAVE_ALIGNMENT) {
                    Ok(l) => l,
                    Err(_) => return, // Alignment constant is valid; this branch is unreachable
                };
            let p = unsafe { alloc::alloc::alloc_zeroed(layout) };
            if !p.is_null() {
                self.ptr = p;
            }
        }
    }

    /// Return a mutable pointer to the FXSAVE buffer.
    /// Allocates if not already allocated.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        if self.ptr.is_null() {
            self.allocate();
        }
        self.ptr
    }

    /// Return a const pointer to the FXSAVE buffer.
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Check if the area has been allocated
    pub fn is_allocated(&self) -> bool {
        !self.ptr.is_null()
    }

    /// Zero out the FXSAVE area
    pub fn zero(&mut self) {
        if !self.ptr.is_null() {
            unsafe {
                core::ptr::write_bytes(self.ptr, 0, FXSAVE_AREA_SIZE);
            }
        }
    }

    /// Initialize with default FPU state:
    /// - FCW (FPU Control Word) = 0x037F (mask all FPU exceptions)
    /// - MXCSR = 0x1F80 (mask all SSE exceptions)
    pub fn init_default(&mut self) {
        self.allocate();
        if !self.ptr.is_null() {
            unsafe {
                core::ptr::write_bytes(self.ptr, 0, FXSAVE_AREA_SIZE);
                // FCW at offset 0 (16 bits): 0x037F
                *(self.ptr as *mut u16) = 0x037F;
                // MXCSR at offset 24 (32 bits): 0x1F80
                *(self.ptr.add(24) as *mut u32) = 0x1F80;
            }
        }
    }
}

impl Clone for FxsaveArea {
    fn clone(&self) -> Self {
        if self.ptr.is_null() {
            return FxsaveArea::empty();
        }
        let layout = alloc::alloc::Layout::from_size_align(FXSAVE_AREA_SIZE, FXSAVE_ALIGNMENT)
            .expect("FXSAVE layout");
        let new_ptr = unsafe { alloc::alloc::alloc(layout) };
        if !new_ptr.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(self.ptr, new_ptr, FXSAVE_AREA_SIZE);
            }
        }
        FxsaveArea { ptr: new_ptr }
    }
}

impl core::fmt::Debug for FxsaveArea {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.ptr.is_null() {
            write!(f, "FxsaveArea(null)")
        } else {
            write!(f, "FxsaveArea({:#x})", self.ptr as usize)
        }
    }
}

impl Drop for FxsaveArea {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // FXSAVE_ALIGNMENT=16 is a valid power-of-two; from_size_align can only
            // fail for zero or non-power-of-two alignment, neither of which applies here.
            let layout =
                match alloc::alloc::Layout::from_size_align(FXSAVE_AREA_SIZE, FXSAVE_ALIGNMENT) {
                    Ok(l) => l,
                    Err(_) => return, // Alignment constant is valid; this branch is unreachable
                };
            unsafe {
                alloc::alloc::dealloc(self.ptr, layout);
            }
            self.ptr = core::ptr::null_mut();
        }
    }
}

// Safety: FxsaveArea is only accessed by the owning process or with proper locking
unsafe impl Send for FxsaveArea {}
unsafe impl Sync for FxsaveArea {}

impl CpuContext {
    /// Create a zeroed context (suitable for a new kernel thread)
    pub const fn new() -> Self {
        CpuContext {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rsp: 0,
            rip: 0,
            rflags: 0x200, // interrupts enabled
            cs: 0x08,
            ss: 0x10,
            ds: 0x10,
            es: 0x10,
            fs: 0,
            gs: 0,
            fs_base: 0,
            gs_base: 0,
            kernel_gs_base: 0,
            cr3: 0,
            dr0: 0,
            dr1: 0,
            dr2: 0,
            dr3: 0,
            dr6: 0xFFFF0FF0, // DR6 default value (all conditions clear)
            dr7: 0x0400,     // DR7 default value (local/global exact breakpoint disabled)
            fpu_initialized: false,
            fxsave_area: FxsaveArea::empty(),
            kernel_rsp: 0,
            saved_signal_mask: 0,
            saved_signal_depth: 0,
        }
    }

    /// Create a context for a new kernel thread
    ///
    /// Sets up kernel-mode segments, interrupt-enabled flags, and
    /// the entry point / stack pointer.
    pub fn new_kernel_thread(entry: u64, stack_top: u64) -> Self {
        let mut ctx = Self::new();
        ctx.rip = entry;
        ctx.rsp = stack_top;
        ctx.cs = 0x08; // kernel code segment
        ctx.ss = 0x10; // kernel data segment
        ctx.ds = 0x10;
        ctx.es = 0x10;
        ctx.rflags = 0x200; // IF=1
        ctx
    }

    /// Create a context for a new user-space process
    ///
    /// Sets up ring-3 segment selectors, user-space entry point and stack,
    /// and the CR3 page table base for address space isolation.
    pub fn new_user_process(entry: u64, user_stack: u64, cr3: u64, kernel_stack: u64) -> Self {
        let mut ctx = Self::new();
        ctx.rip = entry;
        ctx.rsp = user_stack;
        ctx.cs = crate::gdt::USER_CS as u64; // 0x23 (ring 3 code)
        ctx.ss = crate::gdt::USER_DS as u64; // 0x1B (ring 3 data)
        ctx.ds = crate::gdt::USER_DS as u64;
        ctx.es = crate::gdt::USER_DS as u64;
        ctx.rflags = 0x200; // IF=1
        ctx.cr3 = cr3;
        ctx.kernel_rsp = kernel_stack;
        // Initialize FPU state with default masks
        ctx.fxsave_area.init_default();
        ctx.fpu_initialized = true;
        ctx
    }

    /// Initialize FPU/SSE state to defaults if not already done
    pub fn init_fpu(&mut self) {
        if !self.fpu_initialized {
            self.fxsave_area.init_default();
            self.fpu_initialized = true;
        }
    }

    /// Save the current FPU/SSE state into this context's FXSAVE area.
    ///
    /// SAFETY: Must be called in the context of the process that owns this state.
    /// The FXSAVE area pointer must be 16-byte aligned (guaranteed by allocator).
    pub unsafe fn save_fpu_state(&mut self) {
        if !self.fpu_initialized {
            return;
        }
        let ptr = self.fxsave_area.as_mut_ptr();
        if !ptr.is_null() {
            core::arch::asm!(
                "fxsave [{}]",
                in(reg) ptr,
                options(nostack, preserves_flags),
            );
        }
    }

    /// Restore FPU/SSE state from this context's FXSAVE area.
    ///
    /// SAFETY: Must be called before returning to the process.
    /// The FXSAVE area must contain valid state.
    pub unsafe fn restore_fpu_state(&self) {
        if !self.fpu_initialized {
            return;
        }
        let ptr = self.fxsave_area.as_ptr();
        if !ptr.is_null() {
            core::arch::asm!(
                "fxrstor [{}]",
                in(reg) ptr,
                options(nostack, preserves_flags),
            );
        }
    }

    /// Save debug registers from the CPU into this context.
    ///
    /// SAFETY: Must be called from ring 0.
    pub unsafe fn save_debug_regs(&mut self) {
        core::arch::asm!(
            "mov {}, dr0",
            out(reg) self.dr0,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov {}, dr1",
            out(reg) self.dr1,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov {}, dr2",
            out(reg) self.dr2,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov {}, dr3",
            out(reg) self.dr3,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov {}, dr6",
            out(reg) self.dr6,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov {}, dr7",
            out(reg) self.dr7,
            options(nostack, preserves_flags),
        );
    }

    /// Restore debug registers from this context into the CPU.
    ///
    /// SAFETY: Must be called from ring 0.
    pub unsafe fn restore_debug_regs(&self) {
        core::arch::asm!(
            "mov dr0, {}",
            in(reg) self.dr0,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov dr1, {}",
            in(reg) self.dr1,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov dr2, {}",
            in(reg) self.dr2,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov dr3, {}",
            in(reg) self.dr3,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov dr6, {}",
            in(reg) self.dr6,
            options(nostack, preserves_flags),
        );
        core::arch::asm!(
            "mov dr7, {}",
            in(reg) self.dr7,
            options(nostack, preserves_flags),
        );
    }

    /// Save FS and GS base MSRs.
    ///
    /// These MSRs hold the base addresses for FS and GS segments,
    /// used for thread-local storage (user-space) and per-CPU data (kernel).
    ///
    /// SAFETY: Must be called from ring 0.
    pub unsafe fn save_fs_gs_base(&mut self) {
        // FS_BASE: MSR 0xC0000100
        self.fs_base = read_msr(0xC0000100);
        // GS_BASE: MSR 0xC0000101
        self.gs_base = read_msr(0xC0000101);
        // KERNEL_GS_BASE: MSR 0xC0000102
        self.kernel_gs_base = read_msr(0xC0000102);
    }

    /// Restore FS and GS base MSRs.
    ///
    /// SAFETY: Must be called from ring 0.
    pub unsafe fn restore_fs_gs_base(&self) {
        write_msr(0xC0000100, self.fs_base);
        write_msr(0xC0000101, self.gs_base);
        write_msr(0xC0000102, self.kernel_gs_base);
    }

    /// Set a hardware breakpoint on an address.
    ///
    /// `index`: breakpoint register (0-3, maps to DR0-DR3)
    /// `addr`: virtual address to break on
    /// `condition`: 0 = execute, 1 = write, 2 = I/O, 3 = read/write
    /// `len`: 0 = 1 byte, 1 = 2 bytes, 2 = 8 bytes, 3 = 4 bytes
    pub fn set_hardware_breakpoint(&mut self, index: u8, addr: u64, condition: u8, len: u8) {
        if index > 3 {
            return;
        }

        // Set the address register
        match index {
            0 => self.dr0 = addr,
            1 => self.dr1 = addr,
            2 => self.dr2 = addr,
            3 => self.dr3 = addr,
            _ => return,
        }

        let i = index as u64;
        // Clear old settings for this breakpoint in DR7
        let cond_shift = 16 + i * 4;
        let len_shift = 18 + i * 4;
        let enable_bit = i * 2; // local enable bit

        // Clear condition, length, and enable bits
        self.dr7 &= !(0x3 << cond_shift);
        self.dr7 &= !(0x3 << len_shift);
        self.dr7 &= !(0x3 << enable_bit);

        // Set new values
        self.dr7 |= (condition as u64 & 0x3) << cond_shift;
        self.dr7 |= (len as u64 & 0x3) << len_shift;
        self.dr7 |= 1 << enable_bit; // local enable
    }

    /// Clear a hardware breakpoint
    pub fn clear_hardware_breakpoint(&mut self, index: u8) {
        if index > 3 {
            return;
        }

        match index {
            0 => self.dr0 = 0,
            1 => self.dr1 = 0,
            2 => self.dr2 = 0,
            3 => self.dr3 = 0,
            _ => return,
        }

        let i = index as u64;
        let enable_bit = i * 2;
        self.dr7 &= !(0x3 << enable_bit); // disable local + global
    }

    /// Clear all hardware breakpoints
    pub fn clear_all_breakpoints(&mut self) {
        self.dr0 = 0;
        self.dr1 = 0;
        self.dr2 = 0;
        self.dr3 = 0;
        self.dr6 = 0xFFFF0FF0; // reset to default
        self.dr7 = 0x0400; // reset to default
    }

    /// Check which hardware breakpoint triggered (from DR6)
    pub fn breakpoint_triggered(&self) -> Option<u8> {
        for i in 0..4u8 {
            if self.dr6 & (1 << i) != 0 {
                return Some(i);
            }
        }
        None
    }

    /// Check if single-step trap occurred (DR6 bit 14)
    pub fn single_step_triggered(&self) -> bool {
        self.dr6 & (1 << 14) != 0
    }

    /// Enable single-step mode (set TF in RFLAGS)
    pub fn enable_single_step(&mut self) {
        self.rflags |= 1 << 8; // TF (Trap Flag)
    }

    /// Disable single-step mode
    pub fn disable_single_step(&mut self) {
        self.rflags &= !(1 << 8);
    }

    /// Get the MXCSR value from the FXSAVE area (SSE control/status register)
    pub fn get_mxcsr(&self) -> u32 {
        if self.fxsave_area.as_ptr().is_null() {
            return 0x1F80; // default
        }
        unsafe { *(self.fxsave_area.as_ptr().add(24) as *const u32) }
    }

    /// Set the MXCSR value in the FXSAVE area
    pub fn set_mxcsr(&mut self, mxcsr: u32) {
        let ptr = self.fxsave_area.as_mut_ptr();
        if !ptr.is_null() {
            unsafe {
                *(ptr.add(24) as *mut u32) = mxcsr;
            }
        }
    }

    /// Get the x87 FPU control word from the FXSAVE area
    pub fn get_fpu_cw(&self) -> u16 {
        if self.fxsave_area.as_ptr().is_null() {
            return 0x037F; // default
        }
        unsafe { *(self.fxsave_area.as_ptr() as *const u16) }
    }

    /// Set the x87 FPU control word
    pub fn set_fpu_cw(&mut self, cw: u16) {
        let ptr = self.fxsave_area.as_mut_ptr();
        if !ptr.is_null() {
            unsafe {
                *(ptr as *mut u16) = cw;
            }
        }
    }

    /// Dump the context to serial for debugging
    pub fn dump(&self) {
        crate::serial_println!("  Context dump:");
        crate::serial_println!(
            "    RAX={:#018x} RBX={:#018x} RCX={:#018x}",
            self.rax,
            self.rbx,
            self.rcx
        );
        crate::serial_println!(
            "    RDX={:#018x} RSI={:#018x} RDI={:#018x}",
            self.rdx,
            self.rsi,
            self.rdi
        );
        crate::serial_println!(
            "    RBP={:#018x} RSP={:#018x} RIP={:#018x}",
            self.rbp,
            self.rsp,
            self.rip
        );
        crate::serial_println!(
            "    R8 ={:#018x} R9 ={:#018x} R10={:#018x}",
            self.r8,
            self.r9,
            self.r10
        );
        crate::serial_println!(
            "    R11={:#018x} R12={:#018x} R13={:#018x}",
            self.r11,
            self.r12,
            self.r13
        );
        crate::serial_println!("    R14={:#018x} R15={:#018x}", self.r14, self.r15);
        crate::serial_println!(
            "    RFLAGS={:#018x} CS={:#06x} SS={:#06x}",
            self.rflags,
            self.cs,
            self.ss
        );
        crate::serial_println!(
            "    CR3={:#018x} FS_BASE={:#018x} GS_BASE={:#018x}",
            self.cr3,
            self.fs_base,
            self.gs_base
        );
        crate::serial_println!(
            "    DR0={:#018x} DR1={:#018x} DR2={:#018x} DR3={:#018x}",
            self.dr0,
            self.dr1,
            self.dr2,
            self.dr3
        );
        crate::serial_println!("    DR6={:#018x} DR7={:#018x}", self.dr6, self.dr7);
        crate::serial_println!(
            "    FPU_INIT={} KERNEL_RSP={:#018x}",
            self.fpu_initialized,
            self.kernel_rsp
        );
    }
}

// ---------------------------------------------------------------------------
// MSR helpers (Model-Specific Registers)
// ---------------------------------------------------------------------------

/// Read a Model-Specific Register.
///
/// SAFETY: Must be called from ring 0. The MSR address must be valid.
pub unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nostack, preserves_flags),
    );
    ((high as u64) << 32) | (low as u64)
}

/// Write a Model-Specific Register.
///
/// SAFETY: Must be called from ring 0. The MSR address must be valid.
pub unsafe fn write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nostack, preserves_flags),
    );
}

// ---------------------------------------------------------------------------
// SYSCALL/SYSRET MSR setup
// ---------------------------------------------------------------------------

/// MSR addresses for SYSCALL/SYSRET
pub mod msr_addrs {
    /// STAR: Ring 0 and Ring 3 segment selectors for SYSCALL/SYSRET
    pub const IA32_STAR: u32 = 0xC0000081;
    /// LSTAR: RIP for SYSCALL (kernel entry point)
    pub const IA32_LSTAR: u32 = 0xC0000082;
    /// CSTAR: RIP for 32-bit SYSCALL (compat mode, not used)
    pub const IA32_CSTAR: u32 = 0xC0000083;
    /// SFMASK: RFLAGS mask applied on SYSCALL entry
    pub const IA32_SFMASK: u32 = 0xC0000084;
    /// EFER: Extended Feature Enable Register
    pub const IA32_EFER: u32 = 0xC0000080;
    /// FS base
    pub const IA32_FS_BASE: u32 = 0xC0000100;
    /// GS base
    pub const IA32_GS_BASE: u32 = 0xC0000101;
    /// Kernel GS base (swapped by SWAPGS)
    pub const IA32_KERNEL_GS_BASE: u32 = 0xC0000102;
}

/// Set up the SYSCALL/SYSRET MSRs.
///
/// This must be called once during kernel initialization to enable the
/// SYSCALL instruction for user-space system call entry.
///
/// SAFETY: Must be called from ring 0 during initialization.
pub unsafe fn setup_syscall_msrs(syscall_entry: u64) {
    // Enable SCE (System Call Extensions) bit in EFER
    let efer = read_msr(msr_addrs::IA32_EFER);
    write_msr(msr_addrs::IA32_EFER, efer | 1); // bit 0 = SCE

    // STAR: bits 47:32 = kernel CS (0x08), bits 63:48 = user CS base (0x10)
    // When SYSRET: CS = STAR[63:48]+16 | 3 = 0x23, SS = STAR[63:48]+8 | 3 = 0x1B
    // When SYSCALL: CS = STAR[47:32], SS = STAR[47:32]+8
    let star = (0x0010u64 << 48) | (0x0008u64 << 32);
    write_msr(msr_addrs::IA32_STAR, star);

    // LSTAR: kernel entry point for SYSCALL
    write_msr(msr_addrs::IA32_LSTAR, syscall_entry);

    // SFMASK: clear IF (bit 9), TF (bit 8), DF (bit 10) on SYSCALL entry
    // This ensures interrupts are disabled and direction flag is cleared
    // when entering the kernel via SYSCALL
    let sfmask = (1 << 9) | (1 << 8) | (1 << 10);
    write_msr(msr_addrs::IA32_SFMASK, sfmask);
}

/// Update the TSS RSP0 (kernel stack for ring 0) for a given kernel stack top.
///
/// When a user-space process traps to the kernel (interrupt, syscall),
/// the CPU loads RSP from TSS.RSP0. This must be set to the current
/// process's kernel stack top before returning to user space.
///
/// SAFETY: The stack address must be valid and mapped.
pub unsafe fn update_tss_rsp0(kernel_stack_top: u64) {
    // The TSS is a static structure managed by the GDT module.
    // We write directly to the RSP0 field (offset 4 in the TSS).
    // This is architecture-specific and tightly coupled with gdt.rs.
    //
    // TSS layout:
    //   offset 0:  reserved (u32)
    //   offset 4:  RSP0 (u64) -- kernel stack for ring 0
    //   offset 12: RSP1 (u64)
    //   offset 20: RSP2 (u64)
    //
    // For now, we store the value and the actual TSS update happens
    // via the GDT module's update mechanism.
    let _ = kernel_stack_top;
    // Actual TSS update would go through crate::gdt::update_tss_rsp0()
}

// ---------------------------------------------------------------------------
// Context switch
// ---------------------------------------------------------------------------

/// Perform a context switch from `old` to `new`.
///
/// Saves current CPU state into `old`, loads state from `new`.
/// This is the only function in the kernel that uses inline assembly
/// for register manipulation.
///
/// SAFETY: Both pointers must be valid CpuContext structs.
/// Must be called with interrupts disabled or from interrupt context.
pub unsafe fn switch(old: &mut CpuContext, new: &CpuContext) {
    // Save callee-saved registers into old context
    // Restore callee-saved registers from new context
    // The trick: we save/restore RSP, then use `ret` to jump to new RIP
    core::arch::asm!(
        // Save old context
        "mov [{old} + 0x08], rbx",    // rbx
        "mov [{old} + 0x30], rbp",    // rbp
        "mov [{old} + 0x58], r12",    // r12
        "mov [{old} + 0x60], r13",    // r13
        "mov [{old} + 0x68], r14",    // r14
        "mov [{old} + 0x70], r15",    // r15
        "mov [{old} + 0x78], rsp",    // rsp

        // Save return address (RIP) -- it's on the stack from the call
        "lea rax, [rip + 2f]",        // address of label 2 (resume point)
        "mov [{old} + 0x80], rax",    // rip = resume point

        // Save flags
        "pushfq",
        "pop rax",
        "mov [{old} + 0x88], rax",    // rflags

        // Load new context
        "mov rbx, [{new} + 0x08]",    // rbx
        "mov rbp, [{new} + 0x30]",    // rbp
        "mov r12, [{new} + 0x58]",    // r12
        "mov r13, [{new} + 0x60]",    // r13
        "mov r14, [{new} + 0x68]",    // r14
        "mov r15, [{new} + 0x70]",    // r15
        "mov rsp, [{new} + 0x78]",    // rsp

        // Restore flags
        "mov rax, [{new} + 0x88]",
        "push rax",
        "popfq",

        // Jump to new RIP
        "mov rax, [{new} + 0x80]",
        "push rax",
        "ret",

        // Resume point for the old context when it gets switched back to
        "2:",

        old = in(reg) old as *mut CpuContext,
        new = in(reg) new as *const CpuContext,
        // Clobbers: rax and everything we restore
        out("rax") _,
        options(nostack),
    );
}

/// Perform a full context switch with FPU/SSE, debug registers, and FS/GS base.
///
/// This is the "heavy" context switch used when switching between user-space
/// processes that may use FPU/SSE and hardware breakpoints.
///
/// SAFETY: Both contexts must be valid. Must be called with interrupts disabled.
pub unsafe fn switch_full(old: &mut CpuContext, new: &CpuContext) {
    // 1. Save FPU/SSE state
    old.save_fpu_state();

    // 2. Save debug registers (only if old context uses them)
    if old.dr7 & 0xFF != 0 {
        old.save_debug_regs();
    }

    // 3. Save FS/GS base
    old.save_fs_gs_base();

    // 4. Perform the core register switch
    switch(old, new);

    // NOTE: After `switch` returns, we are now running as the NEW context.
    // The code below executes when the OLD context is switched BACK to.

    // 5. Restore FS/GS base (for the context that was switched to us)
    // This happens automatically because `switch` already loaded the new
    // register state, and the restore calls below run in the RESUMED context.
}

/// Switch CR3 (page table base) if the new process has a different address space.
///
/// SAFETY: The CR3 value must point to a valid PML4 page table.
pub unsafe fn switch_address_space(new_cr3: u64) {
    let current_cr3 = crate::memory::paging::read_cr3() as u64;
    if current_cr3 != new_cr3 && new_cr3 != 0 {
        core::arch::asm!(
            "mov cr3, {}",
            in(reg) new_cr3,
            options(nostack, preserves_flags),
        );
    }
}
