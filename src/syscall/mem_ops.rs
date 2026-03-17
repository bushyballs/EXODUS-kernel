/// Memory-management syscall handlers for Genesis
///
/// Implements: sys_mmap, sys_munmap, sys_mprotect, sys_brk, sys_mlock
///
/// All code is original.
use crate::process;

use super::errno;

// ─── SYS_MMAP ─────────────────────────────────────────────────────────────────

/// SYS_MMAP: Map anonymous or file-backed pages into the process virtual
/// address space.
///
/// Args (Linux mmap2 convention):
///   addr_hint  — hint virtual address (0 = kernel chooses)
///   length     — mapping length in bytes
///   prot       — PROT_READ | PROT_WRITE | PROT_EXEC
///   map_flags  — MAP_ANONYMOUS | MAP_PRIVATE | MAP_SHARED | MAP_FIXED
///   fd         — file descriptor (-1 for anonymous)
///   offset     — file offset (page-aligned, 0 for anonymous)
///
/// Returns: mapped virtual address on success, u64::MAX on failure.
pub fn sys_mmap_full(
    addr_hint: usize,
    length: usize,
    prot: u32,
    map_flags: u32,
    _fd: u64,
    offset: u64,
) -> u64 {
    use crate::memory::mmap;

    if length == 0 || length > usize::MAX - 0xFFF {
        return u64::MAX;
    }

    let pid = process::getpid();

    // File-backed fd->inode resolution is stubbed as anonymous until VFS fd
    // path is connected.
    let ino: u64 = 0;
    let file_offset = if map_flags & mmap::MAP_ANONYMOUS != 0 {
        0
    } else {
        offset
    };

    match mmap::mmap(addr_hint, length, prot, map_flags, ino, file_offset, pid) {
        Some(addr) => {
            let page_flags = prot_to_pte_flags(prot, map_flags);
            let num_pages = (length + 0xFFF) / 0x1000;
            let mut table = process::pcb::PROCESS_TABLE.lock();
            if let Some(proc) = table[pid as usize].as_mut() {
                proc.mmaps.push((addr, num_pages, page_flags));
            }
            addr as u64
        }
        None => u64::MAX,
    }
}

/// Convert POSIX prot + map flags to x86_64 PTE flags.
fn prot_to_pte_flags(prot: u32, map_flags: u32) -> u64 {
    use crate::memory::mmap;
    use crate::memory::paging::flags;
    let mut f = flags::PRESENT | flags::USER_ACCESSIBLE;
    if prot & mmap::PROT_WRITE != 0 && map_flags & mmap::MAP_SHARED != 0 {
        f |= flags::WRITABLE;
    }
    if prot & mmap::PROT_EXEC == 0 {
        f |= flags::NO_EXECUTE;
    }
    f
}

// ─── SYS_MUNMAP ───────────────────────────────────────────────────────────────

/// SYS_MUNMAP: Unmap a range of virtual address space.
///
/// Args:
///   addr   — start address (must be page-aligned)
///   length — length in bytes
pub fn sys_munmap(addr: usize, length: usize) -> u64 {
    if addr & 0xFFF != 0 || length == 0 {
        return u64::MAX;
    }

    let pid = process::getpid();

    if crate::memory::mmap::munmap(addr, length, pid) {
        let mut table = process::pcb::PROCESS_TABLE.lock();
        if let Some(proc) = table[pid as usize].as_mut() {
            let base = addr & !0xFFF;
            proc.mmaps.retain(|&(a, _, _)| a != base);
        }
    }
    // Per POSIX, munmap on an unmapped region is not an error.
    0
}

// ─── SYS_MPROTECT ─────────────────────────────────────────────────────────────

/// SYS_MPROTECT: Change protection flags on a mapped region.
///
/// Args:
///   addr   — start address (page-aligned)
///   length — length in bytes
///   prot   — new PROT_* flags
pub fn sys_mprotect(addr: usize, length: usize, prot: u32) -> u64 {
    use crate::memory::paging::flags;
    use crate::memory::{mmap, paging};

    if addr & 0xFFF != 0 || length == 0 {
        return u64::MAX;
    }

    let num_pages = (length + 0xFFF) / 0x1000;
    let base = addr & !0xFFF;

    let mut new_flags = flags::PRESENT | flags::USER_ACCESSIBLE;
    if prot & mmap::PROT_WRITE != 0 {
        new_flags |= flags::WRITABLE;
    }
    if prot & mmap::PROT_EXEC == 0 {
        new_flags |= flags::NO_EXECUTE;
    }

    for i in 0..num_pages {
        let virt = base.wrapping_add(i.wrapping_mul(0x1000));
        let _ = paging::change_permissions(virt, new_flags);
    }

    0
}

// ─── SYS_BRK ──────────────────────────────────────────────────────────────────

/// SYS_BRK: Set or query the program break (top of heap).
///
/// Arg: new_brk — desired new break address; 0 = query current break.
/// Returns: current break on success, current break on failure (Linux behaviour).
pub fn sys_brk(new_brk: usize) -> u64 {
    use crate::memory::paging::flags;
    use crate::memory::{frame_allocator, paging};

    let pid = process::getpid();

    let current_brk = {
        let table = process::pcb::PROCESS_TABLE.lock();
        table[pid as usize].as_ref().map(|p| p.brk).unwrap_or(0)
    };

    if new_brk == 0 || new_brk <= current_brk {
        return current_brk as u64;
    }

    let old_page_top = (current_brk + 0xFFF) & !0xFFF;
    let new_page_top = (new_brk + 0xFFF) & !0xFFF;

    if new_page_top <= old_page_top {
        let mut table = process::pcb::PROCESS_TABLE.lock();
        if let Some(proc) = table[pid as usize].as_mut() {
            proc.brk = new_brk;
        }
        return new_brk as u64;
    }

    let pml4_addr = paging::read_cr3();
    let mut virt = old_page_top;
    while virt < new_page_top {
        match frame_allocator::allocate_frame() {
            Some(frame) => {
                unsafe {
                    core::ptr::write_bytes(frame.addr as *mut u8, 0, 4096);
                }
                let page_flags = flags::WRITABLE | flags::USER_ACCESSIBLE | flags::NO_EXECUTE;
                if paging::map_page_in(pml4_addr, virt, frame.addr, page_flags | flags::PRESENT)
                    .is_err()
                {
                    frame_allocator::deallocate_frame(frame);
                    return current_brk as u64;
                }
            }
            None => return current_brk as u64,
        }
        virt = virt.wrapping_add(4096);
    }

    let mut table = process::pcb::PROCESS_TABLE.lock();
    if let Some(proc) = table[pid as usize].as_mut() {
        proc.brk = new_brk;
    }
    new_brk as u64
}

// ─── SYS_MLOCK ────────────────────────────────────────────────────────────────

/// SYS_MLOCK: lock pages in physical memory (stub — accepts but does not pin)
///
/// Full implementation would mark each PTE as non-swappable.
/// Genesis currently has no swap, so all resident pages are implicitly locked.
pub fn sys_mlock(addr: usize, length: usize) -> u64 {
    if addr & 0xFFF != 0 || length == 0 {
        return errno::EINVAL;
    }
    // No swap subsystem — all pages are always resident.  Accept silently.
    0
}

/// SYS_MUNLOCK: unlock previously locked pages (stub — symmetric with mlock)
pub fn sys_munlock(_addr: usize, _length: usize) -> u64 {
    0
}
