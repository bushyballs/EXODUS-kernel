use alloc::collections::BTreeMap;
/// File-operation syscall handlers for Genesis
///
/// Implements: sys_read, sys_write, sys_open, sys_close, sys_stat, sys_fstat,
///             sys_lseek, sys_dup, sys_dup2, sys_pipe, sys_fcntl, sys_getdents,
///             sys_ioctl, sys_mkdir, sys_rmdir, sys_unlink, sys_rename,
///             sys_chmod, sys_chown, sys_symlink, sys_readlink, sys_truncate,
///             sys_getcwd, sys_chdir, sys_uname, sys_sysinfo
///
/// All code is original.
use alloc::string::String;
use alloc::vec::Vec;

use crate::process;
use crate::sync::Mutex;
use crate::{kprint, kprintln, serial_print, serial_println};

use super::{
    access_mode, can_read_flags, can_write_flags, close_kernel_fd, close_local_fd_mapping, errno,
    read_user_path, resolve_process_path, KernelFd, OpenFile, OpenFileTable, ProcessFdState,
    OPEN_FILES, PROCESS_FDS,
};

// ─── SYS_WRITE ────────────────────────────────────────────────────────────────

/// SYS_WRITE: Write bytes to a file descriptor
///
/// Args: fd (RDI), buf (RSI), count (RDX)
/// Returns: number of bytes written, or error
pub fn sys_write(fd: u64, buf: *const u8, count: usize) -> u64 {
    if buf.is_null() || count == 0 {
        return 0;
    }
    if count > 64 * 1024 * 1024 {
        return errno::EINVAL;
    }

    let slice = unsafe { core::slice::from_raw_parts(buf, count) };
    let pid = process::getpid();
    let mapped = {
        let table = PROCESS_FDS.lock();
        table.get_fd(pid, fd as u32)
    };

    if let Some(fd_kind) = mapped {
        return match fd_kind {
            KernelFd::PipeWrite(pipe_id) => match crate::ipc::pipe::write(pipe_id, slice) {
                Ok(n) => n as u64,
                Err(_) => u64::MAX,
            },
            KernelFd::PipeRead(_) => u64::MAX,
            KernelFd::File(handle_id) => {
                let mut table = OPEN_FILES.lock();
                let file = match table.get_mut(handle_id) {
                    Some(file) => file,
                    None => return u64::MAX,
                };
                if !can_write_flags(file.flags) {
                    return u64::MAX;
                }

                let write_offset = if file.flags & crate::fs::vfs::flags::O_APPEND != 0 {
                    match crate::fs::vfs::fs_stat(&file.path) {
                        Ok((_, size)) => size as usize,
                        Err(_) => return u64::MAX,
                    }
                } else {
                    file.offset as usize
                };

                match crate::fs::vfs::fs_write_at(&file.path, write_offset, slice) {
                    Ok(n) => {
                        file.offset = write_offset as u64 + n as u64;
                        n as u64
                    }
                    Err(_) => u64::MAX,
                }
            }
        };
    }

    // ── TUN/TAP fd intercept ────────────────────────────────────────────────
    if crate::net::tuntap::tun_is_fd(fd as i32) {
        let n = crate::net::tuntap::tun_write(fd as i32, slice);
        return if n < 0 { u64::MAX } else { n as u64 };
    }

    // ── PTY fd intercept ─────────────────────────────────────────────────────
    if crate::drivers::pty::pty_is_master_fd(fd as i32) {
        let n = crate::drivers::pty::pty_master_write(fd as i32, slice);
        return if n < 0 { u64::MAX } else { n as u64 };
    }
    if crate::drivers::pty::pty_is_slave_fd(fd as i32) {
        let n = crate::drivers::pty::pty_slave_write(fd as i32, slice);
        return if n < 0 { u64::MAX } else { n as u64 };
    }

    match fd {
        1 => {
            for &byte in slice {
                if byte == b'\n' || (byte >= 0x20 && byte <= 0x7e) {
                    kprint!("{}", byte as char);
                }
            }
            count as u64
        }
        2 => {
            for &byte in slice {
                serial_print!("{}", byte as char);
            }
            count as u64
        }
        _ => u64::MAX,
    }
}

// ─── SYS_READ ─────────────────────────────────────────────────────────────────

/// SYS_READ: Read bytes from a file descriptor
pub fn sys_read(fd: u64, buf: *mut u8, count: usize) -> u64 {
    if buf.is_null() || count == 0 {
        return 0;
    }

    let pid = process::getpid();
    let mapped = {
        let table = PROCESS_FDS.lock();
        table.get_fd(pid, fd as u32)
    };

    if let Some(fd_kind) = mapped {
        return match fd_kind {
            KernelFd::PipeRead(pipe_id) => {
                let slice = unsafe { core::slice::from_raw_parts_mut(buf, count) };
                match crate::ipc::pipe::read(pipe_id, slice) {
                    Ok(n) => n as u64,
                    Err("would block") => 0,
                    Err(_) => u64::MAX,
                }
            }
            KernelFd::PipeWrite(_) => u64::MAX,
            KernelFd::File(handle_id) => {
                let slice = unsafe { core::slice::from_raw_parts_mut(buf, count) };
                let mut table = OPEN_FILES.lock();
                let file = match table.get_mut(handle_id) {
                    Some(file) => file,
                    None => return u64::MAX,
                };
                if !can_read_flags(file.flags) {
                    return u64::MAX;
                }

                match crate::fs::vfs::fs_read_at(&file.path, file.offset as usize, slice) {
                    Ok(n) => {
                        file.offset = file.offset.saturating_add(n as u64);
                        n as u64
                    }
                    Err(_) => u64::MAX,
                }
            }
        };
    }

    // ── TUN/TAP fd intercept ────────────────────────────────────────────────
    if crate::net::tuntap::tun_is_fd(fd as i32) {
        // tun_read needs a fixed-size buffer; use a stack buffer then copy.
        let mut tmp = [0u8; 1500];
        let n = crate::net::tuntap::tun_read(fd as i32, &mut tmp);
        if n < 0 {
            return u64::MAX;
        }
        let copy = (n as usize).min(count);
        let dest = unsafe { core::slice::from_raw_parts_mut(buf, copy) };
        dest[..copy].copy_from_slice(&tmp[..copy]);
        return copy as u64;
    }

    // ── PTY fd intercept ─────────────────────────────────────────────────────
    if crate::drivers::pty::pty_is_master_fd(fd as i32) {
        let mut tmp = [0u8; 4096];
        let n = crate::drivers::pty::pty_master_read(fd as i32, &mut tmp);
        if n == -11 {
            return super::errno::EAGAIN;
        }
        if n < 0 {
            return u64::MAX;
        }
        let copy = (n as usize).min(count);
        let dest = unsafe { core::slice::from_raw_parts_mut(buf, copy) };
        dest[..copy].copy_from_slice(&tmp[..copy]);
        return copy as u64;
    }
    if crate::drivers::pty::pty_is_slave_fd(fd as i32) {
        let mut tmp = [0u8; 4096];
        let n = crate::drivers::pty::pty_slave_read(fd as i32, &mut tmp);
        if n == -11 {
            return super::errno::EAGAIN;
        }
        if n < 0 {
            return u64::MAX;
        }
        let copy = (n as usize).min(count);
        let dest = unsafe { core::slice::from_raw_parts_mut(buf, copy) };
        dest[..copy].copy_from_slice(&tmp[..copy]);
        return copy as u64;
    }

    match fd {
        0 => {
            let mut bytes_read = 0usize;
            let slice = unsafe { core::slice::from_raw_parts_mut(buf, count) };
            while bytes_read < count {
                if let Some(event) = crate::drivers::keyboard::pop_key() {
                    if event.pressed && event.character != '\0' {
                        slice[bytes_read] = event.character as u8;
                        bytes_read += 1;
                        if event.character == '\n' {
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
            bytes_read as u64
        }
        _ => u64::MAX,
    }
}

// ─── SYS_OPEN ─────────────────────────────────────────────────────────────────

/// SYS_OPEN: open a file and return a process-local FD
pub fn sys_open(path_ptr: *const u8, path_len: usize, flags: u32) -> u64 {
    let pid = process::getpid();
    let raw_path = match read_user_path(path_ptr, path_len) {
        Ok(path) => path,
        Err(_) => return u64::MAX,
    };
    let path = resolve_process_path(pid, &raw_path);
    let access = access_mode(flags);
    if access > crate::fs::vfs::flags::O_RDWR {
        return u64::MAX;
    }

    if flags & crate::fs::vfs::flags::O_CREAT != 0 {
        if crate::fs::vfs::fs_stat(&path).is_err() && crate::fs::vfs::fs_write(&path, &[]).is_err()
        {
            return u64::MAX;
        }
    }

    let (file_type, file_size) = match crate::fs::vfs::fs_stat(&path) {
        Ok(stat) => stat,
        Err(_) => return u64::MAX,
    };
    if file_type == crate::fs::vfs::FileType::Directory {
        return u64::MAX;
    }

    if flags & crate::fs::vfs::flags::O_TRUNC != 0 {
        if !can_write_flags(flags) {
            return u64::MAX;
        }
        if crate::fs::vfs::fs_write(&path, &[]).is_err() {
            return u64::MAX;
        }
    }

    let start_offset = if flags & crate::fs::vfs::flags::O_APPEND != 0 {
        match crate::fs::vfs::fs_stat(&path) {
            Ok((_, size)) => size,
            Err(_) => file_size,
        }
    } else {
        0
    };

    let handle_id = OPEN_FILES.lock().insert(OpenFile {
        path,
        flags,
        offset: start_offset,
    });

    PROCESS_FDS.lock().alloc_fd(pid, KernelFd::File(handle_id)) as u64
}

// ─── SYS_CLOSE ────────────────────────────────────────────────────────────────

/// SYS_CLOSE: close a file descriptor
pub fn sys_close(fd: u64) -> u64 {
    if fd > u32::MAX as u64 {
        return u64::MAX;
    }
    let fd = fd as u32;
    let pid = process::getpid();

    if close_local_fd_mapping(pid, fd) {
        return 0;
    }

    if fd <= 2 {
        return 0;
    }

    u64::MAX
}

// ─── SYS_PIPE ─────────────────────────────────────────────────────────────────

/// SYS_PIPE: Create a pipe pair
pub fn sys_pipe(fds_ptr: *mut u32) -> u64 {
    if fds_ptr.is_null() {
        return u64::MAX;
    }
    let pid = process::getpid();
    match crate::ipc::pipe::create(pid, pid) {
        Ok(pipe_id) => {
            let (read_fd, write_fd) = {
                let mut table = PROCESS_FDS.lock();
                let read = table.alloc_fd(pid, KernelFd::PipeRead(pipe_id));
                let write = table.alloc_fd(pid, KernelFd::PipeWrite(pipe_id));
                (read, write)
            };
            unsafe {
                *fds_ptr = read_fd;
                *fds_ptr.add(1) = write_fd;
            }
            0
        }
        Err(_) => u64::MAX,
    }
}

// ─── SYS_DUP ──────────────────────────────────────────────────────────────────

/// SYS_DUP: duplicate a file descriptor, returning the lowest available fd
pub fn sys_dup(old_fd: u64) -> u64 {
    if old_fd > u32::MAX as u64 {
        return errno::EBADF;
    }
    let old_fd = old_fd as u32;
    let pid = process::getpid();

    let source = {
        let table = PROCESS_FDS.lock();
        table.get_fd(pid, old_fd)
    };

    match source {
        Some(kind) => {
            let new_fd = PROCESS_FDS.lock().alloc_fd(pid, kind);
            new_fd as u64
        }
        None => errno::EBADF,
    }
}

// ─── SYS_DUP2 ─────────────────────────────────────────────────────────────────

/// SYS_DUP2: duplicate old fd onto new fd
pub fn sys_dup2(old_fd: u64, new_fd: u64) -> u64 {
    if old_fd > u32::MAX as u64 || new_fd > u32::MAX as u64 {
        return u64::MAX;
    }
    let old_fd = old_fd as u32;
    let new_fd = new_fd as u32;
    let pid = process::getpid();

    if old_fd == new_fd {
        return new_fd as u64;
    }

    let source = {
        let table = PROCESS_FDS.lock();
        table.get_fd(pid, old_fd)
    };

    match source {
        Some(kind) => {
            let replaced = {
                let mut table = PROCESS_FDS.lock();
                table.set_fd(pid, new_fd, kind)
            };

            if let Some(prev) = replaced {
                if prev != kind {
                    let should_close = {
                        let table = PROCESS_FDS.lock();
                        !table.has_kind(prev)
                    };
                    if should_close {
                        close_kernel_fd(prev);
                    }
                }
            }
            new_fd as u64
        }
        None => {
            if old_fd <= 2 {
                close_local_fd_mapping(pid, new_fd);
                new_fd as u64
            } else {
                u64::MAX
            }
        }
    }
}

// ─── SYS_LSEEK ────────────────────────────────────────────────────────────────

/// SYS_LSEEK: reposition file read/write offset
///
/// whence: 0=SEEK_SET, 1=SEEK_CUR, 2=SEEK_END
pub fn sys_lseek(fd: u64, offset: i64, whence: u32) -> u64 {
    if fd > u32::MAX as u64 {
        return errno::EBADF;
    }
    let pid = process::getpid();
    let mapped = {
        let table = PROCESS_FDS.lock();
        table.get_fd(pid, fd as u32)
    };

    match mapped {
        Some(KernelFd::File(handle_id)) => {
            let mut table = OPEN_FILES.lock();
            let file = match table.get_mut(handle_id) {
                Some(f) => f,
                None => return errno::EBADF,
            };

            let new_offset = match whence {
                0 => {
                    // SEEK_SET
                    if offset < 0 {
                        return errno::EINVAL;
                    }
                    offset as u64
                }
                1 => {
                    // SEEK_CUR
                    let cur = file.offset as i64;
                    let new = cur.saturating_add(offset);
                    if new < 0 {
                        return errno::EINVAL;
                    }
                    new as u64
                }
                2 => {
                    // SEEK_END
                    let size = match crate::fs::vfs::fs_stat(&file.path) {
                        Ok((_, s)) => s as i64,
                        Err(_) => return errno::EIO,
                    };
                    let new = size.saturating_add(offset);
                    if new < 0 {
                        return errno::EINVAL;
                    }
                    new as u64
                }
                _ => return errno::EINVAL,
            };

            file.offset = new_offset;
            new_offset
        }
        Some(KernelFd::PipeRead(_)) | Some(KernelFd::PipeWrite(_)) => errno::EINVAL,
        None => errno::EBADF,
    }
}

// ─── SYS_STAT ─────────────────────────────────────────────────────────────────

/// SYS_STAT: Get file status
pub fn sys_stat(path: *const u8, len: usize, stat_buf: *mut u8) -> u64 {
    if path.is_null() || stat_buf.is_null() || len == 0 {
        return u64::MAX;
    }
    let path_slice = unsafe { core::slice::from_raw_parts(path, len) };
    let path_str = core::str::from_utf8(path_slice).unwrap_or("");

    match crate::fs::vfs::fs_stat(path_str) {
        Ok((ftype, size)) => {
            let out = unsafe { core::slice::from_raw_parts_mut(stat_buf, 16) };
            out[0..8].copy_from_slice(&(size as u64).to_le_bytes());
            out[8..12].copy_from_slice(&(ftype as u32).to_le_bytes());
            out[12..16].copy_from_slice(&0o644u32.to_le_bytes());
            0
        }
        Err(_) => u64::MAX,
    }
}

// ─── SYS_FSTAT ────────────────────────────────────────────────────────────────

/// SYS_FSTAT: get file status by file descriptor
pub fn sys_fstat(fd: u64, stat_buf: *mut u8) -> u64 {
    if stat_buf.is_null() || fd > u32::MAX as u64 {
        return errno::EINVAL;
    }
    let pid = process::getpid();
    let mapped = {
        let table = PROCESS_FDS.lock();
        table.get_fd(pid, fd as u32)
    };

    match mapped {
        Some(KernelFd::File(handle_id)) => {
            let path = {
                let table = OPEN_FILES.lock();
                match table.files.get(&handle_id) {
                    Some(f) => f.path.clone(),
                    None => return errno::EBADF,
                }
            };
            match crate::fs::vfs::fs_stat(&path) {
                Ok((ftype, size)) => {
                    let out = unsafe { core::slice::from_raw_parts_mut(stat_buf, 16) };
                    out[0..8].copy_from_slice(&(size as u64).to_le_bytes());
                    out[8..12].copy_from_slice(&(ftype as u32).to_le_bytes());
                    out[12..16].copy_from_slice(&0o644u32.to_le_bytes());
                    0
                }
                Err(_) => errno::EIO,
            }
        }
        Some(KernelFd::PipeRead(_)) | Some(KernelFd::PipeWrite(_)) => {
            let out = unsafe { core::slice::from_raw_parts_mut(stat_buf, 16) };
            out[0..8].copy_from_slice(&0u64.to_le_bytes());
            out[8..12].copy_from_slice(&0x1000u32.to_le_bytes()); // S_IFIFO
            out[12..16].copy_from_slice(&0o600u32.to_le_bytes());
            0
        }
        None => {
            if fd <= 2 {
                let out = unsafe { core::slice::from_raw_parts_mut(stat_buf, 16) };
                out[0..8].copy_from_slice(&0u64.to_le_bytes());
                out[8..12].copy_from_slice(&0x2000u32.to_le_bytes()); // S_IFCHR
                out[12..16].copy_from_slice(&0o620u32.to_le_bytes());
                0
            } else {
                errno::EBADF
            }
        }
    }
}

// ─── SYS_FCNTL ────────────────────────────────────────────────────────────────

/// SYS_FCNTL: file control operations
///
/// cmd: F_DUPFD=0, F_GETFD=1, F_SETFD=2, F_GETFL=3, F_SETFL=4
pub fn sys_fcntl(fd: u64, cmd: u32, _arg: u64) -> u64 {
    if fd > u32::MAX as u64 {
        return errno::EBADF;
    }

    match cmd {
        0 => sys_dup(fd),
        1 => 0,
        2 => 0,
        3 => {
            let pid = process::getpid();
            let mapped = {
                let table = PROCESS_FDS.lock();
                table.get_fd(pid, fd as u32)
            };
            match mapped {
                Some(KernelFd::File(handle_id)) => {
                    let table = OPEN_FILES.lock();
                    match table.files.get(&handle_id) {
                        Some(f) => f.flags as u64,
                        None => errno::EBADF,
                    }
                }
                Some(KernelFd::PipeRead(_)) => 0,
                Some(KernelFd::PipeWrite(_)) => 1,
                None => {
                    if fd <= 2 {
                        2
                    } else {
                        errno::EBADF
                    }
                }
            }
        }
        4 => 0,
        _ => errno::EINVAL,
    }
}

// ─── SYS_GETDENTS ─────────────────────────────────────────────────────────────

/// SYS_GETDENTS: get directory entries
pub fn sys_getdents(fd: u64, buf: *mut u8, buf_size: usize) -> u64 {
    if buf.is_null() || buf_size == 0 || fd > u32::MAX as u64 {
        return errno::EINVAL;
    }

    let pid = process::getpid();
    let mapped = {
        let table = PROCESS_FDS.lock();
        table.get_fd(pid, fd as u32)
    };

    let path = match mapped {
        Some(KernelFd::File(handle_id)) => {
            let table = OPEN_FILES.lock();
            match table.files.get(&handle_id) {
                Some(f) => f.path.clone(),
                None => return errno::EBADF,
            }
        }
        _ => return errno::EBADF,
    };

    let entries = match crate::fs::vfs::fs_ls(&path) {
        Ok(e) => e,
        Err(_) => return errno::ENOTDIR,
    };

    let out = unsafe { core::slice::from_raw_parts_mut(buf, buf_size) };
    let mut offset = 0usize;

    for (name, ftype, _size) in &entries {
        let name_bytes = name.as_bytes();
        let rec_len = 3 + name_bytes.len() + 1;
        if offset + rec_len > buf_size {
            break;
        }
        out[offset] = (rec_len & 0xFF) as u8;
        out[offset + 1] = ((rec_len >> 8) & 0xFF) as u8;
        out[offset + 2] = match ftype {
            crate::fs::vfs::FileType::Directory => 4,
            crate::fs::vfs::FileType::Symlink => 10,
            _ => 8,
        };
        out[offset + 3..offset + 3 + name_bytes.len()].copy_from_slice(name_bytes);
        out[offset + 3 + name_bytes.len()] = 0;
        offset += rec_len;
    }

    offset as u64
}

// ─── SYS_IOCTL ────────────────────────────────────────────────────────────────

/// SYS_IOCTL: device I/O control
///
/// Handles PTY-related ioctls (TIOCGPTN, TIOCSWINSZ, TIOCGWINSZ, TIOCSPTLCK,
/// TCGETS, TCSETS, TCSETSW, TCSETSF) by delegating to the PTY subsystem.
/// Other requests are handled inline or return 0 (success stub).
pub fn sys_ioctl(fd: u64, request: u64, arg: u64) -> u64 {
    let fd_i32 = fd as i32;

    // ── PTY ioctl intercept ──────────────────────────────────────────────────
    if crate::drivers::pty::pty_is_fd(fd_i32) {
        let ret = crate::drivers::pty::pty_ioctl(fd_i32, request, arg);
        return if ret < 0 { u64::MAX } else { ret as u64 };
    }

    // ── PTY-class ioctls on any fd (e.g. TIOCGWINSZ on stdin) ───────────────
    match request {
        crate::drivers::pty::TIOCGWINSZ => {
            // Non-PTY fd: return a default 80x24 window size if arg is valid.
            if arg != 0 {
                let ws = crate::drivers::pty::WinSize {
                    ws_row: 24,
                    ws_col: 80,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                unsafe {
                    core::ptr::write(arg as *mut crate::drivers::pty::WinSize, ws);
                }
            }
            0
        }
        _ => 0,
    }
}

// ─── SYS_MKDIR ────────────────────────────────────────────────────────────────

/// SYS_MKDIR: create a directory
pub fn sys_mkdir(path_ptr: *const u8, path_len: usize, _mode: u32) -> u64 {
    let pid = process::getpid();
    let raw_path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let path = resolve_process_path(pid, &raw_path);
    match crate::fs::vfs::fs_mkdir(&path) {
        Ok(()) => 0,
        Err(_) => errno::EEXIST,
    }
}

// ─── SYS_RMDIR ────────────────────────────────────────────────────────────────

/// SYS_RMDIR: remove a directory
pub fn sys_rmdir(path_ptr: *const u8, path_len: usize) -> u64 {
    let pid = process::getpid();
    let raw_path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let path = resolve_process_path(pid, &raw_path);

    match crate::fs::vfs::fs_stat(&path) {
        Ok((crate::fs::vfs::FileType::Directory, _)) => {}
        Ok(_) => return errno::ENOTDIR,
        Err(_) => return errno::ENOENT,
    }

    match crate::fs::vfs::fs_ls(&path) {
        Ok(entries) if !entries.is_empty() => return errno::ENOTEMPTY,
        Err(_) => return errno::EIO,
        _ => {}
    }

    match crate::fs::vfs::fs_rm(&path) {
        Ok(()) => 0,
        Err(_) => errno::EIO,
    }
}

// ─── SYS_UNLINK ───────────────────────────────────────────────────────────────

/// SYS_UNLINK: remove a file (not a directory)
pub fn sys_unlink(path_ptr: *const u8, path_len: usize) -> u64 {
    let pid = process::getpid();
    let raw_path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let path = resolve_process_path(pid, &raw_path);

    match crate::fs::vfs::fs_stat(&path) {
        Ok((crate::fs::vfs::FileType::Directory, _)) => return errno::EISDIR,
        Ok(_) => {}
        Err(_) => return errno::ENOENT,
    }

    match crate::fs::vfs::fs_rm(&path) {
        Ok(()) => 0,
        Err(_) => errno::EIO,
    }
}

// ─── SYS_RENAME ───────────────────────────────────────────────────────────────

/// SYS_RENAME: rename/move a file or directory
pub fn sys_rename(old_ptr: *const u8, old_len: usize, new_ptr: *const u8, new_len: usize) -> u64 {
    let pid = process::getpid();
    let old_raw = match read_user_path(old_ptr, old_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let new_raw = match read_user_path(new_ptr, new_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let old_path = resolve_process_path(pid, &old_raw);
    let new_path = resolve_process_path(pid, &new_raw);

    let data = match crate::fs::vfs::fs_read(&old_path) {
        Ok(d) => d,
        Err(_) => return errno::ENOENT,
    };

    if crate::fs::vfs::fs_write(&new_path, &data).is_err() {
        return errno::EIO;
    }

    if crate::fs::vfs::fs_rm(&old_path).is_err() {
        return errno::EIO;
    }

    0
}

// ─── SYS_CHMOD ────────────────────────────────────────────────────────────────

/// SYS_CHMOD: change file permissions
pub fn sys_chmod(path_ptr: *const u8, path_len: usize, mode: u32) -> u64 {
    let pid = process::getpid();
    let raw_path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let path = resolve_process_path(pid, &raw_path);

    match crate::fs::vfs::fs_chmod(&path, mode) {
        Ok(()) => 0,
        Err(_) => errno::ENOENT,
    }
}

// ─── SYS_CHOWN ────────────────────────────────────────────────────────────────

/// SYS_CHOWN: change file ownership (requires CAP_CHOWN)
pub fn sys_chown(path_ptr: *const u8, path_len: usize, _uid: u32, _gid: u32) -> u64 {
    let pid = process::getpid();
    let raw_path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let path = resolve_process_path(pid, &raw_path);

    match crate::fs::vfs::fs_stat(&path) {
        Ok(_) => {
            let table = process::pcb::PROCESS_TABLE.lock();
            let uid = table[pid as usize]
                .as_ref()
                .map(|p| p.uid)
                .unwrap_or(u32::MAX);
            if uid != 0 {
                return errno::EPERM;
            }
            0
        }
        Err(_) => errno::ENOENT,
    }
}

// ─── SYS_SYMLINK ──────────────────────────────────────────────────────────────

/// SYS_SYMLINK: create a symbolic link
pub fn sys_symlink(
    target_ptr: *const u8,
    target_len: usize,
    link_ptr: *const u8,
    link_len: usize,
) -> u64 {
    let pid = process::getpid();
    let target = match read_user_path(target_ptr, target_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let link_raw = match read_user_path(link_ptr, link_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let link_path = resolve_process_path(pid, &link_raw);

    match crate::fs::vfs::fs_symlink(&link_path, &target) {
        Ok(()) => 0,
        Err(_) => errno::EIO,
    }
}

// ─── SYS_READLINK ─────────────────────────────────────────────────────────────

/// SYS_READLINK: read the target of a symbolic link
pub fn sys_readlink(path_ptr: *const u8, path_len: usize, buf: *mut u8, buf_size: usize) -> u64 {
    if buf.is_null() || buf_size == 0 {
        return errno::EINVAL;
    }
    let pid = process::getpid();
    let raw_path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let path = resolve_process_path(pid, &raw_path);

    match crate::fs::vfs::fs_readlink(&path) {
        Ok(target) => {
            let target_bytes = target.as_bytes();
            let copy_len = target_bytes.len().min(buf_size);
            let out = unsafe { core::slice::from_raw_parts_mut(buf, copy_len) };
            out.copy_from_slice(&target_bytes[..copy_len]);
            copy_len as u64
        }
        Err(_) => errno::EINVAL,
    }
}

// ─── SYS_TRUNCATE ─────────────────────────────────────────────────────────────

/// SYS_TRUNCATE: truncate a file to a specified length
pub fn sys_truncate(path_ptr: *const u8, path_len: usize, length: u64) -> u64 {
    let pid = process::getpid();
    let raw_path = match read_user_path(path_ptr, path_len) {
        Ok(p) => p,
        Err(_) => return errno::EFAULT,
    };
    let path = resolve_process_path(pid, &raw_path);

    let data = match crate::fs::vfs::fs_read(&path) {
        Ok(d) => d,
        Err(_) => return errno::ENOENT,
    };

    let new_len = length as usize;
    if new_len >= data.len() {
        let mut new_data = data;
        new_data.resize(new_len, 0);
        match crate::fs::vfs::fs_write(&path, &new_data) {
            Ok(()) => 0,
            Err(_) => errno::EIO,
        }
    } else {
        match crate::fs::vfs::fs_write(&path, &data[..new_len]) {
            Ok(()) => 0,
            Err(_) => errno::EIO,
        }
    }
}

// ─── SYS_GETCWD ───────────────────────────────────────────────────────────────

/// SYS_GETCWD: Get current working directory
pub fn sys_getcwd(buf: *mut u8, size: usize) -> u64 {
    if buf.is_null() || size == 0 {
        return u64::MAX;
    }
    let pid = process::getpid();
    let table = process::pcb::PROCESS_TABLE.lock();
    let cwd = table[pid as usize]
        .as_ref()
        .map(|p| p.cwd.as_bytes())
        .unwrap_or(b"/");
    let len = cwd.len().min(size - 1);
    let slice = unsafe { core::slice::from_raw_parts_mut(buf, len + 1) };
    slice[..len].copy_from_slice(&cwd[..len]);
    slice[len] = 0;
    len as u64
}

// ─── SYS_CHDIR ────────────────────────────────────────────────────────────────

/// SYS_CHDIR: Change current working directory
pub fn sys_chdir(path: *const u8, len: usize) -> u64 {
    if path.is_null() || len == 0 {
        return u64::MAX;
    }
    let path_slice = unsafe { core::slice::from_raw_parts(path, len) };
    let path_str = core::str::from_utf8(path_slice).unwrap_or("/");

    if crate::fs::vfs::fs_stat(path_str).is_err() {
        return u64::MAX;
    }

    let pid = process::getpid();
    let mut table = process::pcb::PROCESS_TABLE.lock();
    if let Some(proc) = table[pid as usize].as_mut() {
        proc.cwd = alloc::string::String::from(path_str);
    }
    0
}

// ─── SYS_UNAME ────────────────────────────────────────────────────────────────

/// SYS_UNAME: get system identification
///
/// Writes a utsname-compatible struct:
///   [0..65]   sysname
///   [65..130] nodename
///   [130..195] release
///   [195..260] version
///   [260..325] machine
pub fn sys_uname(buf: *mut u8, buf_size: usize) -> u64 {
    if buf.is_null() || buf_size < 325 {
        return errno::EFAULT;
    }

    let out = unsafe { core::slice::from_raw_parts_mut(buf, 325) };
    for b in out.iter_mut() {
        *b = 0;
    }

    let fields: [&[u8]; 5] = [
        b"Genesis",
        b"genesis",
        b"0.9.0",
        b"#1 SMP Genesis",
        b"x86_64",
    ];

    for (i, field) in fields.iter().enumerate() {
        let start = i * 65;
        let len = field.len().min(64);
        out[start..start + len].copy_from_slice(&field[..len]);
    }

    0
}

// ─── SYS_SYSINFO ──────────────────────────────────────────────────────────────

/// SYS_SYSINFO: get system information
///
/// Writes a simplified sysinfo struct (28 bytes):
///   [0..8]   uptime (seconds)
///   [8..16]  totalram
///   [16..24] freeram
///   [24..28] procs
pub fn sys_sysinfo(buf: *mut u8, buf_size: usize) -> u64 {
    if buf.is_null() || buf_size < 28 {
        return errno::EFAULT;
    }

    let out = unsafe { core::slice::from_raw_parts_mut(buf, 28) };

    let uptime = crate::time::clock::uptime_secs();
    out[0..8].copy_from_slice(&uptime.to_le_bytes());

    let fa = crate::memory::frame_allocator::FRAME_ALLOCATOR.lock();
    let total = crate::memory::frame_allocator::MAX_MEMORY as u64;
    let free = (fa.free_count() * crate::memory::frame_allocator::FRAME_SIZE) as u64;
    drop(fa);

    out[8..16].copy_from_slice(&total.to_le_bytes());
    out[16..24].copy_from_slice(&free.to_le_bytes());

    let table = process::pcb::PROCESS_TABLE.lock();
    let procs: u32 = table.iter().filter(|s| s.is_some()).count() as u32;
    out[24..28].copy_from_slice(&procs.to_le_bytes());

    0
}
