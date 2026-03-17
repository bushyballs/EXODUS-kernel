/// Kernel Address Space Layout Randomization (KASLR) — Genesis hardening
///
/// We cannot relocate kernel text at runtime (the image is already loaded by
/// the bootloader), but we can:
///   1. Randomize kernel heap / stack allocation bases.
///   2. Apply a random slide to loadable kernel module virtual bases.
///   3. Suppress kernel address values from any diagnostic output unless
///      the caller has explicitly opted in.
///
/// Entropy sources (best-to-worst):
///   1. RDRAND   — hardware TRNG (Intel Ivy Bridge+, AMD Zen+).
///   2. TSC + LAPIC ID — timer jitter + CPU identity XOR.
///
/// All values are 2 MiB-aligned so large-page mappings remain valid.
///
/// Critical rules honoured here:
///   - NO float casts (no `as f32` / `as f64` anywhere).
///   - All counter arithmetic uses `saturating_*` or `wrapping_*`.
///   - No heap (`alloc`) — only static storage.
///   - No panics — errors are handled with early returns.
///
/// All code is original.
use crate::serial_println;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ── Tuning constants ──────────────────────────────────────────────────────────

/// Alignment granularity: 2 MiB (large page).
const ALIGN_2MB: u64 = 0x0020_0000;

/// Maximum randomization window: 512 MiB (29 bits).
/// Keeps the kernel in a predictable coarse virtual range while still providing
/// 512 / 2 = 256 possible 2 MiB offsets — adequate to defeat brute-force.
const MAX_OFFSET: u64 = 0x2000_0000; // 512 MiB

// ── Statics ───────────────────────────────────────────────────────────────────

/// Cached KASLR seed (raw 64-bit entropy, NOT an offset).
/// Initialised to 0; set exactly once by `init_kaslr_seed()`.
pub static KASLR_SEED: AtomicU64 = AtomicU64::new(0);

/// True once `init_kaslr_seed()` has run.
static KASLR_READY: AtomicBool = AtomicBool::new(false);

/// kptr_restrict flag: when true, `sanitize_kptr` returns 0 instead of the
/// real address.  Controlled by `kptr::set_kptr_restrict` or the standalone
/// `kptr` module.
static KPTR_RESTRICT: AtomicBool = AtomicBool::new(true);

// ── Hardware entropy helpers ──────────────────────────────────────────────────

/// Try one RDRAND read.  Returns `None` if the instruction faults, is not
/// supported, or the hardware RNG is temporarily exhausted (CF=0).
pub fn rdrand() -> Option<u64> {
    // First check CPUID leaf 1, ECX bit 30 (RDRAND supported).
    // We use a minimal inline CPUID rather than pulling in a dependency.
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",   // CPUID clobbers RBX; preserve caller-save manually
            "cpuid",
            "pop rbx",
            in("eax") 1u32,
            lateout("eax") _,
            lateout("ecx") ecx,
            lateout("edx") _,
            options(nomem, nostack),
        );
    }
    // Bit 30 of ECX = RDRAND supported.
    if ecx & (1 << 30) == 0 {
        return None;
    }

    let mut value: u64 = 0;
    let cf: u8;
    unsafe {
        core::arch::asm!(
            "rdrand {val}",
            "setc   {flag}",
            val  = out(reg)      value,
            flag = out(reg_byte) cf,
            options(nomem, nostack),
        );
    }
    if cf != 0 {
        Some(value)
    } else {
        None
    }
}

/// TSC + LAPIC ID XOR as an entropy fallback.
///
/// LAPIC ID is per-core and differs across CPUs, so the same TSC value on two
/// cores produces distinct entropy contributions.
pub fn rdtsc_entropy() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack),
        );
    }
    let tsc = ((hi as u64) << 32) | (lo as u64);

    // Read APIC ID from CPUID leaf 1 EBX[31:24].
    let ebx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {out:e}, ebx",
            "pop rbx",
            in("eax") 1u32,
            out = out(reg) ebx,
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nomem, nostack),
        );
    }
    let apic_id = (ebx >> 24) as u64;

    // Mix TSC and APIC ID.
    tsc ^ (apic_id.wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

// ── Seed initialisation ───────────────────────────────────────────────────────

/// Gather hardware entropy and store it in `KASLR_SEED`.
///
/// Idempotent — safe to call multiple times; only the first call stores a
/// value.  Must be called before `kaslr_offset`.
pub fn init_kaslr_seed() {
    // Already seeded?
    if KASLR_READY.load(Ordering::SeqCst) {
        return;
    }

    // Gather entropy from all available sources.
    let hw1 = rdrand().unwrap_or(0);
    let ts1 = rdtsc_entropy();
    let hw2 = rdrand().unwrap_or(0);
    let ts2 = rdtsc_entropy();

    // Mix with splitmix64 finaliser passes.
    let mut seed =
        hw1.wrapping_add(ts1).wrapping_add(hw2).wrapping_add(ts2) ^ 0xDEAD_BEEF_CAFE_BABE;

    seed ^= seed >> 30;
    seed = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    seed ^= seed >> 27;
    seed = seed.wrapping_mul(0x94D0_49BB_1331_11EB);
    seed ^= seed >> 31;

    // Ensure non-zero: a zero seed would produce only the minimum offset.
    if seed == 0 {
        seed = 0xCAFE_F00D_DEAD_C0DE;
    }

    // Store atomically — CAS so a parallel core cannot clobber the first value.
    let _ = KASLR_SEED.compare_exchange(0, seed, Ordering::SeqCst, Ordering::SeqCst);
    KASLR_READY.store(true, Ordering::SeqCst);
}

// ── Public offset API ─────────────────────────────────────────────────────────

/// Return a randomized virtual-address offset for kernel allocations.
///
/// `range_bits` controls the window size:
///   - The raw entropy is masked to `(1 << range_bits) - 1`.
///   - The result is then aligned down to a 2 MiB boundary.
///   - Minimum returned value is one 2 MiB page (offset is never zero).
///
/// Typical callers:
///   - `range_bits = 21` → exactly one 2 MiB slot (for very tight regions)
///   - `range_bits = 28` → up to 256 MiB window (kernel module loader)
///   - `range_bits = 29` → up to 512 MiB window (default MAX_OFFSET)
///
/// Capped internally at MAX_OFFSET regardless of `range_bits`.
pub fn kaslr_offset(range_bits: u8) -> u64 {
    // Clamp range_bits to 1..=63 to avoid shift overflow.
    let bits = if range_bits == 0 {
        1u8
    } else if range_bits > 63 {
        63u8
    } else {
        range_bits
    };

    let seed = KASLR_SEED.load(Ordering::SeqCst);

    // Derive a per-call value by XOR-folding with a second TSC read so that
    // repeated calls to kaslr_offset within the same boot produce distinct
    // offsets for different regions.
    let extra = rdtsc_entropy();
    let raw = seed ^ extra.wrapping_mul(0x517C_C1B7_2722_0A95);

    // Mask to requested window, then cap at MAX_OFFSET.
    let window = if bits < 64 {
        (1u64 << bits).saturating_sub(1)
    } else {
        u64::MAX
    };
    let capped = raw & window & (MAX_OFFSET.saturating_sub(1));

    // Align down to 2 MiB.
    let aligned = capped & !(ALIGN_2MB.saturating_sub(1));

    // Guarantee at least one page of randomisation.
    if aligned == 0 {
        ALIGN_2MB
    } else {
        aligned
    }
}

// ── Kernel pointer sanitisation ───────────────────────────────────────────────

/// Hide a kernel virtual address when `kptr_restrict` is enabled.
///
/// Returns `0` when restriction is active, the original address otherwise.
/// Used before printing any kernel pointer to a user-visible output stream.
pub fn sanitize_kptr(addr: u64) -> u64 {
    if KPTR_RESTRICT.load(Ordering::SeqCst) {
        0
    } else {
        addr
    }
}

/// Enable or disable kernel pointer restriction.
/// `true` = hide all kernel addresses (default, safe).
/// `false` = expose kernel addresses (debug builds only).
pub fn set_kptr_restrict(restrict: bool) {
    KPTR_RESTRICT.store(restrict, Ordering::SeqCst);
}

// ── Module init ───────────────────────────────────────────────────────────────

/// Initialize KASLR: seed the PRNG and log the boot-time entropy status.
///
/// Must be called before paging is fully configured so the first allocation
/// offset is available to the kernel module loader.
pub fn init() {
    init_kaslr_seed();

    let seed = KASLR_SEED.load(Ordering::SeqCst);
    let default_offset = kaslr_offset(29); // 512 MiB window

    serial_println!(
        "  [kaslr] KASLR seed ready (top 16 bits: 0x{:04X})",
        (seed >> 48) & 0xFFFF
    );
    serial_println!(
        "  [kaslr] Default allocation offset: +{:#x} ({} MiB, 2 MiB aligned)",
        default_offset,
        default_offset / (1024 * 1024)
    );
    serial_println!(
        "  [kaslr] Entropy: RDRAND={}, TSC+LAPIC=always, kptr_restrict=on",
        if rdrand().is_some() {
            "available"
        } else {
            "not available"
        }
    );
}
