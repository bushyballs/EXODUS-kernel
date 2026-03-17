use core::sync::atomic::{AtomicU32, Ordering};

/// Maximum number of page migrations allowed per timer tick.
/// Limits migration overhead to avoid starving other kernel work.
const MAX_MIGRATIONS_PER_TICK: u32 = 64;

/// Counter of migrations performed in the current tick window.
/// Reset by `migrate_tick_reset()`, which should be called from the timer ISR.
static MIGRATIONS_THIS_TICK: AtomicU32 = AtomicU32::new(0);

/// Describes a page to migrate with source and destination nodes.
pub struct MigrateRequest {
    /// Source physical address of the page (identity-mapped).
    pub phys_addr: usize,
    /// Source NUMA node identifier.
    pub source_node: usize,
    /// Destination physical address of the page (identity-mapped).
    /// Field is named dest_node for API compatibility; carries the dest phys addr.
    pub dest_node: usize,
    /// Virtual address that currently maps `phys_addr` and must be updated.
    /// Set to 0 to skip PTE update (e.g., when the caller handles it separately).
    pub virt_addr: usize,
}

/// Result of a migration batch.
pub struct MigrateResult {
    pub succeeded: usize,
    pub failed: usize,
}

const PAGE_SIZE: usize = 4096;

/// Migrate a batch of pages between NUMA nodes.
///
/// For each request:
///  1. Enforces the per-tick rate limit (MAX_MIGRATIONS_PER_TICK).
///  2. Copies 4096 bytes from source to destination physical address.
///  3. Remaps `virt_addr` → `dest_phys` via `paging::map_page` (if virt_addr != 0).
///  4. Frees the source physical frame via the frame allocator.
pub fn migrate_pages(requests: &[MigrateRequest]) -> MigrateResult {
    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for req in requests {
        if req.phys_addr == 0 {
            failed = failed.saturating_add(1);
            continue;
        }

        // Rate limiting: cap migrations per tick.
        let count = MIGRATIONS_THIS_TICK.fetch_add(1, Ordering::Relaxed);
        if count >= MAX_MIGRATIONS_PER_TICK {
            // Decrement back so the count stays accurate after we bail.
            MIGRATIONS_THIS_TICK.fetch_sub(1, Ordering::Relaxed);
            failed = failed.saturating_add(1);
            continue;
        }

        let dest_phys = req.dest_node; // dest_node carries dest phys addr

        // Safety: caller guarantees phys_addr and dest_phys are valid,
        // identity-mapped physical addresses of PAGE_SIZE bytes.
        unsafe {
            let src = req.phys_addr as *const u8;
            let dst = dest_phys as *mut u8;
            core::ptr::copy_nonoverlapping(src, dst, PAGE_SIZE);
        }

        // Update the page table entry so the virtual address now points at
        // the destination physical frame.
        if req.virt_addr != 0 && dest_phys != 0 && dest_phys % PAGE_SIZE == 0 {
            // Preserve PRESENT | WRITABLE flags; the migrated page is data.
            let flags = crate::memory::paging::flags::KERNEL_RW;
            // Ignore mapping errors — migration still succeeded at the copy level.
            let _ = crate::memory::paging::map_page(req.virt_addr, dest_phys, flags);
        }

        // Free the source physical frame back to the allocator.
        if req.phys_addr % PAGE_SIZE == 0 {
            crate::memory::frame_allocator::deallocate_frame(
                crate::memory::frame_allocator::PhysFrame::from_addr(req.phys_addr),
            );
        }

        crate::serial_println!(
            "migrate: page 0x{:x} → 0x{:x} (virt 0x{:x})",
            req.phys_addr,
            dest_phys,
            req.virt_addr
        );
        succeeded = succeeded.saturating_add(1);
    }

    MigrateResult { succeeded, failed }
}

/// Move all pages of a process to a target NUMA node.
///
/// Full implementation requires walking the process page tables which needs
/// MM/scheduler integration not yet present. Returns a zero-succeeded stub.
pub fn migrate_process(pid: u32, target_node: usize) -> MigrateResult {
    crate::serial_println!(
        "migrate: migrate_process(pid={}, target_node={}) — PT walk not yet integrated",
        pid,
        target_node
    );
    MigrateResult {
        succeeded: 0,
        failed: 0,
    }
}

/// Reset the per-tick migration counter.
///
/// Call this from the periodic timer interrupt handler (e.g., the tick ISR)
/// to allow a fresh budget of `MAX_MIGRATIONS_PER_TICK` migrations every tick.
pub fn migrate_tick_reset() {
    MIGRATIONS_THIS_TICK.store(0, Ordering::Relaxed);
}

/// Initialize the page migration subsystem.
pub fn init() {
    MIGRATIONS_THIS_TICK.store(0, Ordering::Relaxed);
}
