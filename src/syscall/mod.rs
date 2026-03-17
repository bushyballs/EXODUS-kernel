/// System call interface for Genesis
///
/// Uses the x86_64 SYSCALL/SYSRET mechanism via Model-Specific Registers.
/// User-space programs invoke syscalls via the `syscall` instruction.
///
/// Syscall convention (matches Linux for familiarity):
///   RAX = syscall number
///   RDI = arg1, RSI = arg2, RDX = arg3, R10 = arg4, R8 = arg5, R9 = arg6
///   RAX = return value
///
/// Domain modules:
///   file_ops   -- read/write/open/close/stat/lseek/dup/pipe/fcntl/getdents
///   proc_ops   -- fork/waitpid/kill/futex/uid/session management/time
///   mem_ops    -- mmap/munmap/mprotect/brk/mlock
///   net_ops    -- socket/bind/listen/connect/accept/send/recv (stubbed)
///   neural_ops -- neural_pulse/neural_poll
///
/// All code is original.
pub mod file_ops;
pub mod mem_ops;
pub mod net_ops;
pub mod neural_ops;
pub mod proc_ops;

use file_ops::*;
use mem_ops::*;
use net_ops::*;
use neural_ops::*;
use proc_ops::*;

use crate::process;
use crate::sync::Mutex;
use crate::{kprint, kprintln, serial_print, serial_println};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Shared types (referenced by domain modules via `super::`)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KernelFd {
    PipeRead(usize),
    PipeWrite(usize),
    File(u64),
}

pub(super) struct ProcessFdState {
    next_fd_by_pid: BTreeMap<u32, u32>,
    pub fd_map: BTreeMap<(u32, u32), KernelFd>,
}

impl ProcessFdState {
    pub const fn new() -> Self {
        Self {
            next_fd_by_pid: BTreeMap::new(),
            fd_map: BTreeMap::new(),
        }
    }

    pub fn alloc_fd(&mut self, pid: u32, kind: KernelFd) -> u32 {
        let next = self.next_fd_by_pid.entry(pid).or_insert(3);
        let fd = *next;
        *next = next.saturating_add(1);
        self.fd_map.insert((pid, fd), kind);
        fd
    }

    pub fn get_fd(&self, pid: u32, fd: u32) -> Option<KernelFd> {
        self.fd_map.get(&(pid, fd)).copied()
    }

    pub fn set_fd(&mut self, pid: u32, fd: u32, kind: KernelFd) -> Option<KernelFd> {
        let previous = self.fd_map.insert((pid, fd), kind);
        let next = self.next_fd_by_pid.entry(pid).or_insert(3);
        if fd >= *next {
            *next = fd.saturating_add(1);
        }
        previous
    }

    pub fn remove_fd(&mut self, pid: u32, fd: u32) -> Option<KernelFd> {
        self.fd_map.remove(&(pid, fd))
    }

    pub fn has_kind(&self, kind: KernelFd) -> bool {
        self.fd_map.values().any(|&mapped| mapped == kind)
    }

    pub fn clone_pid_fds(&mut self, src_pid: u32, dst_pid: u32) {
        let mut entries = Vec::new();
        let mut max_fd = 2u32;
        for (&(p, fd), &kind) in self.fd_map.iter() {
            if p == src_pid {
                entries.push((fd, kind));
                if fd > max_fd {
                    max_fd = fd;
                }
            }
        }
        for (fd, kind) in entries {
            self.fd_map.insert((dst_pid, fd), kind);
        }
        let src_next = self
            .next_fd_by_pid
            .get(&src_pid)
            .copied()
            .unwrap_or_else(|| max_fd.saturating_add(1));
        self.next_fd_by_pid
            .insert(dst_pid, core::cmp::max(src_next, max_fd.saturating_add(1)));
    }

    pub fn pid_fds(&self, pid: u32) -> Vec<u32> {
        self.fd_map
            .keys()
            .filter_map(|&(owner, fd)| if owner == pid { Some(fd) } else { None })
            .collect()
    }

    pub fn remove_pid(&mut self, pid: u32) {
        self.next_fd_by_pid.remove(&pid);
    }
}

pub(super) static PROCESS_FDS: Mutex<ProcessFdState> = Mutex::new(ProcessFdState::new());

#[derive(Clone, Debug)]
pub(super) struct OpenFile {
    pub path: String,
    pub flags: u32,
    pub offset: u64,
}

pub(super) struct OpenFileTable {
    pub next_id: u64,
    pub files: BTreeMap<u64, OpenFile>,
}

impl OpenFileTable {
    pub const fn new() -> Self {
        Self {
            next_id: 1,
            files: BTreeMap::new(),
        }
    }
    pub fn insert(&mut self, file: OpenFile) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.files.insert(id, file);
        id
    }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut OpenFile> {
        self.files.get_mut(&id)
    }
    pub fn remove(&mut self, id: u64) {
        self.files.remove(&id);
    }
}

pub(super) static OPEN_FILES: Mutex<OpenFileTable> = Mutex::new(OpenFileTable::new());

pub(super) fn close_kernel_fd(kind: KernelFd) {
    match kind {
        KernelFd::PipeRead(id) => {
            let _ = crate::ipc::pipe::close_read(id);
        }
        KernelFd::PipeWrite(id) => {
            let _ = crate::ipc::pipe::close_write(id);
        }
        KernelFd::File(id) => {
            OPEN_FILES.lock().remove(id);
        }
    }
}

pub(super) fn close_local_fd_mapping(pid: u32, fd: u32) -> bool {
    let (removed, to_close) = {
        let mut table = PROCESS_FDS.lock();
        match table.remove_fd(pid, fd) {
            Some(kind) => {
                let tc = if table.has_kind(kind) {
                    None
                } else {
                    Some(kind)
                };
                (true, tc)
            }
            None => (false, None),
        }
    };
    if let Some(kind) = to_close {
        close_kernel_fd(kind);
    }
    removed
}

pub(super) fn access_mode(flags: u32) -> u32 {
    flags & 0x3
}

pub(super) fn can_read_flags(flags: u32) -> bool {
    matches!(
        access_mode(flags),
        crate::fs::vfs::flags::O_RDONLY | crate::fs::vfs::flags::O_RDWR
    )
}

pub(super) fn can_write_flags(flags: u32) -> bool {
    matches!(
        access_mode(flags),
        crate::fs::vfs::flags::O_WRONLY | crate::fs::vfs::flags::O_RDWR
    )
}

pub(super) fn read_user_path(path_ptr: *const u8, len: usize) -> Result<String, ()> {
    if path_ptr.is_null() || len == 0 || len > 4096 {
        return Err(());
    }
    let slice = unsafe { core::slice::from_raw_parts(path_ptr, len) };
    Ok(String::from(core::str::from_utf8(slice).map_err(|_| ())?))
}

pub(super) fn resolve_process_path(pid: u32, raw_path: &str) -> String {
    if raw_path.starts_with('/') {
        return String::from(raw_path);
    }
    let cwd = {
        if pid as usize >= process::MAX_PROCESSES {
            return String::from(raw_path);
        }
        let table = process::pcb::PROCESS_TABLE.lock();
        table[pid as usize]
            .as_ref()
            .map(|p| p.cwd.clone())
            .unwrap_or_else(|| String::from("/"))
    };
    if cwd == "/" {
        alloc::format!("/{}", raw_path)
    } else {
        alloc::format!("{}/{}", cwd, raw_path)
    }
}

// ---------------------------------------------------------------------------
// Public cleanup helper (called from the process exit path)
// ---------------------------------------------------------------------------

pub fn cleanup_process_fds(pid: u32) {
    let fds = {
        let table = PROCESS_FDS.lock();
        table.pid_fds(pid)
    };
    for fd in fds {
        let _ = close_local_fd_mapping(pid, fd);
    }
    PROCESS_FDS.lock().remove_pid(pid);
}

// ---------------------------------------------------------------------------
// Syscall number constants
// ---------------------------------------------------------------------------

pub mod nr {
    pub const SYS_EXIT: u64 = 0;
    pub const SYS_WRITE: u64 = 1;
    pub const SYS_YIELD: u64 = 2;
    pub const SYS_GETPID: u64 = 3;
    pub const SYS_SPAWN: u64 = 4;
    pub const SYS_SLEEP: u64 = 5;
    pub const SYS_FORK: u64 = 6;
    pub const SYS_WAITPID: u64 = 7;
    pub const SYS_KILL: u64 = 8;
    pub const SYS_GETPPID: u64 = 9;
    pub const SYS_MMAP: u64 = 10;
    pub const SYS_MUNMAP: u64 = 11;
    pub const SYS_MPROTECT: u64 = 74;
    pub const SYS_MADVISE: u64 = 75;
    pub const SYS_READ: u64 = 12;
    pub const SYS_OPEN: u64 = 13;
    pub const SYS_CLOSE: u64 = 14;
    pub const SYS_PIPE: u64 = 15;
    pub const SYS_DUP2: u64 = 16;
    pub const SYS_EXEC: u64 = 17;
    pub const SYS_BRK: u64 = 18;
    pub const SYS_SOCKET: u64 = 19;
    pub const SYS_BIND: u64 = 20;
    pub const SYS_LISTEN: u64 = 21;
    pub const SYS_ACCEPT: u64 = 22;
    pub const SYS_CONNECT: u64 = 23;
    pub const SYS_SEND: u64 = 24;
    pub const SYS_RECV: u64 = 25;
    pub const SYS_FUTEX: u64 = 26;
    pub const SYS_CLONE: u64 = 27;
    pub const SYS_SIGACTION: u64 = 28;
    pub const SYS_SIGRETURN: u64 = 29;
    pub const SYS_GETCWD: u64 = 30;
    pub const SYS_CHDIR: u64 = 31;
    pub const SYS_STAT: u64 = 32;
    pub const SYS_LSEEK: u64 = 33;
    pub const SYS_GETDENTS: u64 = 34;
    pub const SYS_IOCTL: u64 = 35;
    pub const SYS_POLL: u64 = 36;
    pub const SYS_GETUID: u64 = 37;
    pub const SYS_GETGID: u64 = 38;
    pub const SYS_SETUID: u64 = 39;
    pub const SYS_SETGID: u64 = 40;
    pub const SYS_SETSID: u64 = 41;
    pub const SYS_GETSID: u64 = 42;
    pub const SYS_GETPGID: u64 = 43;
    pub const SYS_SETPGID: u64 = 44;
    pub const SYS_TIME: u64 = 45;
    pub const SYS_CLOCK_GETTIME: u64 = 46;
    pub const SYS_NANOSLEEP: u64 = 47;
    pub const SYS_SHUTDOWN: u64 = 48;
    pub const SYS_REBOOT: u64 = 49;
    pub const SYS_MOUNT: u64 = 50;
    pub const SYS_UMOUNT: u64 = 51;
    pub const SYS_FSTAT: u64 = 52;
    pub const SYS_DUP: u64 = 53;
    pub const SYS_MKDIR: u64 = 55;
    pub const SYS_RMDIR: u64 = 56;
    pub const SYS_UNLINK: u64 = 57;
    pub const SYS_RENAME: u64 = 58;
    pub const SYS_CHMOD: u64 = 59;
    pub const SYS_CHOWN: u64 = 60;
    pub const SYS_UNAME: u64 = 61;
    pub const SYS_FCNTL: u64 = 62;
    pub const SYS_GETDENTS64: u64 = 63;
    pub const SYS_SELECT: u64 = 64;
    pub const SYS_SETHOSTNAME: u64 = 65;
    pub const SYS_GETHOSTNAME: u64 = 66;
    pub const SYS_TRUNCATE: u64 = 67;
    pub const SYS_FTRUNCATE: u64 = 68;
    pub const SYS_SYMLINK: u64 = 69;
    pub const SYS_READLINK: u64 = 70;
    pub const SYS_UMASK: u64 = 71;
    pub const SYS_GETRUSAGE: u64 = 72;
    pub const SYS_SYSINFO: u64 = 73;
    pub const SYS_NEURAL_PULSE: u64 = 500;
    pub const SYS_NEURAL_POLL: u64 = 501;
    pub const SYS_GETRANDOM: u64 = 318;
    pub const SYS_MEMFD_CREATE: u64 = 319;

    // --- POSIX Message Queues ---
    /// mq_open(name_ptr, name_len, flags, max_msg, max_msgsize) -> mqfd
    pub const SYS_MQ_OPEN: u64 = 240;
    /// mq_send(mqfd, data_ptr, data_len, priority) -> 0
    pub const SYS_MQ_SEND: u64 = 241;
    /// mq_receive(mqfd, buf_ptr, buf_len, priority_out_ptr) -> bytes
    pub const SYS_MQ_RECEIVE: u64 = 243;
    /// mq_unlink(name_ptr, name_len) -> 0
    pub const SYS_MQ_UNLINK: u64 = 244;
    /// mq_getattr(mqfd) -> packed (max_msg | max_msgsize<<32 etc.) — stub
    pub const SYS_MQ_GETATTR: u64 = 245;
    /// mq_notify(mqfd, pid, signal) -> 0
    pub const SYS_MQ_NOTIFY: u64 = 246;
    /// mq_close(mqfd) -> 0
    pub const SYS_MQ_CLOSE: u64 = 247;

    // --- POSIX / System V Shared Memory ---
    /// shm_open(name_ptr, name_len, flags, mode) -> shmfd
    pub const SYS_SHM_OPEN: u64 = 511;
    /// shm_close(shmfd)
    pub const SYS_SHM_CLOSE: u64 = 512;
    /// shm_unlink(name_ptr, name_len) -> 0
    pub const SYS_SHM_UNLINK: u64 = 513;
    /// shm_read(shmfd, offset, buf_ptr, len) -> bytes
    pub const SYS_SHM_READ: u64 = 514;
    /// shm_write(shmfd, offset, buf_ptr, len) -> bytes
    pub const SYS_SHM_WRITE: u64 = 515;
    /// shm_truncate(shmfd, new_size) -> 0
    pub const SYS_SHM_TRUNCATE: u64 = 516;
    /// shmget(key, size, flags) -> shmid
    pub const SYS_SHMGET: u64 = 517;
    /// shmat(shmid, addr, flags) -> ptr
    pub const SYS_SHMAT: u64 = 518;
    /// shmdt(addr) -> 0
    pub const SYS_SHMDT: u64 = 519;
    /// shmctl(shmid, cmd, buf_ptr) -> 0
    pub const SYS_SHMCTL: u64 = 520;

    // --- Kernel Module Loading ---
    /// load_module(elf_ptr, elf_len, name_ptr, name_len) -> 0
    pub const SYS_LOAD_MODULE: u64 = 530;
    /// unload_module(name_ptr, name_len) -> 0
    pub const SYS_UNLOAD_MODULE: u64 = 531;

    // --- TUN/TAP virtual network interfaces ---
    /// tun_open(name_ptr, name_len, flags) -> fd
    ///   flags: 0 = TUN (IP-level), 1 = TAP (Ethernet-level)
    pub const SYS_TUN_OPEN: u64 = 540;
    /// tun_read(fd, buf_ptr) -> bytes_read  (buf must be 1500 bytes)
    pub const SYS_TUN_READ: u64 = 541;
    /// tun_write(fd, buf_ptr, len) -> bytes_written
    pub const SYS_TUN_WRITE: u64 = 542;

    // --- POSIX Real-Time Signals ---
    // NOTE: Linux-native numbers 13 and 14 conflict with SYS_OPEN/SYS_CLOSE in
    // this kernel's numbering scheme.  Genesis assigns free numbers in the
    // 126-132 range to avoid collisions.
    /// rt_sigaction(signo, new_act_ptr, old_act_ptr, sigsetsize) -> 0
    pub const SYS_RT_SIGACTION: u64 = 130;
    /// rt_sigprocmask(how, set_ptr, oldset_ptr, sigsetsize) -> 0
    pub const SYS_RT_SIGPROCMASK: u64 = 131;
    /// rt_sigpending(set_ptr, sigsetsize) -> 0
    pub const SYS_RT_SIGPENDING: u64 = 127;
    /// rt_sigqueueinfo(pid, signo, siginfo_ptr) -> 0
    pub const SYS_RT_SIGQUEUEINFO: u64 = 129;

    // --- POSIX Timers (Linux-compatible numbers) ---
    /// timer_create(clock_id, sigevent_ptr, timer_id_out) -> 0
    pub const SYS_TIMER_CREATE: u64 = 222;
    /// timer_settime(timer_id, flags, new_value_ptr, old_value_ptr) -> 0
    pub const SYS_TIMER_SETTIME: u64 = 223;
    /// timer_gettime(timer_id, cur_value_ptr) -> 0
    pub const SYS_TIMER_GETTIME: u64 = 224;
    /// timer_getoverrun(timer_id) -> overrun_count
    pub const SYS_TIMER_GETOVERRUN: u64 = 225;
    /// timer_delete(timer_id) -> 0
    pub const SYS_TIMER_DELETE: u64 = 226;

    // --- Namespace management (Linux-compatible numbers) ---
    /// unshare(flags) -> 0
    /// Disassociate parts of the process execution context specified
    /// by `flags` (CLONE_NEWPID, CLONE_NEWNS, CLONE_NEWNET, etc.)
    /// and place the calling process into fresh namespaces.
    pub const SYS_UNSHARE: u64 = 272;

    /// setns(fd, nstype) -> 0
    /// Reassociate the calling thread with a namespace represented by
    /// the file descriptor `fd`. `nstype` constrains which namespace
    /// type the fd may refer to (0 = any).
    pub const SYS_SETNS: u64 = 308;

    // --- PTY (pseudo-terminal) ---
    /// pty_open() -> master_fd
    /// Allocate a new PTY master/slave pair.  Returns the master fd, or
    /// -ENFILE if the MAX_PTYS limit has been reached.
    pub const SYS_PTY_OPEN: u64 = 550;
    /// pty_get_slave_fd(master_fd) -> slave_fd
    /// Return the slave fd corresponding to the given master fd.
    pub const SYS_PTY_SLAVE: u64 = 551;
    /// pty_close_master(master_fd) -> 0
    pub const SYS_PTY_CLOSE_MASTER: u64 = 552;
    /// pty_close_slave(slave_fd) -> 0
    pub const SYS_PTY_CLOSE_SLAVE: u64 = 553;
    /// pty_set_winsize(master_fd, rows, cols) -> 0
    pub const SYS_PTY_SETWINSZ: u64 = 554;
    /// pty_get_slave_name(master_fd, buf_ptr, buf_len) -> name_len
    pub const SYS_PTY_SLAVENAME: u64 = 555;
}

// ---------------------------------------------------------------------------
// POSIX error codes
// ---------------------------------------------------------------------------

pub mod errno {
    pub const EPERM: u64 = 0xFFFF_FFFF_FFFF_FFFF;
    pub const ENOENT: u64 = 0xFFFF_FFFF_FFFF_FFFE;
    pub const ESRCH: u64 = 0xFFFF_FFFF_FFFF_FFFD;
    pub const EINTR: u64 = 0xFFFF_FFFF_FFFF_FFFC;
    pub const EIO: u64 = 0xFFFF_FFFF_FFFF_FFFB;
    pub const ENOMEM: u64 = 0xFFFF_FFFF_FFFF_FFF4;
    pub const EACCES: u64 = 0xFFFF_FFFF_FFFF_FFF3;
    pub const EFAULT: u64 = 0xFFFF_FFFF_FFFF_FFF2;
    pub const EEXIST: u64 = 0xFFFF_FFFF_FFFF_FFEF;
    pub const ENOTDIR: u64 = 0xFFFF_FFFF_FFFF_FFEC;
    pub const EISDIR: u64 = 0xFFFF_FFFF_FFFF_FFEB;
    pub const EINVAL: u64 = 0xFFFF_FFFF_FFFF_FFEA;
    pub const ENFILE: u64 = 0xFFFF_FFFF_FFFF_FFE9;
    pub const EMFILE: u64 = 0xFFFF_FFFF_FFFF_FFE8;
    pub const ENOSPC: u64 = 0xFFFF_FFFF_FFFF_FFE4;
    pub const EPIPE: u64 = 0xFFFF_FFFF_FFFF_FFE0;
    pub const ERANGE: u64 = 0xFFFF_FFFF_FFFF_FFDE;
    pub const ENOSYS: u64 = 0xFFFF_FFFF_FFFF_FFD8;
    pub const ENOTEMPTY: u64 = 0xFFFF_FFFF_FFFF_FFD7;
    pub const EAGAIN: u64 = 0xFFFF_FFFF_FFFF_FFF5;
    pub const EBADF: u64 = 0xFFFF_FFFF_FFFF_FFF7;
    pub const ENAMETOOLONG: u64 = 0xFFFF_FFFF_FFFF_FFD4;
    pub const EAFNOSUPPORT: u64 = 0xFFFF_FFFF_FFFF_FFB4;
}

// ---------------------------------------------------------------------------
// MSR helpers
// ---------------------------------------------------------------------------

mod msr {
    pub const STAR: u32 = 0xC000_0081;
    pub const LSTAR: u32 = 0xC000_0082;
    pub const SFMASK: u32 = 0xC000_0084;
}

unsafe fn wrmsr(reg: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") reg, in("eax") lo, in("edx") hi,
        options(nomem, nostack),
    );
}

/// Initialize the SYSCALL/SYSRET mechanism via MSRs.
pub fn init() {
    unsafe {
        let star = (0x08u64 << 32) | (0x10u64 << 48);
        wrmsr(msr::STAR, star);
        wrmsr(msr::LSTAR, syscall_entry as *const () as u64);
        wrmsr(msr::SFMASK, 0x200);
    }
    serial_println!("  Syscall: SYSCALL/SYSRET configured via MSRs");
}

// ---------------------------------------------------------------------------
// Assembly SYSCALL entry stub
// ---------------------------------------------------------------------------

#[unsafe(naked)]
extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        "push rcx", "push r11", "push rbp", "push rbx",
        "push r12", "push r13", "push r14", "push r15",
        "mov rcx, r10",
        "call {handler}",
        "pop r15", "pop r14", "pop r13", "pop r12",
        "pop rbx", "pop rbp", "pop r11", "pop rcx",
        "sysretq",
        handler = sym syscall_dispatch,
    );
}

// ---------------------------------------------------------------------------
// User-pointer validation helpers
// ---------------------------------------------------------------------------

const USER_ADDR_MAX: usize = 0x0000_7FFF_FFFF_FFFF;

#[inline(always)]
fn validate_user_ptr(ptr: usize, len: usize) -> Result<(), u64> {
    if ptr == 0 || ptr > USER_ADDR_MAX {
        return Err(errno::EFAULT);
    }
    if ptr.saturating_add(len) > USER_ADDR_MAX {
        return Err(errno::EFAULT);
    }
    Ok(())
}

macro_rules! check_user_ptr {
    ($ptr:expr, $len:expr) => {
        match validate_user_ptr($ptr as usize, $len) {
            Ok(()) => {}
            Err(e) => return e,
        }
    };
}

// ---------------------------------------------------------------------------
// Security gate
// ---------------------------------------------------------------------------

#[inline]
fn syscall_security_entry(pid: u32, call_nr: u32, args: [u64; 6]) -> Option<u64> {
    use crate::security::seccomp::SeccompAction;
    crate::security::audit::log_syscall_entry(pid, call_nr, args);
    match crate::security::seccomp::seccomp_check(pid, call_nr as u64) {
        SeccompAction::KillProcess | SeccompAction::KillThread => {
            crate::security::audit::log_seccomp_kill(pid, call_nr);
            process::exit(-1);
            Some(errno::EPERM)
        }
        SeccompAction::Errno => {
            crate::security::audit::log_seccomp_kill(pid, call_nr);
            Some(errno::EPERM)
        }
        SeccompAction::Trap => {
            crate::security::audit::log_seccomp_kill(pid, call_nr);
            Some(errno::EPERM)
        }
        SeccompAction::Log => {
            crate::security::audit::log_seccomp_kill(pid, call_nr);
            None
        }
        SeccompAction::Allow => None,
    }
}

#[inline]
fn require_cap(pid: u32, cap: u64) -> Result<(), u64> {
    let ok = crate::security::caps::process_has_cap(pid, cap);
    crate::security::audit::log_cap_check(pid, cap, ok);
    if ok {
        Ok(())
    } else {
        Err(errno::EPERM)
    }
}

// ---------------------------------------------------------------------------
// Main syscall dispatch table
// ---------------------------------------------------------------------------

#[no_mangle]
extern "C" fn syscall_dispatch(
    arg1: u64,
    arg2: u64,
    arg3: u64,
    arg4: u64,
    _arg5: u64,
    _arg6: u64,
) -> u64 {
    let syscall_nr: u64;
    unsafe {
        core::arch::asm!("", out("rax") syscall_nr, options(nomem, nostack));
    }

    let pid = process::getpid();
    let call_nr = syscall_nr as u32;
    let args = [arg1, arg2, arg3, arg4, _arg5, _arg6];

    if let Some(err) = syscall_security_entry(pid, call_nr, args) {
        return err;
    }

    let ret = match syscall_nr {
        // ────────── Process management ──────────────────────────────────
        nr::SYS_EXIT => {
            process::exit(arg1 as i32);
            0
        }
        nr::SYS_YIELD => {
            process::yield_now();
            0
        }
        nr::SYS_GETPID => process::getpid() as u64,
        nr::SYS_GETPPID => process::getppid() as u64,

        nr::SYS_FORK => {
            let child = sys_fork();
            if child != u64::MAX {
                let cpid = child as u32;
                let _ = crate::security::lsm::lsm_check(
                    crate::security::lsm::LsmHook::TaskCreate,
                    0,
                    cpid,
                );
                crate::security::audit::log_process_fork(pid, cpid);
            }
            child
        }

        nr::SYS_WAITPID => sys_waitpid(arg1 as i32),

        nr::SYS_KILL => {
            let tpid = arg1 as u32;
            let sig = arg2 as u8;
            if tpid != pid {
                if let Err(e) = require_cap(pid, crate::security::caps::CAP_KILL) {
                    return e;
                }
                if crate::security::lsm::lsm_check(
                    crate::security::lsm::LsmHook::TaskKill(pid, tpid),
                    0,
                    pid,
                ) != 0
                {
                    return errno::EPERM;
                }
            }
            sys_kill(tpid, sig)
        }

        nr::SYS_EXEC => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_exec(arg1 as *const u8, arg2 as usize)
        }

        nr::SYS_CLONE => sys_fork(),
        nr::SYS_SIGACTION | nr::SYS_SIGRETURN => 0,

        nr::SYS_FUTEX => {
            check_user_ptr!(arg1, 4usize);
            // arg4 is an optional timeout pointer (NULL = no timeout)
            // For simplicity we treat arg4 as a raw nanosecond value when
            // non-zero (musl/glibc pass a timespec; we accept ns directly).
            let timeout_ns = if arg4 == 0 { None } else { Some(arg4) };
            crate::ipc::futex::sys_futex(
                arg1,         // uaddr
                arg2 as u32,  // op
                arg3 as u32,  // val
                timeout_ns,   // timeout
                _arg5,        // uaddr2
                _arg6 as u32, // val3
            ) as u64
        }

        nr::SYS_SETSID => sys_setsid(),
        nr::SYS_GETSID => {
            let t = if arg1 == 0 {
                process::getpid()
            } else {
                arg1 as u32
            };
            if t as usize >= process::MAX_PROCESSES {
                return errno::ESRCH;
            }
            sys_getsid(t)
        }
        nr::SYS_GETPGID => {
            let t = if arg1 == 0 {
                process::getpid()
            } else {
                arg1 as u32
            };
            if t as usize >= process::MAX_PROCESSES {
                return errno::ESRCH;
            }
            sys_getpgid(t)
        }
        nr::SYS_SETPGID => {
            let t = if arg1 == 0 {
                process::getpid()
            } else {
                arg1 as u32
            };
            if t as usize >= process::MAX_PROCESSES {
                return errno::ESRCH;
            }
            sys_setpgid(t, arg2 as u32)
        }

        // ────────── UID / GID ───────────────────────────────────────────
        nr::SYS_GETUID => {
            let tbl = process::pcb::PROCESS_TABLE.lock();
            tbl[pid as usize]
                .as_ref()
                .map(|p| p.uid as u64)
                .unwrap_or(0)
        }
        nr::SYS_GETGID => {
            let tbl = process::pcb::PROCESS_TABLE.lock();
            tbl[pid as usize]
                .as_ref()
                .map(|p| p.gid as u64)
                .unwrap_or(0)
        }
        nr::SYS_SETUID => {
            {
                let tbl = process::pcb::PROCESS_TABLE.lock();
                let cur = tbl[pid as usize]
                    .as_ref()
                    .map(|p| p.uid)
                    .unwrap_or(u32::MAX);
                if cur != arg1 as u32 {
                    if let Err(e) = require_cap(pid, crate::security::caps::CAP_SETUID) {
                        return e;
                    }
                }
            }
            sys_setuid(pid, arg1 as u32)
        }
        nr::SYS_SETGID => {
            {
                let tbl = process::pcb::PROCESS_TABLE.lock();
                let cur = tbl[pid as usize]
                    .as_ref()
                    .map(|p| p.gid)
                    .unwrap_or(u32::MAX);
                if cur != arg1 as u32 {
                    if let Err(e) = require_cap(pid, crate::security::caps::CAP_SETGID) {
                        return e;
                    }
                }
            }
            sys_setgid(pid, arg1 as u32)
        }

        // ────────── File I/O ────────────────────────────────────────────
        nr::SYS_WRITE => {
            check_user_ptr!(arg2, arg3 as usize);
            sys_write(arg1, arg2 as *const u8, arg3 as usize)
        }
        nr::SYS_READ => {
            check_user_ptr!(arg2, arg3 as usize);
            sys_read(arg1, arg2 as *mut u8, arg3 as usize)
        }
        nr::SYS_OPEN => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_open(arg1 as *const u8, arg2 as usize, arg3 as u32)
        }
        nr::SYS_CLOSE => sys_close(arg1),
        nr::SYS_PIPE => {
            check_user_ptr!(arg1, 8usize);
            sys_pipe(arg1 as *mut u32)
        }
        nr::SYS_DUP => sys_dup(arg1),
        nr::SYS_DUP2 => sys_dup2(arg1, arg2),
        nr::SYS_LSEEK => sys_lseek(arg1, arg2 as i64, arg3 as u32),
        nr::SYS_STAT => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_stat(arg1 as *const u8, arg2 as usize, arg3 as *mut u8)
        }
        nr::SYS_FSTAT => {
            check_user_ptr!(arg2, 72usize);
            sys_fstat(arg1, arg2 as *mut u8)
        }
        nr::SYS_FCNTL => sys_fcntl(arg1, arg2 as u32, arg3),
        nr::SYS_GETDENTS | nr::SYS_GETDENTS64 => {
            check_user_ptr!(arg2, arg3 as usize);
            sys_getdents(arg1, arg2 as *mut u8, arg3 as usize)
        }
        nr::SYS_IOCTL => sys_ioctl(arg1, arg2, arg3),
        nr::SYS_GETCWD => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_getcwd(arg1 as *mut u8, arg2 as usize)
        }
        nr::SYS_CHDIR => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_chdir(arg1 as *const u8, arg2 as usize)
        }
        nr::SYS_MKDIR => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_mkdir(arg1 as *const u8, arg2 as usize, arg3 as u32)
        }
        nr::SYS_RMDIR => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_rmdir(arg1 as *const u8, arg2 as usize)
        }
        nr::SYS_UNLINK => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_unlink(arg1 as *const u8, arg2 as usize)
        }
        nr::SYS_RENAME => {
            check_user_ptr!(arg1, arg2 as usize);
            check_user_ptr!(arg3, arg4 as usize);
            sys_rename(
                arg1 as *const u8,
                arg2 as usize,
                arg3 as *const u8,
                arg4 as usize,
            )
        }
        nr::SYS_CHMOD => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_chmod(arg1 as *const u8, arg2 as usize, arg3 as u32)
        }
        nr::SYS_CHOWN => {
            if let Err(e) = require_cap(pid, crate::security::caps::CAP_CHOWN) {
                return e;
            }
            check_user_ptr!(arg1, arg2 as usize);
            sys_chown(arg1 as *const u8, arg2 as usize, arg3 as u32, arg4 as u32)
        }
        nr::SYS_SYMLINK => {
            check_user_ptr!(arg1, arg2 as usize);
            check_user_ptr!(arg3, arg4 as usize);
            sys_symlink(
                arg1 as *const u8,
                arg2 as usize,
                arg3 as *const u8,
                arg4 as usize,
            )
        }
        nr::SYS_READLINK => {
            check_user_ptr!(arg1, arg2 as usize);
            check_user_ptr!(arg3, arg4 as usize);
            sys_readlink(
                arg1 as *const u8,
                arg2 as usize,
                arg3 as *mut u8,
                arg4 as usize,
            )
        }
        nr::SYS_TRUNCATE => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_truncate(arg1 as *const u8, arg2 as usize, arg3)
        }
        nr::SYS_UNAME => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_uname(arg1 as *mut u8, arg2 as usize)
        }
        nr::SYS_SYSINFO => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_sysinfo(arg1 as *mut u8, arg2 as usize)
        }
        nr::SYS_UMASK => 0o022,

        // ────────── Memory ──────────────────────────────────────────────
        nr::SYS_MMAP => sys_mmap_full(
            arg1 as usize,
            arg2 as usize,
            arg3 as u32,
            arg4 as u32,
            _arg5,
            _arg6,
        ),
        nr::SYS_MUNMAP => sys_munmap(arg1 as usize, arg2 as usize),
        nr::SYS_MPROTECT => sys_mprotect(arg1 as usize, arg2 as usize, arg3 as u32),
        nr::SYS_MADVISE => 0,
        nr::SYS_BRK => sys_brk(arg1 as usize),

        // ────────── Time ────────────────────────────────────────────────
        nr::SYS_TIME => sys_time(),
        nr::SYS_CLOCK_GETTIME => sys_clock_gettime(),
        nr::SYS_NANOSLEEP => sys_nanosleep(arg1),

        // ────────── Privileged operations ───────────────────────────────
        nr::SYS_SHUTDOWN => match require_cap(pid, crate::security::caps::CAP_SYS_BOOT) {
            Err(e) => e,
            Ok(()) => sys_shutdown(pid),
        },
        nr::SYS_REBOOT => match require_cap(pid, crate::security::caps::CAP_SYS_BOOT) {
            Err(e) => e,
            Ok(()) => sys_reboot(pid),
        },
        nr::SYS_MOUNT => match require_cap(pid, crate::security::caps::CAP_SYS_ADMIN) {
            Err(e) => e,
            Ok(()) => sys_mount(pid),
        },
        nr::SYS_UMOUNT => match require_cap(pid, crate::security::caps::CAP_SYS_ADMIN) {
            Err(e) => e,
            Ok(()) => sys_umount(pid),
        },
        nr::SYS_SETHOSTNAME => match require_cap(pid, crate::security::caps::CAP_SYS_ADMIN) {
            Err(e) => e,
            Ok(()) => {
                check_user_ptr!(arg1, arg2 as usize);
                sys_sethostname(arg1 as *const u8, arg2 as usize)
            }
        },
        nr::SYS_GETHOSTNAME => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_gethostname(arg1 as *mut u8, arg2 as usize)
        }
        nr::SYS_GETRUSAGE => {
            check_user_ptr!(arg1, arg2 as usize);
            sys_getrusage(arg1 as *mut u8, arg2 as usize)
        }

        // ────────── Network (stubbed) ────────────────────────────────────
        nr::SYS_SOCKET => sys_socket(arg1 as u32, arg2 as u32, arg3 as u32),
        nr::SYS_BIND => sys_bind(arg1 as u32, arg2 as *const u8, arg3 as u32),
        nr::SYS_LISTEN => sys_listen(arg1 as u32, arg2 as i32),
        nr::SYS_CONNECT => sys_connect(arg1 as u32, arg2 as *const u8, arg3 as u32),
        nr::SYS_ACCEPT => sys_accept(arg1 as u32, arg2 as *mut u8, arg3 as *mut u32),
        nr::SYS_SEND => sys_send(arg1 as u32, arg2 as *const u8, arg3 as usize, arg4 as u32),
        nr::SYS_RECV => sys_recv(arg1 as u32, arg2 as *mut u8, arg3 as usize, arg4 as u32),
        nr::SYS_SELECT => sys_select(arg1 as u32),
        nr::SYS_POLL => sys_poll(arg2 as u32),

        // ────────── Neural bus ───────────────────────────────────────────
        nr::SYS_NEURAL_PULSE => {
            sys_neural_pulse(arg1 as u32, arg2 as u16, arg3 as i32, arg4 as i64)
        }
        nr::SYS_NEURAL_POLL => {
            check_user_ptr!(arg1, 64usize);
            sys_neural_poll(arg1 as *mut u8)
        }

        // ────────── Entropy / anonymous fds ─────────────────────────────
        nr::SYS_GETRANDOM => {
            // arg1 = *mut u8 buf, arg2 = len, arg3 = flags
            if arg1 != 0 {
                check_user_ptr!(arg1, arg2 as usize);
            }
            let result = crate::kernel::getrandom::sys_getrandom(
                arg1 as *mut u8,
                arg2 as usize,
                arg3 as u32,
            );
            result as u64
        }

        nr::SYS_MEMFD_CREATE => {
            // arg1 = name ptr, arg2 = name len, arg3 = flags
            if arg1 != 0 {
                check_user_ptr!(arg1, arg2 as usize);
            }
            let result =
                crate::ipc::memfd::sys_memfd_create(arg1 as *const u8, arg2 as usize, arg3 as u32);
            result as u64
        }

        // ────────── POSIX Message Queues ────────────────────────────────
        nr::SYS_MQ_OPEN => {
            // mq_open(name_ptr, name_len, flags, max_msg, max_msgsize)
            check_user_ptr!(arg1, arg2 as usize);
            let name = match read_user_path(arg1 as *const u8, arg2 as usize) {
                Ok(n) => n,
                Err(()) => return errno::EFAULT,
            };
            let result =
                crate::ipc::mqueue::sys_mq_open(&name, arg3 as u32, arg4 as usize, _arg5 as usize);
            result as u64
        }

        nr::SYS_MQ_SEND => {
            // mq_send(mqfd, data_ptr, data_len, priority)
            check_user_ptr!(arg2, arg3 as usize);
            let data = unsafe { core::slice::from_raw_parts(arg2 as *const u8, arg3 as usize) };
            let result = crate::ipc::mqueue::sys_mq_send(arg1 as i32, data, arg4 as u32);
            result as u64
        }

        nr::SYS_MQ_RECEIVE => {
            // mq_receive(mqfd, buf_ptr, buf_len, priority_out_ptr)
            check_user_ptr!(arg2, arg3 as usize);
            let buf = unsafe { core::slice::from_raw_parts_mut(arg2 as *mut u8, arg3 as usize) };
            let (bytes, prio) = crate::ipc::mqueue::sys_mq_receive(arg1 as i32, buf);
            // Write priority back to userspace if pointer provided
            if arg4 != 0 {
                check_user_ptr!(arg4, 4usize);
                unsafe {
                    core::ptr::write(arg4 as *mut u32, prio);
                }
            }
            bytes as u64
        }

        nr::SYS_MQ_CLOSE => {
            let result = crate::ipc::mqueue::sys_mq_close(arg1 as i32);
            result as u64
        }

        nr::SYS_MQ_UNLINK => {
            check_user_ptr!(arg1, arg2 as usize);
            let name = match read_user_path(arg1 as *const u8, arg2 as usize) {
                Ok(n) => n,
                Err(()) => return errno::EFAULT,
            };
            let result = crate::ipc::mqueue::sys_mq_unlink(&name);
            result as u64
        }

        nr::SYS_MQ_GETATTR => {
            // mq_getattr(mqfd) — returns max_msg in low 32 bits, curmsgs in high 32 bits
            match crate::ipc::mqueue::sys_mq_getattr(arg1 as i32) {
                Some((max_msg, _max_sz, cur, _flags)) => ((cur as u64) << 32) | (max_msg as u64),
                None => errno::EBADF,
            }
        }

        nr::SYS_MQ_NOTIFY => {
            // mq_notify(mqfd, pid, signal)
            let result = crate::ipc::mqueue::sys_mq_notify(arg1 as i32, arg2 as u32, arg3 as u32);
            result as u64
        }

        // ────────── POSIX / System V Shared Memory ───────────────────────
        nr::SYS_SHM_OPEN => {
            // shm_open(name_ptr, name_len, flags, mode)
            check_user_ptr!(arg1, arg2 as usize);
            let name = match read_user_path(arg1 as *const u8, arg2 as usize) {
                Ok(n) => n,
                Err(()) => return errno::EFAULT,
            };
            let result = crate::ipc::shm::shm_open(name.as_bytes(), arg3 as u32, arg4 as u16);
            result as u64
        }

        nr::SYS_SHM_CLOSE => {
            crate::ipc::shm::shm_close(arg1 as i32);
            0
        }

        nr::SYS_SHM_UNLINK => {
            check_user_ptr!(arg1, arg2 as usize);
            let name = match read_user_path(arg1 as *const u8, arg2 as usize) {
                Ok(n) => n,
                Err(()) => return errno::EFAULT,
            };
            let result = crate::ipc::shm::shm_unlink(name.as_bytes());
            result as u64
        }

        nr::SYS_SHM_READ => {
            // shm_read(shmfd, offset, buf_ptr, len)
            check_user_ptr!(arg3, arg4 as usize);
            let buf = unsafe { core::slice::from_raw_parts_mut(arg3 as *mut u8, arg4 as usize) };
            crate::ipc::shm::shm_read(arg1 as i32, arg2 as usize, buf) as u64
        }

        nr::SYS_SHM_WRITE => {
            // shm_write(shmfd, offset, buf_ptr, len)
            check_user_ptr!(arg3, arg4 as usize);
            let data = unsafe { core::slice::from_raw_parts(arg3 as *const u8, arg4 as usize) };
            crate::ipc::shm::shm_write(arg1 as i32, arg2 as usize, data) as u64
        }

        nr::SYS_SHM_TRUNCATE => {
            let result = crate::ipc::shm::shm_truncate(arg1 as i32, arg2 as usize);
            result as u64
        }

        nr::SYS_SHMGET => {
            // shmget(key, size, flags)
            let result = crate::ipc::shm::shm_get(arg1 as i32, arg2 as usize, arg3 as i32);
            result as u64
        }

        nr::SYS_SHMAT => {
            // shmat(shmid, addr, flags)
            crate::ipc::shm::shmat(arg1 as i32, arg2, arg3 as u32)
        }

        nr::SYS_SHMDT => {
            let result = crate::ipc::shm::shmdt(arg1);
            result as u64
        }

        nr::SYS_SHMCTL => {
            // shmctl(shmid, cmd, buf_ptr)
            let result = crate::ipc::shm::shmctl(arg1 as i32, arg2 as u32, arg3);
            result as u64
        }

        // ────────── Kernel Module Loading ────────────────────────────────
        nr::SYS_LOAD_MODULE => {
            // load_module(elf_ptr, elf_len, name_ptr, name_len)
            if arg1 == 0 || arg3 == 0 {
                return errno::EFAULT;
            }
            check_user_ptr!(arg1, arg2 as usize);
            check_user_ptr!(arg3, arg4 as usize);
            // Require CAP_SYS_MODULE to load kernel modules
            if let Err(e) = require_cap(pid, crate::security::caps::CAP_SYS_MODULE) {
                return e;
            }
            let elf_data = unsafe { core::slice::from_raw_parts(arg1 as *const u8, arg2 as usize) };
            let name = match read_user_path(arg3 as *const u8, arg4 as usize) {
                Ok(n) => n,
                Err(()) => return errno::EFAULT,
            };
            let result = crate::kernel::modules::load_elf_module(elf_data, &name);
            result as u64
        }

        nr::SYS_UNLOAD_MODULE => {
            // unload_module(name_ptr, name_len)
            check_user_ptr!(arg1, arg2 as usize);
            if let Err(e) = require_cap(pid, crate::security::caps::CAP_SYS_MODULE) {
                return e;
            }
            let name = match read_user_path(arg1 as *const u8, arg2 as usize) {
                Ok(n) => n,
                Err(()) => return errno::EFAULT,
            };
            let result = crate::kernel::modules::unload_elf_module(&name);
            result as u64
        }

        // ────────── TUN/TAP virtual network interfaces ───────────────────
        nr::SYS_TUN_OPEN => {
            // tun_open(name_ptr, name_len, flags) -> fd
            //   flags: 0 = TUN (IP-level), 1 = TAP (Ethernet-level)
            if arg1 == 0 {
                return errno::EFAULT;
            }
            check_user_ptr!(arg1, arg2 as usize);
            if let Err(e) = require_cap(pid, crate::security::caps::CAP_NET_ADMIN) {
                return e;
            }
            let name_len = (arg2 as usize).min(15);
            let name_bytes = unsafe { core::slice::from_raw_parts(arg1 as *const u8, name_len) };
            let dev_type = if arg3 == 0 {
                crate::net::tuntap::TunTapType::Tun
            } else {
                crate::net::tuntap::TunTapType::Tap
            };
            match crate::net::tuntap::tun_create(name_bytes, dev_type) {
                Some(fd) => fd as u64,
                None => errno::ENFILE,
            }
        }

        nr::SYS_TUN_READ => {
            // tun_read(fd, buf_ptr) -> bytes_read  (buf must be >= 1500 bytes)
            check_user_ptr!(arg2, 1500usize);
            let fd = arg1 as i32;
            if !crate::net::tuntap::tun_is_fd(fd) {
                return errno::EBADF;
            }
            // tun_read requires a &mut [u8; 1500]; copy via stack buffer.
            let mut tmp = [0u8; 1500];
            let n = crate::net::tuntap::tun_read(fd, &mut tmp);
            if n < 0 {
                return errno::EAGAIN;
            }
            let copy = n as usize;
            let buf = unsafe { core::slice::from_raw_parts_mut(arg2 as *mut u8, copy) };
            buf[..copy].copy_from_slice(&tmp[..copy]);
            copy as u64
        }

        nr::SYS_TUN_WRITE => {
            // tun_write(fd, buf_ptr, len) -> bytes_written
            check_user_ptr!(arg2, arg3 as usize);
            let fd = arg1 as i32;
            if !crate::net::tuntap::tun_is_fd(fd) {
                return errno::EBADF;
            }
            let data = unsafe { core::slice::from_raw_parts(arg2 as *const u8, arg3 as usize) };
            let n = crate::net::tuntap::tun_write(fd, data);
            if n < 0 {
                errno::EIO
            } else {
                n as u64
            }
        }

        // ────────── Namespace management ────────────────────────────────
        nr::SYS_UNSHARE => {
            // unshare(flags) — create new namespaces for the calling process.
            // Requires CAP_SYS_ADMIN for all namespace types.
            match require_cap(pid, crate::security::caps::CAP_SYS_ADMIN) {
                Err(e) => e,
                Ok(()) => proc_ops::sys_unshare(pid, arg1 as u32),
            }
        }

        nr::SYS_SETNS => {
            // setns(fd, nstype) — reassociate thread with a namespace.
            // fd is a file descriptor referring to a namespace object;
            // nstype hints the expected namespace type (0 = any).
            // Requires CAP_SYS_ADMIN.
            match require_cap(pid, crate::security::caps::CAP_SYS_ADMIN) {
                Err(e) => e,
                Ok(()) => proc_ops::sys_setns(pid, arg1 as i32, arg2 as i32),
            }
        }

        // ────────── PTY (pseudo-terminal) ────────────────────────────────
        nr::SYS_PTY_OPEN => {
            // pty_open() -> master_fd
            match crate::drivers::pty::pty_open() {
                Some(fd) => fd as u64,
                None => errno::ENFILE,
            }
        }

        nr::SYS_PTY_SLAVE => {
            // pty_get_slave_fd(master_fd) -> slave_fd
            let master_fd = arg1 as i32;
            match crate::drivers::pty::pty_get_slave_fd(master_fd) {
                Some(sfd) => sfd as u64,
                None => errno::EBADF,
            }
        }

        nr::SYS_PTY_CLOSE_MASTER => {
            // pty_close_master(master_fd) -> 0
            let master_fd = arg1 as i32;
            if crate::drivers::pty::pty_close_master(master_fd) {
                0
            } else {
                errno::EBADF
            }
        }

        nr::SYS_PTY_CLOSE_SLAVE => {
            // pty_close_slave(slave_fd) -> 0
            let slave_fd = arg1 as i32;
            if crate::drivers::pty::pty_close_slave(slave_fd) {
                0
            } else {
                errno::EBADF
            }
        }

        nr::SYS_PTY_SETWINSZ => {
            // pty_set_winsize(master_fd, rows, cols) -> 0
            let master_fd = arg1 as i32;
            let rows = arg2 as u16;
            let cols = arg3 as u16;
            if crate::drivers::pty::pty_set_winsize(master_fd, rows, cols) {
                0
            } else {
                errno::EBADF
            }
        }

        nr::SYS_PTY_SLAVENAME => {
            // pty_get_slave_name(master_fd, buf_ptr, buf_len) -> name_len
            // buf must be at least 32 bytes.
            if arg2 == 0 {
                return errno::EFAULT;
            }
            check_user_ptr!(arg2, 32usize);
            let master_fd = arg1 as i32;
            let mut name_buf = [0u8; 32];
            let len = crate::drivers::pty::pty_get_slave_name(master_fd, &mut name_buf);
            if len == 0 {
                return errno::EBADF;
            }
            // Copy to userspace buffer (caller must provide >= 32 bytes).
            let copy = len.min(arg3 as usize).min(32);
            let dest = unsafe { core::slice::from_raw_parts_mut(arg2 as *mut u8, copy) };
            dest[..copy].copy_from_slice(&name_buf[..copy]);
            len as u64
        }

        // ────────── RT signal action (rt_sigaction) ─────────────────────
        nr::SYS_RT_SIGACTION => {
            // rt_sigaction(signo, new_act_ptr, old_act_ptr, sigsetsize)
            // arg1 = signo
            // arg2 = *const SigAction (new action; 0 = query only)
            // arg3 = *mut SigAction   (previous action written here; 0 = ignore)
            // arg4 = sigsetsize (ignored — we use u64 masks)
            let signo = arg1 as u32;
            if signo == 0 || signo >= 64 {
                return errno::EINVAL;
            }
            // Write back old action if caller wants it.
            if arg3 != 0 {
                check_user_ptr!(arg3, core::mem::size_of::<process::sigaction::SigAction>());
                let old = process::sigaction::sigaction_get(pid, signo)
                    .unwrap_or(process::sigaction::SigAction::default());
                unsafe {
                    core::ptr::write_volatile(arg3 as *mut process::sigaction::SigAction, old);
                }
            }
            // Install new action if provided.
            if arg2 != 0 {
                check_user_ptr!(arg2, core::mem::size_of::<process::sigaction::SigAction>());
                let new_act: process::sigaction::SigAction = unsafe {
                    core::ptr::read_volatile(arg2 as *const process::sigaction::SigAction)
                };
                if !process::sigaction::sigaction_set(pid, signo, new_act) {
                    return errno::EINVAL;
                }
            }
            0
        }

        // ────────── RT sigprocmask ───────────────────────────────────────
        nr::SYS_RT_SIGPROCMASK => {
            // rt_sigprocmask(how, set_ptr, oldset_ptr, sigsetsize)
            // how: 0=SIG_BLOCK, 1=SIG_UNBLOCK, 2=SIG_SETMASK
            let how = arg1 as u32;
            let set_ptr = arg2;
            let oset_ptr = arg3;
            // Return old mask to caller if requested.
            if oset_ptr != 0 {
                check_user_ptr!(oset_ptr, 8usize);
                let old = process::sigaction::sigprocmask_get(pid);
                unsafe {
                    core::ptr::write_volatile(oset_ptr as *mut u64, old);
                }
            }
            // Apply new mask if provided.
            if set_ptr != 0 {
                check_user_ptr!(set_ptr, 8usize);
                let new_mask: u64 = unsafe { core::ptr::read_volatile(set_ptr as *const u64) };
                match how {
                    0 => process::sigaction::sigprocmask_block(pid, new_mask), // SIG_BLOCK
                    1 => process::sigaction::sigprocmask_unblock(pid, new_mask), // SIG_UNBLOCK
                    2 => process::sigaction::sigprocmask_set(pid, new_mask),   // SIG_SETMASK
                    _ => return errno::EINVAL,
                }
            }
            0
        }

        // ────────── RT sigpending ────────────────────────────────────────
        nr::SYS_RT_SIGPENDING => {
            // rt_sigpending(set_ptr, sigsetsize)
            if arg1 == 0 {
                return errno::EFAULT;
            }
            check_user_ptr!(arg1, 8usize);
            let pending = process::sigaction::sigpending(pid);
            unsafe {
                core::ptr::write_volatile(arg1 as *mut u64, pending);
            }
            0
        }

        // ────────── RT sigqueueinfo ──────────────────────────────────────
        nr::SYS_RT_SIGQUEUEINFO => {
            // rt_sigqueueinfo(target_pid, signo, siginfo_ptr)
            // arg1 = target_pid, arg2 = signo, arg3 = *const Siginfo
            let target = arg1 as u32;
            let signo = arg2 as u32;
            let info_ptr = arg3;

            if info_ptr == 0 {
                return errno::EFAULT;
            }
            check_user_ptr!(
                info_ptr,
                core::mem::size_of::<process::realtime_signal::Siginfo>()
            );

            let info: process::realtime_signal::Siginfo = unsafe {
                core::ptr::read_volatile(info_ptr as *const process::realtime_signal::Siginfo)
            };

            use process::realtime_signal::{SIGRTMAX, SIGRTMIN};
            if signo >= SIGRTMIN && signo <= SIGRTMAX {
                let rc = process::realtime_signal::sigrt_send(target, signo, info);
                if rc < 0 {
                    (rc as i64 as u64)
                } else {
                    0
                }
            } else if signo > 0 && signo <= 31 {
                match process::signal::send_signal_to(target, signo as u8) {
                    Ok(()) => 0,
                    Err(_) => errno::ESRCH,
                }
            } else {
                errno::EINVAL
            }
        }

        // ────────── POSIX timers ─────────────────────────────────────────
        nr::SYS_TIMER_CREATE => {
            // timer_create(clock_id, sigevent_ptr, timer_id_out_ptr)
            // arg1 = clock_id  (i32)
            // arg2 = sigevent_ptr  — points to packed struct: [notify:i32, signo:u32, value:i32]
            // arg3 = *mut i32  — receives the new timer_id
            let clock_id = arg1 as i32;
            let sev_ptr = arg2;
            let id_out = arg3;

            if id_out == 0 {
                return errno::EFAULT;
            }
            check_user_ptr!(id_out, 4usize);

            // Decode sigevent fields from user pointer if provided.
            let (notify, signo, sival) = if sev_ptr != 0 {
                check_user_ptr!(sev_ptr, 12usize);
                let notify: i32 = unsafe { core::ptr::read_volatile(sev_ptr as *const i32) };
                let signo: u32 = unsafe { core::ptr::read_volatile((sev_ptr + 4) as *const u32) };
                let sival: i32 = unsafe { core::ptr::read_volatile((sev_ptr + 8) as *const i32) };
                (notify, signo, sival)
            } else {
                // Default: SIGEV_SIGNAL, SIGALRM (14)
                (process::posix_timer::SIGEV_SIGNAL, 14u32, 0i32)
            };

            let id = process::posix_timer::timer_create(pid, clock_id, notify, signo, sival);
            if id < 0 {
                return (id as i64 as u64).wrapping_neg().wrapping_neg(); // propagate -errno
            }
            unsafe {
                core::ptr::write_volatile(id_out as *mut i32, id);
            }
            0
        }

        nr::SYS_TIMER_SETTIME => {
            // timer_settime(timer_id, flags, new_value_ptr, old_value_ptr)
            // new_value_ptr → [interval_ms:u64, value_ms:u64]  (simplified)
            let timer_id = arg1 as i32;
            let flags = arg2 as i32;
            let new_val_ptr = arg3;
            let _old_val_ptr = arg4;

            if new_val_ptr == 0 {
                return errno::EFAULT;
            }
            check_user_ptr!(new_val_ptr, 16usize);

            let interval_ms: u64 = unsafe { core::ptr::read_volatile(new_val_ptr as *const u64) };
            let value_ms: u64 =
                unsafe { core::ptr::read_volatile((new_val_ptr + 8) as *const u64) };

            let rc = process::posix_timer::timer_settime(timer_id, flags, interval_ms, value_ms);
            if rc < 0 {
                rc as i64 as u64
            } else {
                0
            }
        }

        nr::SYS_TIMER_GETTIME => {
            // timer_gettime(timer_id, cur_value_ptr)
            // cur_value_ptr → [interval_ms:u64, remaining_ms:u64]
            let timer_id = arg1 as i32;
            let out_ptr = arg2;
            if out_ptr == 0 {
                return errno::EFAULT;
            }
            check_user_ptr!(out_ptr, 16usize);
            match process::posix_timer::timer_gettime(timer_id) {
                None => errno::EINVAL,
                Some((remaining_ms, interval_ms)) => {
                    unsafe {
                        core::ptr::write_volatile(out_ptr as *mut u64, interval_ms);
                        core::ptr::write_volatile((out_ptr + 8) as *mut u64, remaining_ms);
                    }
                    0
                }
            }
        }

        nr::SYS_TIMER_GETOVERRUN => {
            // timer_getoverrun(timer_id) -> overrun_count
            let timer_id = arg1 as i32;
            let rc = process::posix_timer::timer_getoverrun(timer_id);
            if rc < 0 {
                errno::EINVAL
            } else {
                rc as u64
            }
        }

        nr::SYS_TIMER_DELETE => {
            // timer_delete(timer_id) -> 0
            let timer_id = arg1 as i32;
            let rc = process::posix_timer::timer_delete(timer_id);
            if rc < 0 {
                errno::EINVAL
            } else {
                0
            }
        }

        _ => {
            serial_println!("  Syscall: unknown syscall {}", syscall_nr);
            errno::ENOSYS
        }
    };

    crate::security::audit::log_syscall_exit(pid, call_nr, ret as i64);
    ret
}

// ---------------------------------------------------------------------------
// Kernel-side helper wrappers (used without going through userspace)
// ---------------------------------------------------------------------------

pub fn kernel_pipe(fds: &mut [u32; 2]) -> u64 {
    sys_pipe(fds.as_mut_ptr())
}
pub fn kernel_dup2(old: u32, new: u32) -> u64 {
    sys_dup2(old as u64, new as u64)
}
pub fn kernel_read(fd: u32, buf: &mut [u8]) -> u64 {
    sys_read(fd as u64, buf.as_mut_ptr(), buf.len())
}
pub fn kernel_write(fd: u32, buf: &[u8]) -> u64 {
    sys_write(fd as u64, buf.as_ptr(), buf.len())
}
pub fn kernel_close_local_fd(fd: u32) {
    let pid = process::getpid();
    let _ = close_local_fd_mapping(pid, fd);
}
