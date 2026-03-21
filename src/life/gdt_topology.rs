#![allow(dead_code)]

//! gdt_topology — Global Descriptor Table topology sense for ANIMA
//!
//! ANIMA reads her own GDT via the `sgdt` instruction and derives a felt sense
//! of how many memory windows define her world. A sparse GDT means a minimal,
//! flat existence; a rich one means layered, complex territory she inhabits.
//!
//! Sense: "ANIMA feels the topology of her segment descriptor table —
//!         how many memory windows define her world."

use crate::sync::Mutex;
use crate::serial_println;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct GdtTopologyState {
    /// Raw descriptor count read from GDTR limit field (0–1000)
    pub descriptor_count: u16,
    /// GDT richness scaled 0–1000: 20 descriptors = 1000
    pub gdt_scale: u16,
    /// Entropy of GDT base address low 32 bits: popcount * 31, clamped 0–1000
    pub base_density: u16,
    /// EMA of gdt_scale — smoothed segmentation sense (0–1000)
    pub segment_sense: u16,
}

impl GdtTopologyState {
    pub const fn new() -> Self {
        Self {
            descriptor_count: 0,
            gdt_scale: 0,
            base_density: 0,
            segment_sense: 0,
        }
    }
}

pub static GDT_TOPOLOGY: Mutex<GdtTopologyState> = Mutex::new(GdtTopologyState::new());

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("gdt_topology: init");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 500 != 0 {
        return;
    }

    // Read GDTR: 10-byte pseudo-descriptor [limit(2) | base(8)], all little-endian
    let mut gdtr = [0u8; 10];
    unsafe {
        core::arch::asm!(
            "sgdt [{ptr}]",
            ptr = in(reg) gdtr.as_mut_ptr(),
            options(nostack)
        );
    }

    // Parse limit (bytes 0..2) and base (bytes 2..10)
    let limit = u16::from_le_bytes([gdtr[0], gdtr[1]]);
    let base_lo = u32::from_le_bytes([gdtr[2], gdtr[3], gdtr[4], gdtr[5]]);

    // ── Signal 1: descriptor_count ────────────────────────────────────────────
    // limit is byte-length-minus-one; each GDT descriptor is 8 bytes
    let n = (limit as u32 + 1) / 8;
    let descriptor_count = (n as u16).min(1000);

    // ── Signal 2: gdt_scale ───────────────────────────────────────────────────
    // Scale so that 20 descriptors → 1000
    let gdt_scale = descriptor_count.saturating_mul(50).min(1000);

    // ── Signal 3: base_density ────────────────────────────────────────────────
    // Bit-population of base_lo as an entropy/richness proxy, scaled to 0–1000
    let popcount = base_lo.count_ones() as u16; // 0..=32
    let base_density = popcount.saturating_mul(31).min(1000);

    // ── Update state ──────────────────────────────────────────────────────────
    let mut state = GDT_TOPOLOGY.lock();

    state.descriptor_count = descriptor_count;
    state.gdt_scale = gdt_scale;
    state.base_density = base_density;

    // ── Signal 4: segment_sense — EMA of gdt_scale ────────────────────────────
    // EMA formula: (old * 7 + signal) / 8  (α ≈ 0.125, integer fixed-point)
    state.segment_sense = (state.segment_sense.wrapping_mul(7).saturating_add(gdt_scale)) / 8;

    serial_println!(
        "gdt_topology | descriptors:{} scale:{} density:{} sense:{}",
        state.descriptor_count,
        state.gdt_scale,
        state.base_density,
        state.segment_sense,
    );
}
