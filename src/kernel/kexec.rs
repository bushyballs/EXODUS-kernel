/// kexec / kdump kernel handoff — Genesis AIOS
///
/// `kexec` loads a new kernel image into memory and jumps to it without
/// performing a full hardware reboot.  `kdump` (crash kernel) is the same
/// mechanism used specifically for kernel crash dumps: a secondary kernel is
/// pre-loaded at boot and triggered when the primary kernel panics.
///
/// This implementation tracks the kexec image in a fixed-size static
/// structure protected by a `Mutex`.  The actual machine-level hand-off
/// (disable interrupts, flush caches, far-jump) is represented as a stub
/// because the full bare-metal sequence depends on architecture-specific boot
/// protocol details outside the scope of this metadata module.
///
/// Design constraints (bare-metal #![no_std]):
///   - NO heap: no Vec / Box / String / alloc::* — fixed-size static arrays only
///   - NO floats: no `as f64` / `as f32`, no float literals
///   - NO panics: no unwrap() / expect() / panic!() — return Option<T> / bool
///   - Counters: saturating_add / saturating_sub only
///   - Sequence numbers: wrapping_add only
///   - MMIO reads/writes: read_volatile / write_volatile only
///   - Structs in static Mutex<T>: Copy + `const fn empty()`
///   - No division without guarding divisor != 0
///
/// Inspired by: Linux kexec/kdump (kernel/kexec*.c). All code is original.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of kexec segments (memory regions) in one image.
pub const KEXEC_MAX_SEGMENTS: usize = 16;

/// Size of the metadata buffer (informational; not backed by a heap allocation).
pub const KEXEC_BUFFER_SIZE: usize = 8192;

/// Maximum length of the kernel command line passed to the new kernel.
pub const KEXEC_CMDLINE_MAX: usize = 512;

// ---------------------------------------------------------------------------
// KexecSegment
// ---------------------------------------------------------------------------

/// One contiguous region of the new kernel image.
///
/// The loader copies `src_len` bytes from `src` (a virtual address in the
/// running kernel) to `dst` (a physical address for the new kernel).
#[derive(Copy, Clone)]
pub struct KexecSegment {
    /// Source virtual address in the current kernel's address space.
    pub src: u64,
    /// Number of bytes at `src` to copy.
    pub src_len: usize,
    /// Destination physical address in the new kernel's address space.
    pub dst: u64,
    /// Size of the destination region (may be larger than `src_len`; padding
    /// is zeroed).
    pub dst_len: usize,
    /// `true` when this slot is occupied; `false` for an empty/unused entry.
    pub active: bool,
}

impl KexecSegment {
    /// Return an empty (unused) segment slot for `static` initialisation.
    pub const fn empty() -> Self {
        KexecSegment {
            src: 0,
            src_len: 0,
            dst: 0,
            dst_len: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// KexecState
// ---------------------------------------------------------------------------

/// Life-cycle state of the kexec image.
#[derive(Copy, Clone, PartialEq)]
pub enum KexecState {
    /// No image loaded.
    Empty,
    /// Segments are being added but `kexec_load()` has not been called.
    Loading,
    /// Image is fully staged and ready to execute.
    Loaded,
    /// `kexec_execute()` has been called (kernel hand-off in progress).
    Executing,
}

// ---------------------------------------------------------------------------
// KexecImage
// ---------------------------------------------------------------------------

/// Complete kexec image descriptor.
pub struct KexecImage {
    /// Segment table for the new kernel.
    pub segments: [KexecSegment; KEXEC_MAX_SEGMENTS],
    /// Number of valid entries in `segments`.
    pub nsegments: u8,
    /// Entry point (physical address) for the new kernel.
    pub entry_addr: u64,
    /// Current state of this image.
    pub state: KexecState,
    /// Kernel command line to pass to the new kernel (ASCII bytes).
    pub cmdline: [u8; KEXEC_CMDLINE_MAX],
    /// Number of valid bytes in `cmdline`.
    pub cmdline_len: u16,
    /// `true` when this image was loaded as a crash (kdump) kernel.
    pub is_crash: bool,
}

impl KexecImage {
    /// Return a zeroed image descriptor for `static` initialisation.
    pub const fn empty() -> Self {
        KexecImage {
            segments: [KexecSegment::empty(); KEXEC_MAX_SEGMENTS],
            nsegments: 0,
            entry_addr: 0,
            state: KexecState::Empty,
            cmdline: [0u8; KEXEC_CMDLINE_MAX],
            cmdline_len: 0,
            is_crash: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static KEXEC_IMAGE: Mutex<KexecImage> = Mutex::new(KexecImage::empty());

// ---------------------------------------------------------------------------
// Public API — image construction
// ---------------------------------------------------------------------------

/// Add one memory segment to the kexec image.
///
/// Transitions the image state from `Empty` → `Loading` if needed.
///
/// Returns `false` if:
///   - The segment table is full (`KEXEC_MAX_SEGMENTS` reached).
///   - The image is in `Loaded` or `Executing` state (call `kexec_unload` first).
///   - `src_len == 0` or `dst_len == 0`.
pub fn kexec_add_segment(src: u64, src_len: usize, dst: u64, dst_len: usize) -> bool {
    if src_len == 0 || dst_len == 0 {
        return false;
    }

    let mut img = KEXEC_IMAGE.lock();

    match img.state {
        KexecState::Loaded | KexecState::Executing => return false,
        KexecState::Empty => img.state = KexecState::Loading,
        KexecState::Loading => {}
    }

    let n = img.nsegments as usize;
    if n >= KEXEC_MAX_SEGMENTS {
        return false;
    }

    img.segments[n] = KexecSegment {
        src,
        src_len,
        dst,
        dst_len,
        active: true,
    };
    // Saturating increment — nsegments is bounded by KEXEC_MAX_SEGMENTS check above.
    img.nsegments = img.nsegments.saturating_add(1);
    true
}

/// Set the entry-point physical address for the new kernel.
pub fn kexec_set_entry(addr: u64) {
    KEXEC_IMAGE.lock().entry_addr = addr;
}

/// Copy `cmdline` into the image's command-line buffer.
///
/// Silently truncates if `cmdline.len() > KEXEC_CMDLINE_MAX`.
///
/// Returns `false` if the image is in `Executing` state.
pub fn kexec_set_cmdline(cmdline: &[u8]) -> bool {
    let mut img = KEXEC_IMAGE.lock();
    if img.state == KexecState::Executing {
        return false;
    }
    let len = cmdline.len().min(KEXEC_CMDLINE_MAX);
    img.cmdline[..len].copy_from_slice(&cmdline[..len]);
    // Zero any trailing bytes from a previous call.
    for b in img.cmdline[len..].iter_mut() {
        *b = 0;
    }
    img.cmdline_len = len as u16;
    true
}

// ---------------------------------------------------------------------------
// Public API — image control
// ---------------------------------------------------------------------------

/// Validate and finalise the kexec image, transitioning to `Loaded` state.
///
/// Validation rules:
///   - `entry_addr` must be non-zero.
///   - At least one segment must be present (`nsegments >= 1`).
///   - Current state must be `Loading`.
///
/// Returns `false` if any validation check fails.
pub fn kexec_load() -> bool {
    let mut img = KEXEC_IMAGE.lock();

    if img.state != KexecState::Loading {
        return false;
    }
    if img.entry_addr == 0 {
        return false;
    }
    if img.nsegments == 0 {
        return false;
    }

    img.state = KexecState::Loaded;
    true
}

/// Simulate execution of the loaded kernel image.
///
/// Transitions state to `Executing` and prints the hand-off message.
/// In a physical kernel this function would:
///   1. Disable all interrupts (`cli`).
///   2. Flush caches.
///   3. Disable paging / switch to identity mapping.
///   4. Far-jump to `entry_addr`.
///
/// Because this is a metadata stub it does not perform the actual jump and
/// returns `true` to indicate the (simulated) hand-off was initiated.
///
/// Returns `false` if the image is not in `Loaded` state.
pub fn kexec_execute() -> bool {
    let mut img = KEXEC_IMAGE.lock();

    if img.state != KexecState::Loaded {
        return false;
    }

    let entry = img.entry_addr;
    img.state = KexecState::Executing;
    drop(img);

    serial_println!("[kexec] executing new kernel at {:#x}", entry);
    // In a real kernel: disable_irqs(); flush_tlb_all(); jump_to(entry);
    true
}

/// Reset the kexec image back to `Empty` state, discarding all segments.
///
/// Returns `false` if the image is currently `Executing` (hand-off in
/// progress — unsafe to abort).
pub fn kexec_unload() -> bool {
    let mut img = KEXEC_IMAGE.lock();
    if img.state == KexecState::Executing {
        return false;
    }
    *img = KexecImage::empty();
    true
}

// ---------------------------------------------------------------------------
// Public API — query
// ---------------------------------------------------------------------------

/// Return the current state of the kexec image.
pub fn kexec_get_state() -> KexecState {
    KEXEC_IMAGE.lock().state
}

/// Return `true` if the loaded image is a kdump / crash kernel.
pub fn kexec_is_crash_kernel() -> bool {
    KEXEC_IMAGE.lock().is_crash
}

// ---------------------------------------------------------------------------
// kdump crash-kernel support
// ---------------------------------------------------------------------------

/// Pre-load a crash kernel entry point for kdump.
///
/// Should be called during normal boot to prepare for a potential crash.
/// Sets `entry_addr`, marks `is_crash = true`, and creates a single
/// synthetic segment so `kexec_load()` can succeed.
///
/// Returns `false` if `crash_entry == 0` or the image is already in use.
pub fn kdump_crash_init(crash_entry: u64) -> bool {
    if crash_entry == 0 {
        return false;
    }

    let mut img = KEXEC_IMAGE.lock();

    // Only initialise if the image slot is free.
    if img.state != KexecState::Empty {
        return false;
    }

    img.entry_addr = crash_entry;
    img.is_crash = true;
    img.state = KexecState::Loading;

    // Register a symbolic crash-kernel segment so validation passes.
    // The physical address is symbolic — a real implementation would use
    // the region reserved via `crashkernel=` boot parameter.
    img.segments[0] = KexecSegment {
        src: crash_entry,
        src_len: 1, // placeholder — actual size comes from ELF headers
        dst: crash_entry,
        dst_len: 1,
        active: true,
    };
    img.nsegments = 1;

    drop(img);

    // Finalise into Loaded state.
    kexec_load()
}

/// Trigger the crash kernel (called from the kernel panic handler).
///
/// Returns `false` if no crash kernel is loaded.
pub fn kdump_trigger() -> bool {
    if !kexec_is_crash_kernel() {
        return false;
    }
    serial_println!("[kexec] kdump triggered — handing off to crash kernel");
    kexec_execute()
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the kexec / kdump framework.
pub fn init() {
    // The global image is already zeroed via `KexecImage::empty()` in the
    // static initialiser.  Nothing else is required at this point.
    serial_println!("[kexec] kexec/kdump framework initialized");
}
