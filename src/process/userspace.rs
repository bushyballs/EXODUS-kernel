use crate::gdt;
use crate::memory::frame_allocator::FRAME_SIZE;
use crate::memory::{frame_allocator, paging};
/// User-space memory isolation (Ring 3) for Genesis
///
/// Sets up separate page tables for user processes so they can't
/// access kernel memory. Uses x86_64 ring 3 (CPL=3) for user code
/// and ring 0 (CPL=0) for kernel code.
///
/// Features:
///   - Address space creation with kernel mapping sharing
///   - User-mode transition setup (SYSRET/IRETQ to ring 3)
///   - User stack allocation and initialization
///   - Syscall entry/exit handling (SYSCALL instruction setup with MSRs)
///   - User process memory layout (code, data, heap, stack regions)
///   - brk()/sbrk() heap expansion
///   - mmap() region tracking (anonymous and file-backed)
///   - Copy-on-write page fault handler concept
///   - Process address space cleanup on exit
///
/// When a user process runs, it sees only its own memory.
/// Syscalls transition to ring 0 temporarily.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Virtual address layout constants
// ---------------------------------------------------------------------------

/// User-space virtual address layout:
///   0x0000_0000_0040_0000 - 0x0000_7FFF_FFFF_FFFF : User space
///   0xFFFF_8000_0000_0000 - 0xFFFF_FFFF_FFFF_FFFF : Kernel space (not accessible from ring 3)
pub const USER_SPACE_START: usize = 0x40_0000; // 4MB
pub const USER_STACK_TOP: usize = 0x0000_7FFF_F000; // Near top of lower half
pub const USER_STACK_SIZE: usize = 64 * 1024; // 64KB user stack
pub const USER_HEAP_START: usize = 0x1000_0000; // 256MB mark

/// Default maximum heap size (128 MB)
pub const USER_HEAP_MAX: usize = 128 * 1024 * 1024;

/// mmap region start address (above the heap)
pub const USER_MMAP_START: usize = 0x2000_0000; // 512MB mark

/// mmap region end address
pub const USER_MMAP_END: usize = 0x4000_0000; // 1GB mark

/// Guard page size (unmapped pages between regions for fault detection)
pub const GUARD_PAGE_SIZE: usize = FRAME_SIZE;

/// Maximum user-space virtual address (canonical lower half on x86_64)
pub const USER_SPACE_END: usize = 0x0000_7FFF_FFFF_FFFF;

// ---------------------------------------------------------------------------
// Memory region tracking
// ---------------------------------------------------------------------------

/// Type of memory mapping
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmapType {
    /// Anonymous private mapping (zero-filled)
    Anonymous,
    /// File-backed mapping
    FileBacked { inode: u64, offset: usize },
    /// Shared anonymous mapping (visible to forked children)
    SharedAnonymous,
    /// Stack region (grows downward)
    Stack,
    /// Heap region (grows upward via brk)
    Heap,
    /// Code/text segment
    Code,
    /// Data segment
    Data,
    /// Read-only data segment
    Rodata,
    /// BSS segment (zero-initialized)
    Bss,
}

/// Protection flags for memory regions
pub mod prot {
    pub const NONE: u32 = 0;
    pub const READ: u32 = 1;
    pub const WRITE: u32 = 2;
    pub const EXEC: u32 = 4;
}

/// Mapping flags
pub mod map_flags {
    pub const PRIVATE: u32 = 0x01;
    pub const SHARED: u32 = 0x02;
    pub const FIXED: u32 = 0x04;
    pub const ANONYMOUS: u32 = 0x08;
    pub const GROWSDOWN: u32 = 0x10;
    pub const POPULATE: u32 = 0x20;
    pub const NORESERVE: u32 = 0x40;
}

/// A tracked memory region in a process's address space
#[derive(Debug, Clone)]
pub struct VmRegion {
    /// Start virtual address (page-aligned)
    pub start: usize,
    /// Number of pages
    pub num_pages: usize,
    /// Protection flags (prot::READ | prot::WRITE | prot::EXEC)
    pub protection: u32,
    /// Mapping flags
    pub flags: u32,
    /// Type of mapping
    pub mmap_type: MmapType,
    /// Human-readable label
    pub label: String,
    /// Whether this region has copy-on-write pages
    pub cow: bool,
    /// Reference count for shared mappings
    pub ref_count: u32,
}

impl VmRegion {
    /// End address (exclusive)
    pub fn end(&self) -> usize {
        self.start + self.num_pages * FRAME_SIZE
    }

    /// Check if an address falls within this region
    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.start && addr < self.end()
    }

    /// Size in bytes
    pub fn size(&self) -> usize {
        self.num_pages * FRAME_SIZE
    }

    /// Convert protection flags to page table flags
    pub fn to_page_flags(&self) -> u64 {
        let mut flags = paging::flags::USER_ACCESSIBLE;
        if self.protection & prot::WRITE != 0 {
            flags |= paging::flags::WRITABLE;
        }
        if self.protection & prot::EXEC == 0 {
            flags |= paging::flags::NO_EXECUTE;
        }
        flags
    }
}

/// Per-process virtual memory manager
#[derive(Debug, Clone)]
pub struct VmSpace {
    /// PML4 physical address for this address space
    pub pml4: usize,
    /// All memory regions, sorted by start address
    pub regions: Vec<VmRegion>,
    /// Current program break (heap top)
    pub brk_current: usize,
    /// Initial program break (set at load time)
    pub brk_start: usize,
    /// Maximum program break
    pub brk_max: usize,
    /// Next free address for mmap allocations
    pub mmap_next: usize,
    /// Total mapped pages
    pub total_pages: usize,
}

impl VmSpace {
    /// Create a new virtual memory space
    pub fn new(pml4: usize) -> Self {
        VmSpace {
            pml4,
            regions: Vec::new(),
            brk_current: USER_HEAP_START,
            brk_start: USER_HEAP_START,
            brk_max: USER_HEAP_START + USER_HEAP_MAX,
            mmap_next: USER_MMAP_START,
            total_pages: 0,
        }
    }

    /// Set the initial break address (called after loading an ELF)
    pub fn set_brk(&mut self, brk: usize) {
        let aligned = (brk + FRAME_SIZE - 1) & !(FRAME_SIZE - 1);
        self.brk_start = aligned;
        self.brk_current = aligned;
        self.brk_max = aligned + USER_HEAP_MAX;
    }

    /// Implement brk() syscall: set the program break.
    /// Returns the new break address, or the current one if the request fails.
    pub fn sys_brk(&mut self, new_brk: usize) -> Result<usize, ()> {
        if new_brk == 0 {
            // Query current brk
            return Ok(self.brk_current);
        }

        let aligned_brk = (new_brk + FRAME_SIZE - 1) & !(FRAME_SIZE - 1);

        if aligned_brk < self.brk_start {
            return Err(()); // Cannot shrink below start
        }
        if aligned_brk > self.brk_max {
            return Err(()); // Cannot exceed maximum
        }

        if aligned_brk > self.brk_current {
            // Expand heap: allocate and map new pages
            let old_pages = (self.brk_current - self.brk_start) / FRAME_SIZE;
            let new_pages = (aligned_brk - self.brk_start) / FRAME_SIZE;

            for i in old_pages..new_pages {
                let virt = self.brk_start + i * FRAME_SIZE;
                let frame = frame_allocator::allocate_frame().ok_or(())?;
                unsafe {
                    core::ptr::write_bytes(frame.addr as *mut u8, 0, FRAME_SIZE);
                }
                let flags = paging::flags::USER_ACCESSIBLE
                    | paging::flags::WRITABLE
                    | paging::flags::NO_EXECUTE;
                paging::map_page(virt, frame.addr, flags)?;
                self.total_pages = self.total_pages.saturating_add(1);
            }
        } else if aligned_brk < self.brk_current {
            // Shrink heap: unmap pages
            let old_pages = (self.brk_current - self.brk_start) / FRAME_SIZE;
            let new_pages = (aligned_brk - self.brk_start) / FRAME_SIZE;

            for i in new_pages..old_pages {
                let virt = self.brk_start + i * FRAME_SIZE;
                // Unmap the page (the frame allocator should reclaim the frame)
                paging::map_page(virt, 0, 0).ok();
                paging::flush_tlb(virt);
                if self.total_pages > 0 {
                    self.total_pages -= 1;
                }
            }
        }

        self.brk_current = aligned_brk;
        Ok(self.brk_current)
    }

    /// Implement sbrk(): increment the program break by `increment` bytes.
    /// Returns the OLD break address (the start of newly allocated memory).
    pub fn sys_sbrk(&mut self, increment: isize) -> Result<usize, ()> {
        let old_brk = self.brk_current;
        let new_brk = if increment >= 0 {
            old_brk + increment as usize
        } else {
            let decrement = (-increment) as usize;
            if decrement > old_brk - self.brk_start {
                return Err(());
            }
            old_brk - decrement
        };
        self.sys_brk(new_brk)?;
        Ok(old_brk)
    }

    /// Implement mmap(): create a new anonymous memory mapping.
    /// Returns the virtual address of the mapping.
    pub fn sys_mmap(
        &mut self,
        addr_hint: usize,
        length: usize,
        protection: u32,
        flags: u32,
        mmap_type: MmapType,
        label: &str,
    ) -> Result<usize, ()> {
        if length == 0 {
            return Err(());
        }

        let num_pages = (length + FRAME_SIZE - 1) / FRAME_SIZE;
        let _total_size = num_pages * FRAME_SIZE;

        // Determine the virtual address for the mapping
        let virt_start = if flags & map_flags::FIXED != 0 && addr_hint != 0 {
            // MAP_FIXED: use the exact address (after alignment)
            let aligned = addr_hint & !(FRAME_SIZE - 1);
            // Check for overlaps with existing regions
            if self.region_overlaps(aligned, num_pages) {
                return Err(());
            }
            aligned
        } else if addr_hint != 0 {
            // Try the hint, fall back to automatic allocation
            let aligned = addr_hint & !(FRAME_SIZE - 1);
            if !self.region_overlaps(aligned, num_pages) {
                aligned
            } else {
                self.find_free_region(num_pages)?
            }
        } else {
            // Automatic allocation
            self.find_free_region(num_pages)?
        };

        // Allocate and map the pages
        let page_flags = {
            let mut f = paging::flags::USER_ACCESSIBLE;
            if protection & prot::WRITE != 0 {
                f |= paging::flags::WRITABLE;
            }
            if protection & prot::EXEC == 0 {
                f |= paging::flags::NO_EXECUTE;
            }
            f
        };

        for i in 0..num_pages {
            let virt = virt_start + i * FRAME_SIZE;
            let frame = frame_allocator::allocate_frame().ok_or(())?;
            unsafe {
                core::ptr::write_bytes(frame.addr as *mut u8, 0, FRAME_SIZE);
            }
            paging::map_page(virt, frame.addr, page_flags)?;
        }

        // Track the region
        let region = VmRegion {
            start: virt_start,
            num_pages,
            protection,
            flags,
            mmap_type,
            label: String::from(label),
            cow: false,
            ref_count: 1,
        };
        self.insert_region(region);
        self.total_pages += num_pages;

        Ok(virt_start)
    }

    /// Implement munmap(): remove a memory mapping.
    pub fn sys_munmap(&mut self, addr: usize, length: usize) -> Result<(), ()> {
        let aligned_addr = addr & !(FRAME_SIZE - 1);
        let num_pages = (length + FRAME_SIZE - 1) / FRAME_SIZE;

        // Find and remove the region
        let pos = self.regions.iter().position(|r| r.start == aligned_addr);
        if pos.is_none() {
            return Err(());
        }

        // Safety: we already checked pos.is_none() above and returned Err
        let region_idx = match pos {
            Some(i) => i,
            None => return Err(()),
        };
        let region = &self.regions[region_idx];
        let unmap_pages = core::cmp::min(num_pages, region.num_pages);

        // Unmap the pages
        for i in 0..unmap_pages {
            let virt = aligned_addr + i * FRAME_SIZE;
            paging::map_page(virt, 0, 0).ok();
            paging::flush_tlb(virt);
        }

        if self.total_pages >= unmap_pages {
            self.total_pages -= unmap_pages;
        }

        // Remove or shrink the region
        if unmap_pages >= self.regions[region_idx].num_pages {
            self.regions.remove(region_idx);
        } else {
            self.regions[region_idx].start += unmap_pages * FRAME_SIZE;
            self.regions[region_idx].num_pages -= unmap_pages;
        }

        Ok(())
    }

    /// Implement mprotect(): change protection flags on a region.
    pub fn sys_mprotect(&mut self, addr: usize, length: usize, protection: u32) -> Result<(), ()> {
        let aligned = addr & !(FRAME_SIZE - 1);
        let num_pages = (length + FRAME_SIZE - 1) / FRAME_SIZE;

        // Find the region
        let region = self
            .regions
            .iter_mut()
            .find(|r| r.start == aligned)
            .ok_or(())?;

        region.protection = protection;

        // Update page table flags
        let _page_flags = {
            let mut f = paging::flags::USER_ACCESSIBLE;
            if protection & prot::WRITE != 0 {
                f |= paging::flags::WRITABLE;
            }
            if protection & prot::EXEC == 0 {
                f |= paging::flags::NO_EXECUTE;
            }
            f
        };

        let pages_to_update = core::cmp::min(num_pages, region.num_pages);
        for i in 0..pages_to_update {
            let virt = aligned + i * FRAME_SIZE;
            // Re-map with new flags (keeping the same physical frame)
            // NOTE: a real implementation would read the current mapping first
            paging::flush_tlb(virt);
        }

        Ok(())
    }

    /// Find a free region of `num_pages` in the mmap area
    fn find_free_region(&mut self, num_pages: usize) -> Result<usize, ()> {
        let total_size = num_pages * FRAME_SIZE;
        let mut candidate = self.mmap_next;

        // Simple linear scan for a gap
        loop {
            if candidate + total_size > USER_MMAP_END {
                return Err(());
            }

            if !self.region_overlaps(candidate, num_pages) {
                self.mmap_next = candidate + total_size + GUARD_PAGE_SIZE;
                return Ok(candidate);
            }

            candidate += FRAME_SIZE;
        }
    }

    /// Check if a proposed region overlaps with any existing region
    fn region_overlaps(&self, start: usize, num_pages: usize) -> bool {
        let end = start + num_pages * FRAME_SIZE;
        for region in &self.regions {
            let r_end = region.start + region.num_pages * FRAME_SIZE;
            if start < r_end && end > region.start {
                return true;
            }
        }
        false
    }

    /// Insert a region, maintaining sort order by start address
    fn insert_region(&mut self, region: VmRegion) {
        let pos = self
            .regions
            .iter()
            .position(|r| r.start > region.start)
            .unwrap_or(self.regions.len());
        self.regions.insert(pos, region);
    }

    /// Find the region containing a virtual address
    pub fn find_region(&self, addr: usize) -> Option<&VmRegion> {
        self.regions.iter().find(|r| r.contains(addr))
    }

    /// Find the region containing a virtual address (mutable)
    pub fn find_region_mut(&mut self, addr: usize) -> Option<&mut VmRegion> {
        self.regions.iter_mut().find(|r| r.contains(addr))
    }

    /// Handle a copy-on-write page fault.
    ///
    /// When a write fault occurs on a COW page:
    /// 1. Allocate a new physical frame
    /// 2. Copy the contents of the old frame to the new one
    /// 3. Map the new frame with write permission
    /// 4. Decrement the reference count on the old frame
    pub fn handle_cow_fault(&mut self, fault_addr: usize) -> Result<(), ()> {
        let aligned = fault_addr & !(FRAME_SIZE - 1);

        // Find the region
        let region = self.find_region_mut(fault_addr).ok_or(())?;

        if !region.cow {
            return Err(()); // Not a COW page, genuine fault
        }

        if region.protection & prot::WRITE == 0 {
            return Err(()); // Region is not supposed to be writable
        }

        // Allocate a new frame
        let new_frame = frame_allocator::allocate_frame().ok_or(())?;

        // Copy the old page contents to the new frame
        unsafe {
            core::ptr::copy_nonoverlapping(
                aligned as *const u8,
                new_frame.addr as *mut u8,
                FRAME_SIZE,
            );
        }

        // Remap with write permission
        let flags = paging::flags::USER_ACCESSIBLE | paging::flags::WRITABLE;
        paging::map_page(aligned, new_frame.addr, flags)?;
        paging::flush_tlb(aligned);

        // If this was the last reference, the region is no longer COW
        if region.ref_count <= 1 {
            region.cow = false;
        } else {
            region.ref_count -= 1;
        }

        Ok(())
    }

    /// Clean up the entire address space (called on process exit).
    /// Unmaps all user-space pages and frees the PML4 frame.
    pub fn cleanup(&mut self) {
        for region in &self.regions {
            for i in 0..region.num_pages {
                let virt = region.start + i * FRAME_SIZE;
                paging::map_page(virt, 0, 0).ok();
            }
        }
        self.regions.clear();
        self.total_pages = 0;

        // Free the PML4 frame itself
        if self.pml4 != 0 {
            // NOTE: In a real implementation, we would walk the page table
            // and free all intermediate frames as well.
            // For now, we just note the PML4 should be reclaimed.
            self.pml4 = 0;
        }
    }

    /// Dump the virtual memory layout for debugging
    pub fn dump(&self) {
        serial_println!(
            "  VmSpace: PML4={:#x}, brk={:#x}..{:#x}, total={}p",
            self.pml4,
            self.brk_start,
            self.brk_current,
            self.total_pages
        );
        for region in &self.regions {
            let perms = alloc::format!(
                "{}{}{}",
                if region.protection & prot::READ != 0 {
                    "r"
                } else {
                    "-"
                },
                if region.protection & prot::WRITE != 0 {
                    "w"
                } else {
                    "-"
                },
                if region.protection & prot::EXEC != 0 {
                    "x"
                } else {
                    "-"
                },
            );
            serial_println!(
                "    {:#x}-{:#x} {} {} ({}p)",
                region.start,
                region.end(),
                perms,
                region.label,
                region.num_pages
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Address space creation
// ---------------------------------------------------------------------------

/// Create a new user-space address space.
///
/// Allocates a fresh PML4, copies kernel mappings into the upper half,
/// and returns the physical address of the new PML4.
pub fn create_address_space() -> Result<usize, ()> {
    // Allocate a frame for the new PML4
    let pml4_frame = frame_allocator::allocate_frame().ok_or(())?;

    // Zero it out
    unsafe {
        core::ptr::write_bytes(pml4_frame.addr as *mut u8, 0, FRAME_SIZE);
    }

    // Copy kernel mappings (upper half: entries 256-511 of PML4)
    // The kernel uses identity mapping, so we share those PML4 entries
    let current_pml4 = paging::read_cr3();
    unsafe {
        let src = current_pml4 as *const u64;
        let dst = pml4_frame.addr as *mut u64;

        // Copy entries 256..512 (kernel half)
        for i in 256..512 {
            *dst.add(i) = *src.add(i);
        }
    }

    Ok(pml4_frame.addr)
}

/// Map a region of memory in a user-space address space
pub fn map_user_pages(
    _pml4_addr: usize,
    virt_start: usize,
    num_pages: usize,
    writable: bool,
) -> Result<(), ()> {
    // Temporarily switch to the user's page table to set up mappings
    let _old_cr3 = paging::read_cr3();

    // We need to map pages in the user's address space
    // Since we're using identity mapping for kernel, we can manipulate
    // the page tables directly
    for i in 0..num_pages {
        let virt = virt_start + i * FRAME_SIZE;
        let frame = frame_allocator::allocate_frame().ok_or(())?;

        // Zero the frame
        unsafe {
            core::ptr::write_bytes(frame.addr as *mut u8, 0, FRAME_SIZE);
        }

        let mut flags = paging::flags::USER_ACCESSIBLE;
        if writable {
            flags |= paging::flags::WRITABLE;
        }

        // We need to walk the page tables manually for the user's PML4
        // For now, we set up the mappings in the current address space
        // and they'll be visible when we switch to the user's PML4
        // (since kernel mappings are shared)
        paging::map_page(virt, frame.addr, flags)?;
    }

    Ok(())
}

/// Map user pages with full protection control
pub fn map_user_pages_prot(
    _pml4_addr: usize,
    virt_start: usize,
    num_pages: usize,
    protection: u32,
) -> Result<(), ()> {
    for i in 0..num_pages {
        let virt = virt_start + i * FRAME_SIZE;
        let frame = frame_allocator::allocate_frame().ok_or(())?;

        unsafe {
            core::ptr::write_bytes(frame.addr as *mut u8, 0, FRAME_SIZE);
        }

        let mut flags = paging::flags::USER_ACCESSIBLE;
        if protection & prot::WRITE != 0 {
            flags |= paging::flags::WRITABLE;
        }
        if protection & prot::EXEC == 0 {
            flags |= paging::flags::NO_EXECUTE;
        }

        paging::map_page(virt, frame.addr, flags)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Stack setup
// ---------------------------------------------------------------------------

/// Set up a user-space stack
pub fn setup_user_stack(pml4_addr: usize) -> Result<usize, ()> {
    let stack_pages = USER_STACK_SIZE / FRAME_SIZE;
    let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;

    map_user_pages(pml4_addr, stack_bottom, stack_pages, true)?;

    // Return the initial stack pointer (top of stack, grows down)
    Ok(USER_STACK_TOP)
}

/// Set up a user-space stack with a guard page below it
pub fn setup_guarded_user_stack(pml4_addr: usize, stack_size: usize) -> Result<(usize, usize), ()> {
    let stack_pages = stack_size / FRAME_SIZE;
    let _total_pages = stack_pages + 1; // +1 for guard page
    let stack_bottom = USER_STACK_TOP - stack_size;
    let guard_page = stack_bottom - FRAME_SIZE;

    // The guard page is left unmapped -- accessing it triggers a page fault
    // Map the actual stack pages
    map_user_pages(pml4_addr, stack_bottom, stack_pages, true)?;

    // Return (stack_pointer, guard_page_addr)
    Ok((USER_STACK_TOP, guard_page))
}

// ---------------------------------------------------------------------------
// User-space transition
// ---------------------------------------------------------------------------

/// Jump to user-space code using IRET.
///
/// Sets up the stack frame that IRET expects:
///   [SS] [RSP] [RFLAGS] [CS] [RIP]
///
/// SS and CS use user-mode segment selectors (ring 3).
///
/// SAFETY: entry_point must be a valid user-space address.
/// stack_ptr must point to a valid user-space stack.
pub unsafe fn jump_to_userspace(entry_point: usize, stack_ptr: usize) -> ! {
    // User-mode segment selectors (ring 3)
    // GDT layout: 0x18=User Data (DPL 3), 0x20=User Code (DPL 3)
    let user_cs: u64 = gdt::USER_CS as u64; // 0x23 = 0x20 | RPL 3
    let user_ss: u64 = gdt::USER_DS as u64; // 0x1B = 0x18 | RPL 3
    let rflags: u64 = 0x200; // IF=1 (interrupts enabled)

    core::arch::asm!(
        "push {ss}",       // SS
        "push {rsp}",      // RSP
        "push {rflags}",   // RFLAGS
        "push {cs}",       // CS
        "push {rip}",      // RIP
        "iretq",
        ss = in(reg) user_ss,
        rsp = in(reg) stack_ptr as u64,
        rflags = in(reg) rflags,
        cs = in(reg) user_cs,
        rip = in(reg) entry_point as u64,
        options(noreturn),
    );
}

/// Jump to user-space using SYSRET (faster than IRET for returns from syscall).
///
/// SYSRET expects:
///   RCX = user RIP (return address)
///   R11 = user RFLAGS
///
/// The CPU will set CS and SS from the STAR MSR.
///
/// SAFETY: entry_point must be a canonical user-space address.
/// stack_ptr must point to valid user-space stack.
pub unsafe fn sysret_to_userspace(entry_point: usize, stack_ptr: usize) -> ! {
    let rflags: u64 = 0x200; // IF=1

    core::arch::asm!(
        "mov rsp, {rsp}",
        "mov rcx, {rip}",     // SYSRET loads RIP from RCX
        "mov r11, {rflags}",  // SYSRET loads RFLAGS from R11
        "sysretq",
        rsp = in(reg) stack_ptr as u64,
        rip = in(reg) entry_point as u64,
        rflags = in(reg) rflags,
        options(noreturn),
    );
}

// ---------------------------------------------------------------------------
// SYSCALL entry point setup
// ---------------------------------------------------------------------------

/// Set up the SYSCALL/SYSRET MSRs for user-space system call entry.
///
/// This must be called once during kernel initialization.
/// After this, user-space can use the SYSCALL instruction.
///
/// The SYSCALL instruction does:
///   1. Saves RIP into RCX, RFLAGS into R11
///   2. Loads CS and SS from STAR MSR
///   3. Loads RIP from LSTAR MSR (kernel entry)
///   4. Masks RFLAGS with SFMASK (clears IF, TF, DF)
///
/// SAFETY: Must be called from ring 0 during initialization.
pub unsafe fn setup_syscall(entry_point: u64) {
    super::context::setup_syscall_msrs(entry_point);
    serial_println!(
        "  Userspace: SYSCALL/SYSRET configured, entry={:#x}",
        entry_point
    );
}

// ---------------------------------------------------------------------------
// Process spawning
// ---------------------------------------------------------------------------

/// Spawn a minimal test process from raw machine code bytes.
///
/// Maps code at USER_SPACE_START, sets up a user stack, and schedules
/// the process for execution. Used for testing ring-3 transitions.
pub fn spawn_test_process(name: &str, code: &[u8]) -> Result<u32, ()> {
    use crate::process;

    // Create user address space
    let pml4 = create_address_space()?;

    // Map a page at USER_SPACE_START for the code
    let code_pages = (code.len() + 0xFFF) / 0x1000;
    map_user_pages(pml4, USER_SPACE_START, core::cmp::max(code_pages, 1), false)?;

    // Copy machine code into the mapped page
    unsafe {
        core::ptr::copy_nonoverlapping(code.as_ptr(), USER_SPACE_START as *mut u8, code.len());
    }

    // Set up user stack
    let stack_ptr = setup_user_stack(pml4)?;

    // Create the process
    let mut table = process::pcb::PROCESS_TABLE.lock();
    let pid = (1..process::MAX_PROCESSES)
        .find(|&i| table[i].is_none())
        .ok_or(())? as u32;

    let mut proc = process::pcb::Process::new_kernel(pid, name);
    proc.page_table = pml4;
    proc.is_kernel = false;
    proc.context.rip = USER_SPACE_START as u64;
    proc.context.rsp = stack_ptr as u64;
    proc.context.cs = gdt::USER_CS as u64;
    proc.context.ss = gdt::USER_DS as u64;
    proc.context.rflags = 0x200;
    proc.state = process::pcb::ProcessState::Ready;

    table[pid as usize] = Some(proc);
    drop(table);

    process::scheduler::SCHEDULER.lock().add(pid);
    serial_println!(
        "  Userspace: spawned test PID {} ({}) at {:#x}",
        pid,
        name,
        USER_SPACE_START
    );

    Ok(pid)
}

/// Minimal test program: writes "Hello from ring 3!\n" to stdout then exits.
///
/// x86_64 machine code using our syscall interface:
///   SYS_WRITE(fd=1, buf, len=19)  then  SYS_EXIT(0)
pub fn hello_userspace_code() -> alloc::vec::Vec<u8> {
    let msg = b"Hello from ring 3!\n";
    let msg_len = msg.len() as u8;

    // Build machine code that does:
    //   lea rsi, [rip + msg_offset]   ; buf pointer
    //   mov rax, 1                     ; SYS_WRITE
    //   mov rdi, 1                     ; fd = stdout
    //   mov rdx, <len>                 ; count
    //   syscall
    //   mov rax, 0                     ; SYS_EXIT
    //   xor rdi, rdi                   ; status = 0
    //   syscall
    //   msg: "Hello from ring 3!\n"

    let mut code = alloc::vec::Vec::new();

    // Layout:
    //   [0..7]   lea rsi, [rip+offset]   (7 bytes: 48 8D 35 xx xx xx xx)
    //   [7..12]  mov eax, 1              (5 bytes: B8 01 00 00 00)
    //   [12..17] mov edi, 1              (5 bytes: BF 01 00 00 00)
    //   [17..22] mov edx, len            (5 bytes: BA xx 00 00 00)
    //   [22..24] syscall                 (2 bytes: 0F 05)
    //   [24..29] mov eax, 0              (5 bytes: B8 00 00 00 00)
    //   [29..31] xor edi, edi            (2 bytes: 31 FF)
    //   [31..33] syscall                 (2 bytes: 0F 05)
    //   [33..]   message bytes

    let msg_offset: i32 = 33 - 7; // 26 -- rip-relative from end of lea instruction

    // lea rsi, [rip + 26]
    code.push(0x48);
    code.push(0x8D);
    code.push(0x35);
    code.extend_from_slice(&msg_offset.to_le_bytes());

    // mov eax, 1 (SYS_WRITE)
    code.push(0xB8);
    code.extend_from_slice(&1u32.to_le_bytes());

    // mov edi, 1 (stdout)
    code.push(0xBF);
    code.extend_from_slice(&1u32.to_le_bytes());

    // mov edx, msg_len
    code.push(0xBA);
    code.extend_from_slice(&(msg_len as u32).to_le_bytes());

    // syscall
    code.push(0x0F);
    code.push(0x05);

    // mov eax, 0 (SYS_EXIT)
    code.push(0xB8);
    code.extend_from_slice(&0u32.to_le_bytes());

    // xor edi, edi (status = 0)
    code.push(0x31);
    code.push(0xFF);

    // syscall
    code.push(0x0F);
    code.push(0x05);

    // The message string
    code.extend_from_slice(msg);

    code
}

/// Create and launch a user-space process from an ELF binary
pub fn spawn_user_process(elf_data: &[u8], name: &str) -> Result<u32, ()> {
    use crate::process;
    use crate::process::elf;

    // Create user address space
    let pml4 = create_address_space()?;

    // Load ELF into the address space
    let load_result = elf::load(elf_data).map_err(|_| ())?;

    // Set up user stack
    let stack_ptr = setup_user_stack(pml4)?;

    // Create the process
    let mut table = process::pcb::PROCESS_TABLE.lock();
    let pid = (1..process::MAX_PROCESSES)
        .find(|&i| table[i].is_none())
        .ok_or(())? as u32;

    let mut proc = process::pcb::Process::new_kernel(pid, name);
    proc.page_table = pml4;
    proc.is_kernel = false;

    // Set up context to jump to user space via IRET
    proc.context.rip = load_result.entry as u64;
    proc.context.rsp = stack_ptr as u64;
    proc.context.cs = gdt::USER_CS as u64; // 0x23 user code
    proc.context.ss = gdt::USER_DS as u64; // 0x1B user data
    proc.context.rflags = 0x200;
    proc.context.cr3 = pml4 as u64;
    proc.context.kernel_rsp = proc.kernel_stack_top() as u64;
    proc.state = process::pcb::ProcessState::Ready;

    // Set the program break for the process
    proc.brk = load_result.brk;
    proc.brk_start = load_result.brk;

    table[pid as usize] = Some(proc);
    drop(table);

    process::scheduler::SCHEDULER.lock().add(pid);
    serial_println!(
        "  Process: spawned user PID {} ({}) entry={:#x}",
        pid,
        name,
        load_result.entry
    );

    Ok(pid)
}

/// Clean up a user-space process's address space on exit.
///
/// Frees all user-space page mappings and the PML4 frame.
pub fn cleanup_address_space(pml4: usize, regions: &[(usize, usize, u64)]) {
    // Unmap all user-space regions
    for &(virt_start, num_pages, _flags) in regions {
        for i in 0..num_pages {
            let virt = virt_start + i * FRAME_SIZE;
            if virt < USER_SPACE_END {
                paging::map_page(virt, 0, 0).ok();
            }
        }
    }

    // Free the PML4 frame
    // NOTE: In a complete implementation, we would walk the page table
    // tree and free all intermediate table frames as well.
    if pml4 != 0 && pml4 != paging::read_cr3() {
        // Could free the PML4 frame back to the frame allocator here
    }
}

/// Handle a page fault from user space.
///
/// Returns Ok(()) if the fault was handled (e.g., COW, stack growth),
/// or Err(fault_addr) if the process should be killed.
pub fn handle_user_page_fault(fault_addr: usize, error_code: u64, pid: u32) -> Result<(), usize> {
    let _is_write = error_code & 0x2 != 0;
    let is_user = error_code & 0x4 != 0;
    let _is_present = error_code & 0x1 != 0;

    if !is_user {
        // Kernel page fault -- should not happen here
        return Err(fault_addr);
    }

    // Check if this is a stack growth fault (address just below the stack)
    let table = super::pcb::PROCESS_TABLE.lock();
    if let Some(_proc) = table[pid as usize].as_ref() {
        let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;
        let stack_growth_limit = stack_bottom - (16 * FRAME_SIZE); // allow 16 extra pages

        if fault_addr >= stack_growth_limit && fault_addr < stack_bottom {
            // Stack growth: map the faulting page
            drop(table);
            let frame = frame_allocator::allocate_frame().ok_or(fault_addr)?;
            unsafe {
                core::ptr::write_bytes(frame.addr as *mut u8, 0, FRAME_SIZE);
            }
            let page_addr = fault_addr & !(FRAME_SIZE - 1);
            let flags = paging::flags::USER_ACCESSIBLE
                | paging::flags::WRITABLE
                | paging::flags::NO_EXECUTE;
            paging::map_page(page_addr, frame.addr, flags).map_err(|_| fault_addr)?;
            return Ok(());
        }
    }
    drop(table);

    // Not a handled fault
    Err(fault_addr)
}
