/// Kernel debugger for Genesis — breakpoints, stepping, inspection
///
/// Software/hardware breakpoints, single-step execution,
/// register/memory inspection, stack tracing.
///
/// Inspired by: GDB, LLDB, WinDbg. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Breakpoint type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakpointType {
    Software,
    Hardware,
    Watchpoint,
    Conditional,
}

/// Breakpoint state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakpointState {
    Enabled,
    Disabled,
    Deleted,
}

/// A breakpoint
pub struct Breakpoint {
    pub id: u32,
    pub bp_type: BreakpointType,
    pub address: u64,
    pub state: BreakpointState,
    pub hit_count: u64,
    pub original_byte: u8,
}

/// x86_64 register state
pub struct RegisterState {
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
    pub cs: u16,
    pub ss: u16,
    pub ds: u16,
    pub es: u16,
    pub fs: u16,
    pub gs: u16,
    pub cr0: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
}

impl RegisterState {
    pub fn new() -> Self {
        RegisterState {
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
            ds: 0,
            es: 0,
            fs: 0,
            gs: 0,
            cr0: 0,
            cr2: 0,
            cr3: 0,
            cr4: 0,
        }
    }

    pub fn dump(&self) -> String {
        format!(
            "RAX={:016x} RBX={:016x} RCX={:016x} RDX={:016x}\n\
             RSI={:016x} RDI={:016x} RBP={:016x} RSP={:016x}\n\
             R8 ={:016x} R9 ={:016x} R10={:016x} R11={:016x}\n\
             R12={:016x} R13={:016x} R14={:016x} R15={:016x}\n\
             RIP={:016x} RFLAGS={:016x}",
            self.rax,
            self.rbx,
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
            self.rflags
        )
    }
}

/// Stack frame for backtrace
pub struct StackFrame {
    pub return_addr: u64,
    pub frame_ptr: u64,
    pub depth: u32,
}

/// Debug session
pub struct DebugSession {
    pub active: bool,
    pub target_pid: u32,
    pub breakpoints: Vec<Breakpoint>,
    pub next_bp_id: u32,
    pub registers: RegisterState,
    pub single_stepping: bool,
    pub frames: Vec<StackFrame>,
}

impl DebugSession {
    const fn new() -> Self {
        DebugSession {
            active: false,
            target_pid: 0,
            breakpoints: Vec::new(),
            next_bp_id: 1,
            registers: RegisterState {
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
                ds: 0,
                es: 0,
                fs: 0,
                gs: 0,
                cr0: 0,
                cr2: 0,
                cr3: 0,
                cr4: 0,
            },
            single_stepping: false,
            frames: Vec::new(),
        }
    }

    pub fn attach(&mut self, pid: u32) {
        self.active = true;
        self.target_pid = pid;
        self.breakpoints.clear();
        self.single_stepping = false;
        crate::serial_println!("  [debugger] Attached to PID {}", pid);
    }

    pub fn detach(&mut self) {
        self.active = false;
        self.target_pid = 0;
        self.breakpoints.clear();
    }

    pub fn set_breakpoint(&mut self, address: u64, bp_type: BreakpointType) -> u32 {
        let id = self.next_bp_id;
        self.next_bp_id = self.next_bp_id.saturating_add(1);
        self.breakpoints.push(Breakpoint {
            id,
            bp_type,
            address,
            state: BreakpointState::Enabled,
            hit_count: 0,
            original_byte: 0,
        });
        id
    }

    pub fn remove_breakpoint(&mut self, id: u32) -> bool {
        if let Some(bp) = self.breakpoints.iter_mut().find(|b| b.id == id) {
            bp.state = BreakpointState::Deleted;
            true
        } else {
            false
        }
    }

    pub fn continue_exec(&mut self) {
        self.single_stepping = false;
    }

    pub fn step(&mut self) {
        self.single_stepping = true;
        // Set TF (trap flag) in RFLAGS
        self.registers.rflags |= 0x100;
    }

    pub fn step_over(&mut self) {
        // Set breakpoint at next instruction after current call
        let next_addr = self.registers.rip + 5; // assume 5-byte call
        self.set_breakpoint(next_addr, BreakpointType::Software);
        self.continue_exec();
    }

    pub fn read_memory(&self, addr: u64, size: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(size);
        for i in 0..size {
            let ptr = (addr + i as u64) as *const u8;
            unsafe {
                buf.push(core::ptr::read_volatile(ptr));
            }
        }
        buf
    }

    pub fn backtrace(&mut self) -> &[StackFrame] {
        self.frames.clear();
        let mut rbp = self.registers.rbp;
        let mut depth = 0u32;

        while rbp != 0 && depth < 64 {
            let frame_ptr = rbp;
            let ret_addr_ptr = (rbp + 8) as *const u64;
            let next_rbp_ptr = rbp as *const u64;

            let ret_addr = unsafe { core::ptr::read_volatile(ret_addr_ptr) };
            let next_rbp = unsafe { core::ptr::read_volatile(next_rbp_ptr) };

            self.frames.push(StackFrame {
                return_addr: ret_addr,
                frame_ptr,
                depth,
            });

            rbp = next_rbp;
            depth += 1;

            if next_rbp <= frame_ptr {
                break;
            }
        }
        &self.frames
    }

    pub fn breakpoint_count(&self) -> usize {
        self.breakpoints
            .iter()
            .filter(|b| b.state == BreakpointState::Enabled)
            .count()
    }
}

static DEBUGGER: Mutex<DebugSession> = Mutex::new(DebugSession::new());

pub fn init() {
    crate::serial_println!("  [debugger] Kernel debugger initialized");
}

pub fn attach(pid: u32) {
    DEBUGGER.lock().attach(pid);
}
pub fn detach() {
    DEBUGGER.lock().detach();
}
