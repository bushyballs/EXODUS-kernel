/// Memory mapping flags and operations (POSIX mman)
///
/// Part of the AIOS compatibility layer.
///
/// Provides POSIX-compatible mmap/munmap/mprotect/msync operations.
/// Manages a per-process table of virtual memory mappings that track
/// address, length, protection, flags, and backing (anonymous or file).
///
/// Design:
///   - Memory region descriptors track each mmap'd region.
///   - mmap allocates virtual address ranges from a per-process VMA list.
///   - munmap removes and splits regions as needed.
///   - mprotect changes protection flags on existing regions.
///   - MAP_ANONYMOUS provides zero-filled memory (no file backing).
///   - MAP_FIXED places the mapping at an exact address.
///   - Global Mutex<Option<Inner>> singleton.
///
/// Inspired by: POSIX sys/mman.h, Linux mm/mmap.c. All code is original.

use alloc::vec::Vec;
use crate::sync::Mutex;
use crate::serial_println;

// ---------------------------------------------------------------------------
// Protection flags
// ---------------------------------------------------------------------------

pub const PROT_NONE: u32 = 0x0;
pub const PROT_READ: u32 = 0x1;
pub const PROT_WRITE: u32 = 0x2;
pub const PROT_EXEC: u32 = 0x4;

// ---------------------------------------------------------------------------
// Mapping flags
// ---------------------------------------------------------------------------

pub const MAP_SHARED: u32 = 0x01;
pub const MAP_PRIVATE: u32 = 0x02;
pub const MAP_FIXED: u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;
pub const MAP_GROWSDOWN: u32 = 0x0100;
pub const MAP_DENYWRITE: u32 = 0x0800;
pub const MAP_NORESERVE: u32 = 0x4000;
pub const MAP_POPULATE: u32 = 0x8000;
pub const MAP_NONBLOCK: u32 = 0x10000;
pub const MAP_STACK: u32 = 0x20000;
pub const MAP_HUGETLB: u32 = 0x40000;

pub const MAP_FAILED: usize = usize::MAX;

// msync flags
pub const MS_ASYNC: u32 = 1;
pub const MS_SYNC: u32 = 4;
pub const MS_INVALIDATE: u32 = 2;

// madvise flags
pub const MADV_NORMAL: u32 = 0;
pub const MADV_RANDOM: u32 = 1;
pub const MADV_SEQUENTIAL: u32 = 2;
pub const MADV_WILLNEED: u32 = 3;
pub const MADV_DONTNEED: u32 = 4;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const PAGE_SIZE: usize = 4096;

/// Default base address for new anonymous mappings.
const MMAP_BASE: usize = 0x7F00_0000_0000;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single virtual memory region.
#[derive(Clone)]
struct MmapRegion {
    start: usize,
    length: usize,
    prot: u32,
    flags: u32,
    /// File descriptor (-1 for anonymous)
    fd: i32,
    /// File offset
    offset: i64,
}

/// Per-process VMA tracker.
struct ProcessVma {
    pid: u32,
    regions: Vec<MmapRegion>,
    next_addr: usize,
}

impl ProcessVma {
    fn new(pid: u32) -> Self {
        ProcessVma {
            pid,
            regions: Vec::new(),
            next_addr: MMAP_BASE,
        }
    }

    fn page_align(size: usize) -> usize {
        (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
    }

    /// Find a gap large enough for the requested length.
    fn find_free(&self, length: usize) -> usize {
        let aligned_len = Self::page_align(length);
        let mut candidate = self.next_addr;

        'outer: loop {
            let cand_end = candidate + aligned_len;
            for region in self.regions.iter() {
                let r_end = region.start + region.length;
                if candidate < r_end && cand_end > region.start {
                    candidate = Self::page_align(r_end);
                    continue 'outer;
                }
            }
            return candidate;
        }
    }

    fn mmap(
        &mut self,
        addr: usize,
        length: usize,
        prot: u32,
        flags: u32,
        fd: i32,
        offset: i64,
    ) -> usize {
        if length == 0 {
            return MAP_FAILED;
        }

        let aligned_len = Self::page_align(length);

        let start = if flags & MAP_FIXED != 0 {
            // Unmap any existing overlapping regions
            let end = addr + aligned_len;
            self.regions.retain(|r| {
                let r_end = r.start + r.length;
                !(r.start < end && r_end > addr)
            });
            addr
        } else if addr != 0 {
            // Hint: try this address, fall back to find_free
            let conflict = self.regions.iter().any(|r| {
                let r_end = r.start + r.length;
                addr < r_end && (addr + aligned_len) > r.start
            });
            if conflict {
                self.find_free(aligned_len)
            } else {
                addr
            }
        } else {
            self.find_free(aligned_len)
        };

        self.regions.push(MmapRegion {
            start,
            length: aligned_len,
            prot,
            flags,
            fd,
            offset,
        });

        let end = start + aligned_len;
        if end > self.next_addr {
            self.next_addr = end;
        }

        start
    }

    fn munmap(&mut self, addr: usize, length: usize) -> Result<(), i32> {
        if length == 0 || addr % PAGE_SIZE != 0 {
            return Err(-22); // EINVAL
        }

        let aligned_len = Self::page_align(length);
        let unmap_end = addr + aligned_len;
        let mut new_regions = Vec::new();
        let mut found = false;

        for region in self.regions.iter() {
            let r_end = region.start + region.length;

            if region.start >= addr && r_end <= unmap_end {
                // Entirely within unmap range
                found = true;
            } else if region.start < addr && r_end > unmap_end {
                // Splits into two pieces
                found = true;
                new_regions.push(MmapRegion {
                    start: region.start,
                    length: addr - region.start,
                    prot: region.prot,
                    flags: region.flags,
                    fd: region.fd,
                    offset: region.offset,
                });
                let right_off = region.offset + (unmap_end - region.start) as i64;
                new_regions.push(MmapRegion {
                    start: unmap_end,
                    length: r_end - unmap_end,
                    prot: region.prot,
                    flags: region.flags,
                    fd: region.fd,
                    offset: right_off,
                });
            } else if region.start < unmap_end && r_end > unmap_end {
                // Partial overlap at start
                found = true;
                new_regions.push(MmapRegion {
                    start: unmap_end,
                    length: r_end - unmap_end,
                    prot: region.prot,
                    flags: region.flags,
                    fd: region.fd,
                    offset: region.offset + (unmap_end - region.start) as i64,
                });
            } else if region.start < addr && r_end > addr {
                // Partial overlap at end
                found = true;
                new_regions.push(MmapRegion {
                    start: region.start,
                    length: addr - region.start,
                    prot: region.prot,
                    flags: region.flags,
                    fd: region.fd,
                    offset: region.offset,
                });
            } else {
                // No overlap
                new_regions.push(region.clone());
            }
        }

        if !found {
            return Err(-22);
        }

        self.regions = new_regions;
        Ok(())
    }

    fn mprotect(&mut self, addr: usize, length: usize, prot: u32) -> Result<(), i32> {
        if length == 0 || addr % PAGE_SIZE != 0 {
            return Err(-22);
        }
        let aligned_len = Self::page_align(length);
        let end = addr + aligned_len;
        let mut found = false;

        for region in self.regions.iter_mut() {
            let r_end = region.start + region.length;
            if region.start >= addr && r_end <= end {
                region.prot = prot;
                found = true;
            }
        }

        if found {
            Ok(())
        } else {
            Err(-12) // ENOMEM
        }
    }

    fn list_mappings(&self) -> Vec<(usize, usize, u32, u32, i32)> {
        self.regions
            .iter()
            .map(|r| (r.start, r.length, r.prot, r.flags, r.fd))
            .collect()
    }
}

/// Global inner state with per-process VMA tables.
struct Inner {
    processes: Vec<ProcessVma>,
    total_mapped: u64,
}

impl Inner {
    fn new() -> Self {
        Inner {
            processes: Vec::new(),
            total_mapped: 0,
        }
    }

    fn get_or_create(&mut self, pid: u32) -> &mut ProcessVma {
        let pos = self.processes.iter().position(|p| p.pid == pid);
        match pos {
            Some(idx) => &mut self.processes[idx],
            None => {
                self.processes.push(ProcessVma::new(pid));
                let last = self.processes.len() - 1;
                &mut self.processes[last]
            }
        }
    }

    fn remove_process(&mut self, pid: u32) {
        self.processes.retain(|p| p.pid != pid);
    }
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

static MMAN: Mutex<Option<Inner>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Perform a POSIX mmap operation. Returns the mapped address or MAP_FAILED.
pub fn mmap(
    addr: usize,
    len: usize,
    prot: u32,
    flags: u32,
    fd: i32,
    offset: i64,
) -> usize {
    mmap_pid(0, addr, len, prot, flags, fd, offset)
}

/// mmap with explicit pid (for multi-process support).
pub fn mmap_pid(
    pid: u32,
    addr: usize,
    len: usize,
    prot: u32,
    flags: u32,
    fd: i32,
    offset: i64,
) -> usize {
    let mut guard = MMAN.lock();
    match guard.as_mut() {
        Some(inner) => {
            let pv = inner.get_or_create(pid);
            let result = pv.mmap(addr, len, prot, flags, fd, offset);
            if result != MAP_FAILED {
                inner.total_mapped += len as u64;
            }
            result
        }
        None => MAP_FAILED,
    }
}

/// Unmap a memory region.
pub fn munmap(addr: usize, len: usize) -> i32 {
    munmap_pid(0, addr, len)
}

/// munmap with explicit pid.
pub fn munmap_pid(pid: u32, addr: usize, len: usize) -> i32 {
    let mut guard = MMAN.lock();
    match guard.as_mut() {
        Some(inner) => {
            let pv = inner.get_or_create(pid);
            match pv.munmap(addr, len) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        None => -1,
    }
}

/// Change protection on a memory region.
pub fn mprotect(addr: usize, len: usize, prot: u32) -> i32 {
    mprotect_pid(0, addr, len, prot)
}

/// mprotect with explicit pid.
pub fn mprotect_pid(pid: u32, addr: usize, len: usize, prot: u32) -> i32 {
    let mut guard = MMAN.lock();
    match guard.as_mut() {
        Some(inner) => {
            let pv = inner.get_or_create(pid);
            match pv.mprotect(addr, len, prot) {
                Ok(()) => 0,
                Err(e) => e,
            }
        }
        None => -1,
    }
}

/// List memory mappings for a process: (start, length, prot, flags, fd).
pub fn list_mappings(pid: u32) -> Vec<(usize, usize, u32, u32, i32)> {
    let mut guard = MMAN.lock();
    match guard.as_mut() {
        Some(inner) => {
            let pv = inner.get_or_create(pid);
            pv.list_mappings()
        }
        None => Vec::new(),
    }
}

/// Clean up all mappings for a process on exit.
pub fn cleanup(pid: u32) {
    let mut guard = MMAN.lock();
    if let Some(inner) = guard.as_mut() {
        inner.remove_process(pid);
    }
}

/// Return total bytes mapped across all processes.
pub fn total_mapped() -> u64 {
    let guard = MMAN.lock();
    guard.as_ref().map_or(0, |inner| inner.total_mapped)
}

/// Initialize the mman subsystem.
pub fn init() {
    let mut guard = MMAN.lock();
    *guard = Some(Inner::new());
    serial_println!("    mman: initialized (mmap/munmap/mprotect, anonymous/file-backed)");
}
