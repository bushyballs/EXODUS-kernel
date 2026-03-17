/// Live VM migration between physical hosts.
///
/// Part of the AIOS hypervisor subsystem.
///
/// Implements iterative pre-copy live migration:
/// 1. Pre-copy phase: iteratively transfer dirty memory pages while the
///    guest continues running.
/// 2. Stop-and-copy phase: pause the guest, transfer remaining dirty
///    pages and CPU/device state, resume on the destination.
///
/// Dirty page tracking uses the EPT dirty/accessed bits (PML — Page
/// Modification Logging on Intel, or dirty bit tracking on AMD).

use crate::{serial_print, serial_println};
use crate::sync::Mutex;
use alloc::vec::Vec;

/// Maximum number of pre-copy iterations before forced stop-and-copy.
const MAX_PRECOPY_ITERATIONS: u32 = 30;

/// Dirty page threshold (in pages) to switch from pre-copy to stop-and-copy.
/// When fewer than this many pages are dirty, it is efficient to stop the guest.
const DIRTY_PAGE_THRESHOLD: usize = 256;

/// Page size for dirty tracking (4 KiB).
const PAGE_SIZE: u64 = 4096;

/// Global migration state.
static MIGRATION_STATE: Mutex<Option<MigrationManager>> = Mutex::new(None);

/// Manages active migration sessions.
struct MigrationManager {
    /// Currently active migration session (at most one at a time).
    active_session: Option<u64>, // guest_id of the migrating VM.
}

/// Migration state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationState {
    /// No migration in progress.
    Idle,
    /// Iterative pre-copy of dirty pages.
    PreCopy,
    /// Guest paused, transferring final state.
    StopAndCopy,
    /// Migration completed successfully.
    Complete,
    /// Migration failed.
    Failed,
}

/// Manages live migration of a guest VM.
pub struct MigrationSession {
    /// ID of the guest being migrated.
    guest_id: u64,
    /// Current migration state.
    state: MigrationState,
    /// Total guest memory size in bytes.
    memory_size: u64,
    /// Bitmap of dirty pages (1 bit per page).
    dirty_bitmap: Vec<u8>,
    /// Number of pages in the guest.
    total_pages: usize,
    /// Number of pre-copy iterations completed.
    precopy_iterations: u32,
    /// Number of pages transferred in total.
    pages_transferred: u64,
    /// Number of pages remaining in current dirty set.
    dirty_page_count: usize,
    /// TSC timestamp when migration started.
    start_tsc: u64,
    /// TSC timestamp when migration ended.
    end_tsc: u64,
}

impl MigrationSession {
    pub fn new(guest_id: u64) -> Self {
        // Default to 256 MiB guest for dirty bitmap sizing.
        // In practice, this would be read from the GuestVm's memory_size.
        let memory_size: u64 = 256 * 1024 * 1024;
        let total_pages = (memory_size / PAGE_SIZE) as usize;
        let bitmap_bytes = (total_pages + 7) / 8;

        // Initialize dirty bitmap with all pages marked dirty (initial full copy).
        let dirty_bitmap = {
            let mut bm = Vec::with_capacity(bitmap_bytes);
            for _ in 0..bitmap_bytes {
                bm.push(0xFF); // All pages dirty initially.
            }
            bm
        };

        let start_tsc = rdtsc();

        serial_println!(
            "    [migration] Created migration session for guest {} ({} pages, {} MiB)",
            guest_id, total_pages, memory_size / (1024 * 1024)
        );

        MigrationSession {
            guest_id,
            state: MigrationState::Idle,
            memory_size,
            dirty_bitmap,
            total_pages,
            precopy_iterations: 0,
            pages_transferred: 0,
            dirty_page_count: total_pages,
            start_tsc,
            end_tsc: 0,
        }
    }

    /// Begin iterative pre-copy of guest memory.
    ///
    /// Each iteration:
    /// 1. Scan the dirty bitmap for pages that have been modified.
    /// 2. Transfer those pages to the destination (simulated here).
    /// 3. Clear the dirty bitmap.
    /// 4. Let the guest continue running (more pages may become dirty).
    /// 5. Repeat until dirty count is below the threshold.
    pub fn start_precopy(&mut self) {
        if self.state != MigrationState::Idle {
            serial_println!(
                "    [migration] Cannot start precopy: session in state {:?}",
                self.state
            );
            return;
        }

        self.state = MigrationState::PreCopy;

        // Register this session as active.
        {
            let mut mgr = MIGRATION_STATE.lock();
            if let Some(ref mut m) = *mgr {
                if m.active_session.is_some() {
                    serial_println!("    [migration] Another migration is already in progress");
                    self.state = MigrationState::Failed;
                    return;
                }
                m.active_session = Some(self.guest_id);
            }
        }

        serial_println!(
            "    [migration] Starting pre-copy for guest {} ({} dirty pages)",
            self.guest_id, self.dirty_page_count
        );

        // Iterative pre-copy loop.
        while self.state == MigrationState::PreCopy {
            self.precopy_iterations = self.precopy_iterations.saturating_add(1);

            // Count and transfer dirty pages.
            let dirty_count = self.count_dirty_pages();
            self.dirty_page_count = dirty_count;

            serial_println!(
                "    [migration] Pre-copy iteration {}: {} dirty pages",
                self.precopy_iterations, dirty_count
            );

            // Simulate transferring dirty pages.
            self.transfer_dirty_pages();

            // Clear the dirty bitmap (the EPT/PML would re-dirty pages as the guest writes).
            self.clear_dirty_bitmap();

            // Simulate the guest dirtying some pages during the transfer.
            // In a real system, the EPT dirty bits would be collected via PML.
            let re_dirtied = self.simulate_guest_dirties();
            self.dirty_page_count = re_dirtied;

            // Check convergence criteria.
            if re_dirtied <= DIRTY_PAGE_THRESHOLD {
                serial_println!(
                    "    [migration] Converged after {} iterations ({} dirty pages remaining)",
                    self.precopy_iterations, re_dirtied
                );
                break;
            }

            if self.precopy_iterations >= MAX_PRECOPY_ITERATIONS {
                serial_println!(
                    "    [migration] Max pre-copy iterations reached, forcing stop-and-copy"
                );
                break;
            }
        }
    }

    /// Finalize migration with stop-and-copy.
    ///
    /// 1. Pause the guest VM.
    /// 2. Transfer all remaining dirty pages.
    /// 3. Transfer CPU state, device state, VMCS/VMCB contents.
    /// 4. Signal the destination to start the guest.
    pub fn finalize(&mut self) {
        if self.state != MigrationState::PreCopy {
            serial_println!(
                "    [migration] Cannot finalize: session in state {:?}",
                self.state
            );
            return;
        }

        self.state = MigrationState::StopAndCopy;
        serial_println!("    [migration] Entering stop-and-copy for guest {}", self.guest_id);

        // In a real implementation: pause the guest via GuestVm::pause().

        // Transfer remaining dirty pages.
        let final_dirty = self.count_dirty_pages();
        serial_println!(
            "    [migration] Transferring {} final dirty pages",
            final_dirty
        );
        self.transfer_dirty_pages();

        // Transfer CPU/device state.
        self.transfer_cpu_state();
        self.transfer_device_state();

        self.end_tsc = rdtsc();
        self.state = MigrationState::Complete;

        // Clear active session.
        {
            let mut mgr = MIGRATION_STATE.lock();
            if let Some(ref mut m) = *mgr {
                m.active_session = None;
            }
        }

        let elapsed_cycles = self.end_tsc.saturating_sub(self.start_tsc);
        serial_println!(
            "    [migration] Migration complete for guest {} ({} pages transferred, {} cycles elapsed)",
            self.guest_id, self.pages_transferred, elapsed_cycles
        );
    }

    /// Get the current migration state.
    pub fn state(&self) -> MigrationState {
        self.state
    }

    /// Cancel an in-progress migration.
    pub fn cancel(&mut self) {
        serial_println!("    [migration] Cancelling migration for guest {}", self.guest_id);
        self.state = MigrationState::Failed;
        self.end_tsc = rdtsc();

        let mut mgr = MIGRATION_STATE.lock();
        if let Some(ref mut m) = *mgr {
            m.active_session = None;
        }
    }

    // --- Internal helpers ---

    /// Count the number of set bits in the dirty bitmap.
    fn count_dirty_pages(&self) -> usize {
        let mut count = 0usize;
        for &byte in &self.dirty_bitmap {
            count += byte.count_ones() as usize;
        }
        count
    }

    /// Simulate transferring dirty pages to the destination.
    fn transfer_dirty_pages(&mut self) {
        for (byte_idx, byte) in self.dirty_bitmap.iter().enumerate() {
            if *byte == 0 {
                continue;
            }
            for bit in 0..8 {
                if byte & (1 << bit) != 0 {
                    let page_index = byte_idx * 8 + bit;
                    if page_index < self.total_pages {
                        // In a real implementation, read the page from guest memory
                        // and send it to the destination host over the network.
                        self.pages_transferred = self.pages_transferred.saturating_add(1);
                    }
                }
            }
        }
    }

    /// Clear the dirty bitmap (all pages marked clean).
    fn clear_dirty_bitmap(&mut self) {
        for byte in self.dirty_bitmap.iter_mut() {
            *byte = 0;
        }
    }

    /// Simulate the guest dirtying pages during a pre-copy iteration.
    /// Returns the number of newly dirtied pages.
    fn simulate_guest_dirties(&mut self) -> usize {
        // Heuristic: the guest re-dirties a decreasing fraction of pages
        // with each iteration (working set convergence).
        let fraction = if self.precopy_iterations < 5 {
            4 // 1/4 of pages
        } else if self.precopy_iterations < 10 {
            8 // 1/8
        } else if self.precopy_iterations < 20 {
            16 // 1/16
        } else {
            32 // 1/32
        };

        let pages_to_dirty = self.total_pages / fraction;
        let mut dirtied = 0usize;

        // Use a simple FNV-1a-seeded pattern to select pages.
        let mut hash: u64 = 0xcbf29ce484222325;
        let prime: u64 = 0x100000001b3;

        for _ in 0..pages_to_dirty {
            hash ^= self.precopy_iterations as u64;
            hash = hash.wrapping_mul(prime);
            let page_index = (hash as usize) % self.total_pages;
            let byte_idx = page_index / 8;
            let bit_idx = page_index % 8;
            if byte_idx < self.dirty_bitmap.len() {
                self.dirty_bitmap[byte_idx] |= 1 << bit_idx;
                dirtied += 1;
            }
        }

        dirtied
    }

    /// Transfer CPU register state (VMCS fields).
    fn transfer_cpu_state(&self) {
        // In a real implementation, read all VMCS/VMCB fields and serialize them.
        serial_println!("    [migration] CPU state serialized for guest {}", self.guest_id);
    }

    /// Transfer virtual device state.
    fn transfer_device_state(&self) {
        // Serialize virtio device queues, PIC/PIT/UART state, etc.
        serial_println!("    [migration] Device state serialized for guest {}", self.guest_id);
    }
}

/// Read the Time Stamp Counter.
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    let mgr = MigrationManager {
        active_session: None,
    };
    *MIGRATION_STATE.lock() = Some(mgr);
    serial_println!("    [migration] Live migration subsystem initialized");
}
