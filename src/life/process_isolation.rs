use crate::serial_println;
use crate::sync::Mutex;

// ── Constants ────────────────────────────────────────────────────────────────
pub const MAX_PROCESSES: usize = 32;
pub const KERNEL_RING: u8 = 0;
pub const USER_RING: u8 = 3;
pub const GDT_KERNEL_CODE: u16 = 0x08;
pub const GDT_KERNEL_DATA: u16 = 0x10;
pub const GDT_USER_CODE: u16 = 0x1B;
pub const GDT_USER_DATA: u16 = 0x23;
pub const MSR_STAR: u32 = 0xC000_0081;
pub const MSR_LSTAR: u32 = 0xC000_0082;
pub const MSR_EFER: u32 = 0xC000_0080;

// SCE bit (bit 0) of EFER enables SYSCALL/SYSRET
const EFER_SCE: u64 = 1 << 0;

// ── Unsafe MSR / CPU register helpers ────────────────────────────────────────
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (hi as u64) << 32 | lo as u64
}

unsafe fn wrmsr(msr: u32, val: u64) {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, nomem)
    );
}

unsafe fn read_cs() -> u16 {
    let cs: u16;
    core::arch::asm!(
        "mov {0:x}, cs",
        out(reg) cs,
        options(nostack, nomem)
    );
    cs
}

unsafe fn read_cr3() -> u64 {
    let val: u64;
    core::arch::asm!(
        "mov {}, cr3",
        out(reg) val,
        options(nostack, nomem)
    );
    val
}

// ── ProcessState ─────────────────────────────────────────────────────────────
#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum ProcessState {
    Empty   = 0,
    Ready   = 1,
    Running = 2,
    Blocked = 3,
    Zombie  = 4,
}

// ── ProcessEntry ─────────────────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct ProcessEntry {
    pub pid:         u16,
    pub name:        [u8; 16],
    pub state:       ProcessState,
    pub ring:        u8,
    pub rip:         u64,
    pub rsp:         u64,
    pub cr3:         u64,
    pub ticks_run:   u32,
    pub trust_score: u16,  // 0–1000
}

impl ProcessEntry {
    pub const fn empty() -> Self {
        Self {
            pid:         0,
            name:        [0u8; 16],
            state:       ProcessState::Empty,
            ring:        0,
            rip:         0,
            rsp:         0,
            cr3:         0,
            ticks_run:   0,
            trust_score: 0,
        }
    }
}

// ── IsolationState ────────────────────────────────────────────────────────────
pub struct IsolationState {
    pub processes:       [ProcessEntry; MAX_PROCESSES],
    pub process_count:   usize,
    pub current_pid:     u16,
    pub ring_violations: u32,
    pub syscalls_served: u32,
    pub context_switches: u32,
    pub isolation_score: u16,  // 0–1000
    pub syscall_enabled: bool,
    pub gdt_loaded:      bool,
}

impl IsolationState {
    pub const fn new() -> Self {
        Self {
            processes:        [ProcessEntry::empty(); MAX_PROCESSES],
            process_count:    0,
            current_pid:      0,
            ring_violations:  0,
            syscalls_served:  0,
            context_switches: 0,
            isolation_score:  0,
            syscall_enabled:  false,
            gdt_loaded:       false,
        }
    }
}

// ── Static state ──────────────────────────────────────────────────────────────
pub static STATE: Mutex<IsolationState> = Mutex::new(IsolationState::new());

// ── Internal: find empty slot ─────────────────────────────────────────────────
fn find_empty_slot(st: &IsolationState) -> Option<usize> {
    for i in 0..MAX_PROCESSES {
        if st.processes[i].state == ProcessState::Empty {
            return Some(i);
        }
    }
    None
}

// ── Internal: find process by pid ────────────────────────────────────────────
fn find_by_pid(st: &mut IsolationState, pid: u16) -> Option<usize> {
    for i in 0..MAX_PROCESSES {
        if st.processes[i].state != ProcessState::Empty && st.processes[i].pid == pid {
            return Some(i);
        }
    }
    None
}

// ── Internal: next available PID ─────────────────────────────────────────────
fn next_pid(st: &IsolationState) -> u16 {
    // Scan for the highest pid currently in use, return +1 (capped at u16::MAX)
    let mut max: u16 = 0;
    for i in 0..MAX_PROCESSES {
        if st.processes[i].state != ProcessState::Empty {
            if st.processes[i].pid > max {
                max = st.processes[i].pid;
            }
        }
    }
    max.saturating_add(1)
}

// ── init() ────────────────────────────────────────────────────────────────────
pub fn init() {
    // Read hardware state before taking the lock so the lock is held minimally.
    let cs_val = unsafe { read_cs() };
    let current_ring = (cs_val & 0x3) as u8;

    let efer = unsafe { rdmsr(MSR_EFER) };
    let already_enabled = (efer & EFER_SCE) != 0;

    if !already_enabled {
        // Enable SCE bit
        unsafe { wrmsr(MSR_EFER, efer | EFER_SCE) };
    }

    // MSR_STAR layout (for SYSCALL/SYSRET):
    //   bits 63:48 = SYSRET CS/SS selectors  (user code base)
    //   bits 47:32 = SYSCALL CS/SS selectors (kernel code base)
    // GDT_USER_CODE = 0x1B → user ring base = 0x18 (strip RPL)
    // GDT_KERNEL_CODE = 0x08
    let star_val: u64 = ((GDT_USER_CODE as u64 & !0x3) << 48)
        | ((GDT_KERNEL_CODE as u64) << 32);
    unsafe { wrmsr(MSR_STAR, star_val) };

    let cr3_val = unsafe { read_cr3() };
    let syscall_on = !already_enabled || (efer & EFER_SCE != 0);

    let mut st = STATE.lock();

    // PID 0 — ANIMA kernel process
    if let Some(slot) = find_empty_slot(&st) {
        let mut name = [0u8; 16];
        let label = b"ANIMA";
        let copy_len = if label.len() < 16 { label.len() } else { 16 };
        let mut i = 0;
        while i < copy_len {
            name[i] = label[i];
            i += 1;
        }
        st.processes[slot] = ProcessEntry {
            pid:         0,
            name,
            state:       ProcessState::Running,
            ring:        KERNEL_RING,
            rip:         0,   // kernel entry — not tracked here
            rsp:         0,
            cr3:         cr3_val,
            ticks_run:   0,
            trust_score: 1000,
        };
        st.process_count = st.process_count.saturating_add(1);
    }

    st.current_pid      = 0;
    st.gdt_loaded       = true;
    st.syscall_enabled  = syscall_on;
    st.isolation_score  = 800;

    serial_println!(
        "[isolation] ANIMA process isolation online — ring={} pid=0 syscall={}",
        current_ring,
        syscall_on
    );
}

// ── register_process() ────────────────────────────────────────────────────────
pub fn register_process(name: &[u8], ring: u8, rip: u64, rsp: u64) -> u16 {
    let cr3_val = unsafe { read_cr3() };

    let mut st = STATE.lock();
    let slot = match find_empty_slot(&st) {
        Some(s) => s,
        None => {
            serial_println!("[isolation] register_process: process table full");
            return u16::MAX;
        }
    };

    let pid = next_pid(&st);

    let mut entry_name = [0u8; 16];
    let copy_len = if name.len() < 16 { name.len() } else { 16 };
    let mut i = 0;
    while i < copy_len {
        entry_name[i] = name[i];
        i += 1;
    }

    let trust_score = if ring == KERNEL_RING { 1000 } else { 400 };

    st.processes[slot] = ProcessEntry {
        pid,
        name: entry_name,
        state: ProcessState::Ready,
        ring,
        rip,
        rsp,
        cr3: cr3_val,
        ticks_run: 0,
        trust_score,
    };
    st.process_count = st.process_count.saturating_add(1);

    serial_println!("[isolation] process registered pid={} ring={}", pid, ring);
    pid
}

// ── context_switch() ──────────────────────────────────────────────────────────
pub fn context_switch(to_pid: u16) {
    let mut st = STATE.lock();
    let current = st.current_pid;

    // Mark the previous process as Ready (if it exists and is Running)
    if let Some(idx) = find_by_pid(&mut st, current) {
        if st.processes[idx].state == ProcessState::Running {
            st.processes[idx].state = ProcessState::Ready;
        }
    }

    // Activate the target process
    if let Some(idx) = find_by_pid(&mut st, to_pid) {
        st.processes[idx].state = ProcessState::Running;
    }

    st.current_pid = to_pid;
    st.context_switches = st.context_switches.saturating_add(1);
}

// ── report_violation() ────────────────────────────────────────────────────────
pub fn report_violation(pid: u16) {
    let mut st = STATE.lock();
    st.ring_violations = st.ring_violations.saturating_add(1);
    let violations = st.ring_violations;

    if let Some(idx) = find_by_pid(&mut st, pid) {
        st.processes[idx].trust_score =
            st.processes[idx].trust_score.saturating_sub(200);
    }

    serial_println!(
        "[ISOLATION_WARN] ring violation pid={} violations={}",
        pid,
        violations
    );
}

// ── tick() ────────────────────────────────────────────────────────────────────
pub fn tick(consciousness: u16, age: u32) {
    // Every 10 ticks: verify current privilege ring
    if age % 10 == 0 {
        let cs_val = unsafe { read_cs() };
        let actual_ring = (cs_val & 0x3) as u8;

        let expected_ring = {
            let st = STATE.lock();
            let pid = st.current_pid;
            // Find the expected ring for the current process
            let mut r: u8 = KERNEL_RING;
            let mut i = 0;
            while i < MAX_PROCESSES {
                if st.processes[i].state != ProcessState::Empty
                    && st.processes[i].pid == pid
                {
                    r = st.processes[i].ring;
                    break;
                }
                i += 1;
            }
            r
        };

        if actual_ring != expected_ring {
            let pid = STATE.lock().current_pid;
            report_violation(pid);
        }
    }

    // Increment ticks_run for the currently running process
    {
        let mut st = STATE.lock();
        let pid = st.current_pid;
        if let Some(idx) = find_by_pid(&mut st, pid) {
            st.processes[idx].ticks_run =
                st.processes[idx].ticks_run.saturating_add(1);
        }

        // isolation_score: 800 base, +200 bonus for zero violations
        let no_violations: u16 = if st.ring_violations == 0 { 1 } else { 0 };
        st.isolation_score = 800_u16.saturating_add(no_violations.saturating_mul(200));

        // Periodic report every 400 ticks
        if age % 400 == 0 {
            let procs    = st.process_count;
            let viols    = st.ring_violations;
            let switches = st.context_switches;
            let score    = st.isolation_score;
            // Drop lock before serial_println to avoid holding it during I/O
            drop(st);
            serial_println!(
                "[isolation] procs={} violations={} switches={} score={}",
                procs,
                viols,
                switches,
                score
            );
        }
    }

    // Suppress unused-parameter warning
    let _ = consciousness;
}

// ── Getters ───────────────────────────────────────────────────────────────────
pub fn isolation_score() -> u16 {
    STATE.lock().isolation_score
}

pub fn ring_violations() -> u32 {
    STATE.lock().ring_violations
}

pub fn context_switches() -> u32 {
    STATE.lock().context_switches
}

pub fn process_count() -> usize {
    STATE.lock().process_count
}

pub fn syscall_enabled() -> bool {
    STATE.lock().syscall_enabled
}
