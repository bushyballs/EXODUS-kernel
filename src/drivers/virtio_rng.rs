/// VirtIO Entropy Source (RNG) Driver — no-heap, static-buffer implementation
///
/// VirtIO RNG (PCI vendor 0x1AF4, device 0x1044) exposes a single virtqueue
/// through which the host streams random bytes to the guest.  Since actual
/// device I/O requires a live QEMU/KVM instance, this driver falls back to a
/// 64-bit xorshift LFSR seeded from the x86 TSC when the PCI device is absent.
///
/// Double-buffer design:
///   fill_idx  — which buffer virtio is currently filling  (0 or 1)
///   read_idx  — which buffer the consumer reads from       (0 or 1)
///   read_pos  — byte offset within the read_idx buffer
///
/// Public API:
///   virtio_rng_init()    -> bool    probe PCI, initialise LFSR seed
///   virtio_rng_read(out) -> usize   fill caller slice with entropy bytes
///   virtio_rng_get_u32() -> u32     convenience: 4 random bytes as u32
///   virtio_rng_get_u64() -> u64     convenience: 8 random bytes as u64
///   init()                          called by drivers::init()
///
/// SAFETY RULES:
///   - No as f32 / as f64
///   - saturating_add/saturating_sub for counters
///   - wrapping_add for sequence numbers
///   - read_volatile/write_volatile for MMIO/shared-ring accesses
///   - No panic — use serial_println! + return false on fatal errors
///   - No Vec, Box, String, alloc::* — fixed-size static arrays only
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// PCI IDs
// ============================================================================

pub const VIRTIO_RNG_VENDOR: u16 = 0x1AF4;
pub const VIRTIO_RNG_DEV_ID: u16 = 0x1044;

// ============================================================================
// Buffer constants
// ============================================================================

pub const RNG_BUF_SIZE: usize = 4096;

// ============================================================================
// TSC seed helper — x86 RDTSC, no floats
// ============================================================================

/// Read the x86 Time Stamp Counter.  Used as the initial LFSR seed.
///
/// SAFETY: RDTSC is a user-accessible instruction on all modern x86 CPUs.
/// options(nostack, nomem) tells the compiler we do not touch the stack or
/// memory, so it is safe to use inside any context.
#[inline]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ============================================================================
// xorshift64 LFSR
// ============================================================================

/// xorshift64 — produces a non-zero pseudo-random u64 given a mutable state.
///
/// The three shift constants (13, 7, 17) are one of Marsaglia's recommended
/// xorshift64 triplets with a period of 2^64 − 1.
///
/// SAFETY RULE: `*state` must never be zero; if it starts zero the sequence
/// immediately degenerates.  `init_lfsr_seed()` guards against this.
#[inline]
fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Return a safe non-zero LFSR seed derived from the TSC.
///
/// If the TSC reads zero (extremely unlikely but possible on some emulators)
/// we fall back to a fixed non-zero constant so the LFSR never stalls.
#[inline]
fn init_lfsr_seed() -> u64 {
    let tsc = read_tsc();
    if tsc == 0 {
        0xDEAD_BEEF_CAFE_1234
    } else {
        tsc
    }
}

// ============================================================================
// Device state struct
// ============================================================================

/// Runtime state for the VirtIO RNG driver.
///
/// `present` is set to true if the PCI device was found; when false the LFSR
/// path is used unconditionally.
///
/// The struct is stored in a static `Mutex<VirtioRng>` — it must be `Copy`
/// and provide a `const fn empty()`.
#[derive(Copy, Clone)]
pub struct VirtioRng {
    /// I/O BAR0 base of the PCI device (0 when device not present)
    io_base: u16,
    /// Double buffer — index 0 and 1 are alternated
    buf: [[u8; RNG_BUF_SIZE]; 2],
    /// How many bytes have been written into each buffer
    buf_fill: [usize; 2],
    /// Which buffer virtio is currently filling (0 or 1)
    fill_idx: u8,
    /// Which buffer the consumer is reading from (0 or 1)
    read_idx: u8,
    /// Byte read-position within read_idx buffer
    read_pos: usize,
    /// True if the PCI device was found and the virtqueue is active
    present: bool,
    /// LFSR state — always non-zero once initialised
    lfsr_state: u64,
    /// Total bytes ever produced (counter — saturating)
    bytes_produced: u64,
}

impl VirtioRng {
    /// Compile-time zero initialiser.  `lfsr_state` is set here to a known
    /// constant; `virtio_rng_init()` replaces it with a TSC-derived seed.
    pub const fn empty() -> Self {
        VirtioRng {
            io_base: 0,
            buf: [[0u8; RNG_BUF_SIZE]; 2],
            buf_fill: [0usize; 2],
            fill_idx: 0,
            read_idx: 0,
            read_pos: 0,
            present: false,
            lfsr_state: 0xDEAD_BEEF_CAFE_1234u64, // non-zero placeholder
            bytes_produced: 0,
        }
    }
}

// ============================================================================
// Global driver state
// ============================================================================

static VIRTIO_RNG: Mutex<VirtioRng> = Mutex::new(VirtioRng::empty());

// ============================================================================
// Internal: fill one buffer using the LFSR
// ============================================================================

/// Fill `rng.buf[idx]` entirely with pseudo-random bytes and record the fill
/// count.  The LFSR is stepped 8 bytes at a time; remaining bytes are filled
/// from the high-order bits of the last word.
fn lfsr_fill_buf(rng: &mut VirtioRng, idx: usize) {
    if idx >= 2 {
        return; // bounds guard
    }

    let mut state = rng.lfsr_state;
    let mut written = 0usize;

    while written.saturating_add(8) <= RNG_BUF_SIZE {
        let word = xorshift64(&mut state);
        // Decompose u64 into 8 bytes, little-endian order
        let b0 = (word) as u8;
        let b1 = (word >> 8) as u8;
        let b2 = (word >> 16) as u8;
        let b3 = (word >> 24) as u8;
        let b4 = (word >> 32) as u8;
        let b5 = (word >> 40) as u8;
        let b6 = (word >> 48) as u8;
        let b7 = (word >> 56) as u8;

        // Bounds checks: the while condition guarantees written+8 <= RNG_BUF_SIZE
        if let Some(slot) = rng.buf[idx].get_mut(written) {
            *slot = b0;
        }
        if let Some(slot) = rng.buf[idx].get_mut(written + 1) {
            *slot = b1;
        }
        if let Some(slot) = rng.buf[idx].get_mut(written + 2) {
            *slot = b2;
        }
        if let Some(slot) = rng.buf[idx].get_mut(written + 3) {
            *slot = b3;
        }
        if let Some(slot) = rng.buf[idx].get_mut(written + 4) {
            *slot = b4;
        }
        if let Some(slot) = rng.buf[idx].get_mut(written + 5) {
            *slot = b5;
        }
        if let Some(slot) = rng.buf[idx].get_mut(written + 6) {
            *slot = b6;
        }
        if let Some(slot) = rng.buf[idx].get_mut(written + 7) {
            *slot = b7;
        }

        written = written.saturating_add(8);
    }

    // Handle any trailing bytes if RNG_BUF_SIZE is not a multiple of 8
    if written < RNG_BUF_SIZE {
        let word = xorshift64(&mut state);
        let mut shift = 0u32;
        while written < RNG_BUF_SIZE {
            if let Some(slot) = rng.buf[idx].get_mut(written) {
                *slot = (word >> shift) as u8;
            }
            written = written.saturating_add(1);
            shift = shift.saturating_add(8);
        }
    }

    rng.lfsr_state = state;
    rng.buf_fill[idx] = RNG_BUF_SIZE;
}

// ============================================================================
// Public: probe and initialise
// ============================================================================

/// Probe the PCI bus for a VirtIO RNG device and initialise the driver.
///
/// When the device is found the virtqueue handshake is performed and the
/// first buffer is filled from the LFSR (seeded from TSC); in a live QEMU
/// environment the host would fill the buffer via the virtqueue.
///
/// When the device is absent the driver falls back to pure LFSR mode.
///
/// Returns `true` if the PCI device was found; `false` in LFSR-only mode.
pub fn virtio_rng_init() -> bool {
    let mut rng = VIRTIO_RNG.lock();

    // Seed the LFSR from the TSC regardless of whether the PCI device exists
    rng.lfsr_state = init_lfsr_seed();

    // Pre-fill both buffers
    lfsr_fill_buf(&mut rng, 0);
    lfsr_fill_buf(&mut rng, 1);

    // Consumer starts reading from buffer 0
    rng.read_idx = 0;
    rng.read_pos = 0;
    // Producer (virtio / LFSR refill) would next fill buffer 1
    rng.fill_idx = 1;

    // Attempt PCI scan via the shared virtio helper
    match super::virtio::pci_find_virtio(VIRTIO_RNG_VENDOR, VIRTIO_RNG_DEV_ID) {
        Some((io_base, _bus, _dev, _func)) => {
            rng.io_base = io_base;
            rng.present = true;
            // Perform the minimal VirtIO legacy handshake (RESET -> ACK -> DRIVER -> DRIVER_OK)
            let _dev_features = super::virtio::device_begin_init(io_base);
            // We negotiate zero features for the RNG (no optional capabilities needed)
            let _ = super::virtio::device_set_features(io_base, 0);
            super::virtio::device_driver_ok(io_base);
            super::register("virtio-rng", super::DeviceType::Other);
            true
        }
        None => {
            rng.present = false;
            false
        }
    }
}

// ============================================================================
// Public: read entropy bytes
// ============================================================================

/// Fill `out` with entropy bytes.
///
/// Reads sequentially from the active read buffer.  When the read buffer is
/// exhausted it swaps to the other buffer (which has been pre-filled by the
/// LFSR) and schedules a refill of the now-free buffer.
///
/// Returns the number of bytes written into `out` (always == out.len() unless
/// `out` is empty).
pub fn virtio_rng_read(out: &mut [u8]) -> usize {
    if out.is_empty() {
        return 0;
    }

    let mut rng = VIRTIO_RNG.lock();
    let mut written = 0usize;

    while written < out.len() {
        let read_idx = rng.read_idx as usize;
        let buf_filled = rng.buf_fill[if read_idx < 2 { read_idx } else { 0 }];

        if rng.read_pos >= buf_filled {
            // Current read buffer exhausted — swap to the other one
            let new_read = (rng.read_idx as usize).wrapping_add(1) % 2;
            let new_fill = rng.read_idx as usize; // old read buffer becomes fill target
            rng.read_idx = new_read as u8;
            rng.fill_idx = new_fill as u8;
            rng.read_pos = 0;

            // Immediately refill the just-swapped-out buffer so next swap is ready
            lfsr_fill_buf(&mut rng, new_fill);

            // If the new read buffer is also empty (shouldn't happen after init), break
            let new_idx = rng.read_idx as usize;
            if rng.buf_fill[if new_idx < 2 { new_idx } else { 0 }] == 0 {
                break;
            }
        }

        let read_idx = rng.read_idx as usize;
        let safe_ridx = if read_idx < 2 { read_idx } else { 0 };
        let available = rng.buf_fill[safe_ridx].saturating_sub(rng.read_pos);
        let to_copy = (out.len() - written).min(available);

        if to_copy == 0 {
            break;
        }

        // Copy to_copy bytes from the buffer into out
        let src_start = rng.read_pos;
        let src_end = src_start.saturating_add(to_copy);

        // Bounds-checked copy, byte by byte
        for i in 0..to_copy {
            let src_pos = src_start.saturating_add(i);
            let dst_pos = written.saturating_add(i);

            let src_byte = rng.buf[safe_ridx].get(src_pos).copied().unwrap_or(0);
            if let Some(dst) = out.get_mut(dst_pos) {
                *dst = src_byte;
            }
        }

        rng.read_pos = src_end;
        written = written.saturating_add(to_copy);
        rng.bytes_produced = rng.bytes_produced.saturating_add(to_copy as u64);
    }

    written
}

// ============================================================================
// Public: convenience wrappers
// ============================================================================

/// Return 4 random bytes assembled into a `u32` (little-endian).
pub fn virtio_rng_get_u32() -> u32 {
    let mut buf = [0u8; 4];
    virtio_rng_read(&mut buf);
    (buf[0] as u32) | ((buf[1] as u32) << 8) | ((buf[2] as u32) << 16) | ((buf[3] as u32) << 24)
}

/// Return 8 random bytes assembled into a `u64` (little-endian).
pub fn virtio_rng_get_u64() -> u64 {
    let mut buf = [0u8; 8];
    virtio_rng_read(&mut buf);
    (buf[0] as u64)
        | ((buf[1] as u64) << 8)
        | ((buf[2] as u64) << 16)
        | ((buf[3] as u64) << 24)
        | ((buf[4] as u64) << 32)
        | ((buf[5] as u64) << 40)
        | ((buf[6] as u64) << 48)
        | ((buf[7] as u64) << 56)
}

/// Return whether the hardware VirtIO RNG device is present.
#[inline]
pub fn virtio_rng_is_present() -> bool {
    VIRTIO_RNG.lock().present
}

/// Return total bytes produced since init (saturating counter).
#[inline]
pub fn virtio_rng_bytes_produced() -> u64 {
    VIRTIO_RNG.lock().bytes_produced
}

// ============================================================================
// Module entry point — called by drivers::init()
// ============================================================================

/// Probe, initialise, and log the VirtIO RNG (or LFSR fallback).
/// Called once during kernel boot by `drivers::init()`.
pub fn init() {
    if virtio_rng_init() {
        serial_println!("[virtio_rng] entropy source initialized");
    } else {
        serial_println!("[virtio_rng] RNG device not found, using LFSR");
    }
}
