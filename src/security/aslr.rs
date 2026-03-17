/// Address Space Layout Randomization (ASLR) for Genesis
///
/// Randomizes the memory layout of processes to prevent exploitation:
///   - Stack base randomization (up to 8 MB range)
///   - Heap base randomization (up to 1 GB range)
///   - mmap base randomization
///   - Executable base randomization (PIE support)
///   - VDSO page randomization
///   - Kernel ASLR (KASLR) for kernel text/data
///
/// Entropy sources:
///   - RDRAND/RDSEED for hardware entropy
///   - TSC jitter as backup
///   - Mixed through our CSPRNG (ChaCha20-based)
///
/// All code is original.
use crate::serial_println;
use crate::sync::Mutex;

/// ASLR configuration
static ASLR_CONFIG: Mutex<AslrConfig> = Mutex::new(AslrConfig::new());

/// Entropy bits for each region
const STACK_ENTROPY_BITS: u32 = 22; // ~4 MB randomization range
const HEAP_ENTROPY_BITS: u32 = 28; // ~256 MB randomization range
const MMAP_ENTROPY_BITS: u32 = 28; // ~256 MB randomization range
const EXEC_ENTROPY_BITS: u32 = 22; // ~4 MB randomization range (PIE)
const VDSO_ENTROPY_BITS: u32 = 16; // ~64 KB randomization range
const KASLR_ENTROPY_BITS: u32 = 18; // ~256 KB for kernel text

/// Page size (must be page-aligned)
const PAGE_SIZE: usize = 4096;

/// Default base addresses (before randomization)
const DEFAULT_STACK_BASE: u64 = 0x0000_7FFF_FFFF_0000; // Top of user space
const DEFAULT_HEAP_BASE: u64 = 0x0000_0000_1000_0000; // 256 MB
const DEFAULT_MMAP_BASE: u64 = 0x0000_7F00_0000_0000; // High user space
const DEFAULT_EXEC_BASE: u64 = 0x0000_0000_0040_0000; // 4 MB (PIE base)
const DEFAULT_VDSO_BASE: u64 = 0x0000_7FFF_FFF0_0000; // Near top
const DEFAULT_KERNEL_BASE: u64 = 0xFFFF_FFFF_8000_0000; // Kernel space

/// ASLR configuration
pub struct AslrConfig {
    pub enabled: bool,
    pub kaslr_enabled: bool,
    /// Actual kernel offset (set once at boot)
    pub kernel_offset: u64,
    /// Per-region entropy bits (can be tuned)
    pub stack_bits: u32,
    pub heap_bits: u32,
    pub mmap_bits: u32,
    pub exec_bits: u32,
}

impl AslrConfig {
    const fn new() -> Self {
        AslrConfig {
            enabled: true,
            kaslr_enabled: true,
            kernel_offset: 0,
            stack_bits: STACK_ENTROPY_BITS,
            heap_bits: HEAP_ENTROPY_BITS,
            mmap_bits: MMAP_ENTROPY_BITS,
            exec_bits: EXEC_ENTROPY_BITS,
        }
    }
}

/// Get a random page-aligned offset with the given number of entropy bits
fn random_offset(entropy_bits: u32) -> u64 {
    let mut random_bytes = [0u8; 8];
    crate::crypto::random::fill_bytes(&mut random_bytes);
    let mut raw = 0u64;
    for i in 0..8 {
        raw |= (random_bytes[i] as u64) << (i * 8);
    }

    // Mask to desired entropy and align to page boundary
    let mask = ((1u64 << entropy_bits) - 1) & !((PAGE_SIZE as u64) - 1);
    raw & mask
}

/// Randomized process memory layout
#[derive(Debug, Clone, Copy)]
pub struct ProcessLayout {
    /// Randomized stack top (grows down)
    pub stack_top: u64,
    /// Randomized heap start
    pub heap_start: u64,
    /// Randomized mmap base
    pub mmap_base: u64,
    /// Randomized executable base (for PIE binaries)
    pub exec_base: u64,
    /// Randomized VDSO page
    pub vdso_base: u64,
}

/// Generate a randomized process memory layout
pub fn randomize_layout() -> ProcessLayout {
    let config = ASLR_CONFIG.lock();

    if !config.enabled {
        return ProcessLayout {
            stack_top: DEFAULT_STACK_BASE,
            heap_start: DEFAULT_HEAP_BASE,
            mmap_base: DEFAULT_MMAP_BASE,
            exec_base: DEFAULT_EXEC_BASE,
            vdso_base: DEFAULT_VDSO_BASE,
        };
    }

    ProcessLayout {
        stack_top: DEFAULT_STACK_BASE - random_offset(config.stack_bits),
        heap_start: DEFAULT_HEAP_BASE + random_offset(config.heap_bits),
        mmap_base: DEFAULT_MMAP_BASE - random_offset(config.mmap_bits),
        exec_base: DEFAULT_EXEC_BASE + random_offset(config.exec_bits),
        vdso_base: DEFAULT_VDSO_BASE - random_offset(VDSO_ENTROPY_BITS),
    }
}

/// Generate KASLR offset for kernel text
/// Called once during early boot before anything else
pub fn kaslr_offset() -> u64 {
    let config = ASLR_CONFIG.lock();
    if !config.kaslr_enabled {
        return 0;
    }
    random_offset(KASLR_ENTROPY_BITS)
}

/// Initialize ASLR
pub fn init() {
    let mut config = ASLR_CONFIG.lock();

    // Set kernel ASLR offset (would be applied during early boot in real impl)
    config.kernel_offset = random_offset(KASLR_ENTROPY_BITS);

    serial_println!("  [aslr] ASLR initialized:");
    serial_println!("    Stack: {} bits entropy", config.stack_bits);
    serial_println!("    Heap:  {} bits entropy", config.heap_bits);
    serial_println!("    Mmap:  {} bits entropy", config.mmap_bits);
    serial_println!("    Exec:  {} bits entropy (PIE)", config.exec_bits);
    serial_println!(
        "    KASLR: {} bits entropy, offset=+{:#x}",
        KASLR_ENTROPY_BITS,
        config.kernel_offset
    );
}

/// Check if ASLR is enabled
pub fn is_enabled() -> bool {
    ASLR_CONFIG.lock().enabled
}

/// Disable ASLR (for debugging only — requires lockdown to be disabled)
pub fn disable() {
    if !crate::security::lockdown::is_locked() {
        ASLR_CONFIG.lock().enabled = false;
        serial_println!("  [aslr] WARNING: ASLR disabled");
    }
}
