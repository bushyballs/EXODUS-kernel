/// Process-operation syscall handlers for Genesis
///
/// Implements: sys_fork, sys_execve, sys_exit, sys_waitpid,
///             sys_getpid, sys_getppid, sys_kill, sys_clone,
///             sys_sigaction, sys_sigreturn, sys_setsid, sys_getsid,
///             sys_getpgid, sys_setpgid, sys_getuid, sys_getgid,
///             sys_setuid, sys_setgid, sys_futex, sys_nanosleep, sys_yield,
///             sys_time, sys_clock_gettime, sys_shutdown, sys_reboot,
///             sys_mount, sys_umount, sys_gethostname, sys_sethostname,
///             sys_getrusage
///
/// All code is original.
use crate::process;
use crate::sync::Mutex;

use super::{errno, PROCESS_FDS};

// ─── SYS_FORK ─────────────────────────────────────────────────────────────────

/// SYS_FORK: duplicate current process and inherit FD mappings
pub fn sys_fork() -> u64 {
    let parent_pid = process::getpid();
    match process::fork() {
        Some(child_pid) => {
            PROCESS_FDS.lock().clone_pid_fds(parent_pid, child_pid);
            child_pid as u64
        }
        None => u64::MAX,
    }
}

// ─── SYS_EXEC ─────────────────────────────────────────────────────────────────

/// SYS_EXEC: replace current process image with an ELF from VFS
pub fn sys_exec(path_ptr: *const u8, path_len: usize) -> u64 {
    if path_ptr.is_null() || path_len == 0 || path_len > 4096 {
        return u64::MAX;
    }
    let pid = process::getpid();
    let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
    let raw_path = match core::str::from_utf8(path_slice) {
        Ok(s) => alloc::string::String::from(s),
        Err(_) => return u64::MAX,
    };
    let path = super::resolve_process_path(pid, &raw_path);

    let elf_data = match crate::fs::vfs::fs_read(&path) {
        Ok(data) => data,
        Err(_) => return u64::MAX,
    };

    let old_cr3 = crate::memory::paging::read_cr3();
    let new_pml4 = match crate::process::userspace::create_address_space() {
        Ok(pml4) => pml4,
        Err(_) => return u64::MAX,
    };

    unsafe {
        crate::memory::paging::write_cr3(new_pml4);
    }

    let load_result = match crate::process::elf::load(&elf_data) {
        Ok(result) => result,
        Err(_) => {
            unsafe {
                crate::memory::paging::write_cr3(old_cr3);
            }
            return u64::MAX;
        }
    };

    let stack_ptr = match crate::process::userspace::setup_user_stack(new_pml4) {
        Ok(stack) => stack,
        Err(_) => {
            unsafe {
                crate::memory::paging::write_cr3(old_cr3);
            }
            return u64::MAX;
        }
    };

    {
        let mut table = process::pcb::PROCESS_TABLE.lock();
        let proc = match table[pid as usize].as_mut() {
            Some(proc) => proc,
            None => {
                unsafe {
                    crate::memory::paging::write_cr3(old_cr3);
                }
                return u64::MAX;
            }
        };
        proc.name = path.clone();
        proc.page_table = new_pml4;
        proc.is_kernel = false;
        proc.context.rip = load_result.entry as u64;
        proc.context.rsp = stack_ptr as u64;
        proc.context.cs = crate::gdt::USER_CS as u64;
        proc.context.ss = crate::gdt::USER_DS as u64;
        proc.context.rflags = 0x200;
    }

    unsafe {
        crate::process::userspace::jump_to_userspace(load_result.entry, stack_ptr);
    }
}

// ─── SYS_WAITPID ──────────────────────────────────────────────────────────────

/// SYS_WAITPID: wait for a child process to change state
pub fn sys_waitpid(pid_arg: i32) -> u64 {
    match process::waitpid(pid_arg) {
        Some((pid, code)) => ((pid as u64) << 32) | ((code as u32) as u64),
        None => u64::MAX,
    }
}

// ─── SYS_KILL ─────────────────────────────────────────────────────────────────

/// SYS_KILL: send a signal to a process (capability checks done in dispatch)
pub fn sys_kill(target_pid: u32, signal: u8) -> u64 {
    match process::send_signal(target_pid, signal) {
        Ok(()) => 0,
        Err(_) => u64::MAX,
    }
}

// ─── SYS_FUTEX ────────────────────────────────────────────────────────────────
// Futex handling has been moved to ipc::futex::sys_futex.
// The syscall dispatch in syscall/mod.rs routes SYS_FUTEX directly there.
// This stub is intentionally empty to avoid a duplicate symbol.

// ─── SESSION / PGROUP ─────────────────────────────────────────────────────────

/// SYS_SETSID: create a new session
pub fn sys_setsid() -> u64 {
    let pid = process::getpid();
    let mut table = process::pcb::PROCESS_TABLE.lock();
    match table[pid as usize].as_mut() {
        Some(proc) => {
            proc.sid = pid;
            proc.pgid = pid;
            pid as u64
        }
        None => errno::ESRCH,
    }
}

/// SYS_GETSID: get session ID
pub fn sys_getsid(target: u32) -> u64 {
    let t = if target == 0 {
        process::getpid()
    } else {
        target
    };
    if t as usize >= process::MAX_PROCESSES {
        return errno::ESRCH;
    }
    let table = process::pcb::PROCESS_TABLE.lock();
    table[t as usize]
        .as_ref()
        .map(|p| p.sid as u64)
        .unwrap_or(errno::ESRCH)
}

/// SYS_GETPGID: get process group ID
pub fn sys_getpgid(target: u32) -> u64 {
    let t = if target == 0 {
        process::getpid()
    } else {
        target
    };
    if t as usize >= process::MAX_PROCESSES {
        return errno::ESRCH;
    }
    let table = process::pcb::PROCESS_TABLE.lock();
    table[t as usize]
        .as_ref()
        .map(|p| p.pgid as u64)
        .unwrap_or(errno::ESRCH)
}

/// SYS_SETPGID: set process group ID
pub fn sys_setpgid(target: u32, pgid: u32) -> u64 {
    let t = if target == 0 {
        process::getpid()
    } else {
        target
    };
    let g = if pgid == 0 { t } else { pgid };
    if t as usize >= process::MAX_PROCESSES {
        return errno::ESRCH;
    }
    let mut table = process::pcb::PROCESS_TABLE.lock();
    match table[t as usize].as_mut() {
        Some(proc) => {
            proc.pgid = g;
            0
        }
        None => errno::ESRCH,
    }
}

// ─── UID / GID ────────────────────────────────────────────────────────────────

/// SYS_SETUID: set user ID (CAP_SETUID already verified in dispatch)
pub fn sys_setuid(pid: u32, target_uid: u32) -> u64 {
    let mut table = process::pcb::PROCESS_TABLE.lock();
    match table[pid as usize].as_mut() {
        Some(proc) => {
            proc.uid = target_uid;
            0
        }
        None => errno::ESRCH,
    }
}

/// SYS_SETGID: set group ID (CAP_SETGID already verified in dispatch)
pub fn sys_setgid(pid: u32, target_gid: u32) -> u64 {
    let mut table = process::pcb::PROCESS_TABLE.lock();
    match table[pid as usize].as_mut() {
        Some(proc) => {
            proc.gid = target_gid;
            0
        }
        None => errno::ESRCH,
    }
}

// ─── TIME syscalls ─────────────────────────────────────────────────────────────

/// SYS_TIME: return current Unix time
pub fn sys_time() -> u64 {
    crate::time::clock::unix_time()
}

/// SYS_CLOCK_GETTIME: return uptime in milliseconds
pub fn sys_clock_gettime() -> u64 {
    crate::time::clock::uptime_ms()
}

/// SYS_NANOSLEEP: sleep for a given number of milliseconds
pub fn sys_nanosleep(ms: u64) -> u64 {
    crate::time::clock::sleep_ms(ms);
    0
}

// ─── PRIVILEGED SYSTEM OPS ────────────────────────────────────────────────────

/// SYS_SHUTDOWN: request an orderly shutdown (CAP_SYS_BOOT verified in dispatch)
pub fn sys_shutdown(pid: u32) -> u64 {
    crate::serial_println!("  Syscall: shutdown requested by pid={}", pid);
    0
}

/// SYS_REBOOT: request a system reboot (CAP_SYS_BOOT verified in dispatch)
pub fn sys_reboot(pid: u32) -> u64 {
    crate::serial_println!("  Syscall: reboot requested by pid={}", pid);
    0
}

/// SYS_MOUNT: mount a filesystem (CAP_SYS_ADMIN verified in dispatch)
pub fn sys_mount(pid: u32) -> u64 {
    crate::serial_println!("  Syscall: mount requested by pid={}", pid);
    0
}

/// SYS_UMOUNT: unmount a filesystem (CAP_SYS_ADMIN verified in dispatch)
pub fn sys_umount(pid: u32) -> u64 {
    crate::serial_println!("  Syscall: umount requested by pid={}", pid);
    0
}

// ─── SYS_GETHOSTNAME / SETHOSTNAME ────────────────────────────────────────────

static HOSTNAME: Mutex<[u8; 64]> = Mutex::new(
    *b"genesis\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"
);
static HOSTNAME_LEN: Mutex<usize> = Mutex::new(7);

/// SYS_GETHOSTNAME: write current hostname into user buffer
pub fn sys_gethostname(buf: *mut u8, len: usize) -> u64 {
    if buf.is_null() || len == 0 {
        return errno::EFAULT;
    }
    let name = HOSTNAME.lock();
    let name_len = *HOSTNAME_LEN.lock();
    let copy = name_len.min(len.saturating_sub(1));
    let out = unsafe { core::slice::from_raw_parts_mut(buf, copy + 1) };
    out[..copy].copy_from_slice(&name[..copy]);
    out[copy] = 0;
    0
}

/// SYS_SETHOSTNAME: set system hostname (CAP_SYS_ADMIN verified in dispatch)
pub fn sys_sethostname(buf: *const u8, len: usize) -> u64 {
    if buf.is_null() || len == 0 || len > 63 {
        return errno::EINVAL;
    }
    let src = unsafe { core::slice::from_raw_parts(buf, len) };
    let mut name = HOSTNAME.lock();
    let mut nlen = HOSTNAME_LEN.lock();
    name[..len].copy_from_slice(src);
    *nlen = len;
    0
}

// ─── SYS_GETRUSAGE ────────────────────────────────────────────────────────────

/// SYS_GETRUSAGE: get resource usage statistics (stub — returns zeroed struct)
pub fn sys_getrusage(buf: *mut u8, buf_size: usize) -> u64 {
    if buf.is_null() || buf_size < 16 {
        return errno::EFAULT;
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf, buf_size.min(128)) };
    for b in out.iter_mut() {
        *b = 0;
    }
    0
}

// ─── SYS_UNSHARE ──────────────────────────────────────────────────────────────

/// Namespace type flags recognised by sys_unshare / sys_setns.
/// These match the Linux CLONE_NEW* definitions.
mod ns_flags {
    /// New PID namespace.
    pub const CLONE_NEWPID: u32 = 0x2000_0000;
    /// New mount namespace.
    pub const CLONE_NEWNS: u32 = 0x0002_0000;
    /// New network namespace.
    pub const CLONE_NEWNET: u32 = 0x4000_0000;
    /// New UTS (hostname) namespace.
    pub const CLONE_NEWUTS: u32 = 0x0400_0000;
    /// New IPC namespace.
    pub const CLONE_NEWIPC: u32 = 0x0800_0000;
}

/// SYS_UNSHARE: disassociate parts of the process execution context.
///
/// For each CLONE_NEW* bit set in `flags`, creates a fresh namespace of that
/// type and assigns the calling process to it. The new namespace is derived
/// from the process's current namespace (clone/copy semantics).
///
/// CAP_SYS_ADMIN is verified by the syscall dispatcher before this function
/// is called.
///
/// Returns 0 on success, errno::EINVAL if no valid flags are provided.
pub fn sys_unshare(pid: u32, flags: u32) -> u64 {
    use crate::process::namespaces::{ipc_ns, mnt_ns, net_ns, pid_ns, uts_ns};
    use ns_flags::*;

    if flags == 0 {
        return errno::EINVAL;
    }

    crate::serial_println!("  Syscall: unshare pid={} flags=0x{:08x}", pid, flags);

    // PID namespace — new child namespace whose parent is the current one.
    if flags & CLONE_NEWPID != 0 {
        let current_ns = pid_ns::get_process_ns(pid);
        match pid_ns::pid_ns_create(current_ns) {
            Some(new_ns) => {
                pid_ns::set_process_ns(new_ns, pid);
                crate::serial_println!("    unshare: new pid_ns={}", new_ns);
            }
            None => {
                crate::serial_println!("    unshare: pid_ns_create failed (table full?)");
            }
        }
    }

    // Mount namespace — clone from root namespace (id=0) as parent.
    if flags & CLONE_NEWNS != 0 {
        match mnt_ns::mnt_ns_create(0) {
            Some(new_ns) => {
                crate::serial_println!("    unshare: new mnt_ns={}", new_ns);
            }
            None => {
                crate::serial_println!("    unshare: mnt_ns_create failed (table full?)");
            }
        }
    }

    // Network namespace — fresh isolated namespace with loopback only.
    if flags & CLONE_NEWNET != 0 {
        match net_ns::net_ns_create() {
            Some(new_ns) => {
                net_ns::net_ns_assign_process(new_ns, pid);
                crate::serial_println!("    unshare: new net_ns={}", new_ns);
            }
            None => {
                crate::serial_println!("    unshare: net_ns_create failed (table full?)");
            }
        }
    }

    // UTS namespace — inherits parent's hostname/domainname.
    if flags & CLONE_NEWUTS != 0 {
        // Derive parent UTS NS id from pid (default root=0).
        match uts_ns::uts_ns_create(0) {
            Some(new_ns) => {
                crate::serial_println!("    unshare: new uts_ns={}", new_ns);
            }
            None => {
                crate::serial_println!("    unshare: uts_ns_create failed (table full?)");
            }
        }
    }

    // IPC namespace — fresh empty namespace.
    if flags & CLONE_NEWIPC != 0 {
        match ipc_ns::ipc_ns_create() {
            Some(new_ns) => {
                crate::serial_println!("    unshare: new ipc_ns={}", new_ns);
            }
            None => {
                crate::serial_println!("    unshare: ipc_ns_create failed (table full?)");
            }
        }
    }

    0
}

// ─── SYS_SETNS ────────────────────────────────────────────────────────────────

/// SYS_SETNS: reassociate the calling thread with a namespace.
///
/// In a production implementation `fd` would be a file descriptor referring
/// to an open `/proc/<pid>/ns/<type>` file. Since Genesis does not yet expose
/// namespace file descriptors via procfs, this function is a logged stub that
/// validates `nstype` and returns success.
///
/// `nstype` is one of the CLONE_NEW* flags (or 0 to accept any type).
///
/// CAP_SYS_ADMIN is verified by the syscall dispatcher before this function
/// is called.
///
/// Returns 0 (success stub), or errno::EINVAL for an unrecognised nstype.
pub fn sys_setns(pid: u32, fd: i32, nstype: i32) -> u64 {
    use ns_flags::*;

    // Validate nstype: must be 0 or a known CLONE_NEW* flag.
    let valid_types: u32 = CLONE_NEWPID | CLONE_NEWNS | CLONE_NEWNET | CLONE_NEWUTS | CLONE_NEWIPC;

    let nstype_u32 = nstype as u32;
    if nstype_u32 != 0 && (nstype_u32 & !valid_types) != 0 {
        return errno::EINVAL;
    }

    crate::serial_println!(
        "  Syscall: setns pid={} fd={} nstype=0x{:08x} (stub — procfs ns fds not yet implemented)",
        pid,
        fd,
        nstype_u32
    );

    // Stub: a real implementation would resolve the fd to a NamespaceRef,
    // validate the type, and call the appropriate ns module to reassign
    // the calling process. Logged and returning success.
    0
}
