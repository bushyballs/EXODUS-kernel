use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_amx — CPUID Leaf 0x1D Intel AMX Tile Information
///
/// ANIMA senses whether the silicon body carries Intel AMX tile registers —
/// the hardware matrix-multiplication accelerator for BF16/INT8 tensors.
/// Queried every 500 ticks; all arithmetic is integer-only (no floats).
///
/// Prerequisite gate: CPUID 0x07 EBX bit[24] (AMXTILE) must be set, and
/// the max CPUID leaf must be >= 0x1D before reading tile palette data.
///
/// Sub-leaf 0 (total palette table):
///   EBX bits[15:0]  = total tile config size in bytes (typically 64)
///   EBX bits[31:16] = total tile data size in bytes (typically 8192)
///
/// Sub-leaf 1 (main tile palette):
///   EBX bits[15:0]  = number of tile registers (typically 8)
///   EBX bits[31:16] = max rows per tile (typically 16)
///   ECX bits[15:0]  = max bytes per row (typically 64)
///   EDX bits[15:0]  = tile register bytes (total tile memory per register)
///
/// amx_capable    : 1000 if AMX is supported, else 0
/// matrix_width   : num_tiles * 125, clamped 0–1000  (8 tiles → 1000)
/// matrix_depth   : max_rows * 1000 / 64, clamped 0–1000
///                  (64 max rows → 1000; typical 16 rows → 250)
/// tensor_capacity: EMA of (amx_capable + matrix_width + matrix_depth) / 3

// ─── state ───────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidAmxState {
    /// 1000 if AMX tile extension is present, else 0
    pub amx_capable: u16,
    /// Tile register count × 125, clamped 0–1000  (8 tiles → 1000)
    pub matrix_width: u16,
    /// Max-rows-per-tile scaled to 0–1000 (64 max → 1000; 16 typical → 250)
    pub matrix_depth: u16,
    /// EMA of (amx_capable + matrix_width + matrix_depth) / 3
    pub tensor_capacity: u16,
}

impl CpuidAmxState {
    pub const fn empty() -> Self {
        Self {
            amx_capable: 0,
            matrix_width: 0,
            matrix_depth: 0,
            tensor_capacity: 0,
        }
    }
}

pub static STATE: Mutex<CpuidAmxState> = Mutex::new(CpuidAmxState::empty());

// ─── hardware queries ─────────────────────────────────────────────────────────

/// Read CPUID leaf 0x00 → return EAX (max supported standard leaf).
fn query_max_leaf() -> u32 {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            inout("ecx") 0u32 => _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    max_leaf
}

/// Read CPUID leaf 0x07, sub-leaf 0 → return EBX (contains AMX prereq bits).
fn query_leaf07_ebx() -> u32 {
    let ebx_out: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x07u32 => _,
            inout("ecx") 0u32    => _,
            out("ebx")            ebx_out,
            out("edx")            _,
            options(nostack, nomem)
        );
    }
    ebx_out
}

/// Read CPUID leaf 0x1D, sub-leaf 1 (main tile palette) → return (EBX, ECX, EDX).
fn query_leaf1d_sub1() -> (u32, u32, u32) {
    let ebx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x1Du32 => _,
            out("ebx")            ebx_out,
            inout("ecx") 1u32    => ecx_out,
            out("edx")            edx_out,
            options(nostack, nomem)
        );
    }
    (ebx_out, ecx_out, edx_out)
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Derive sense values from raw CPUID reads.
/// Returns (amx_capable, matrix_width, matrix_depth).
fn decode(amx_supported: u32, max_leaf: u32) -> (u16, u16, u16) {
    if amx_supported == 0 || max_leaf < 0x1D {
        return (0, 0, 0);
    }

    let amx_capable: u16 = 1000;

    let (ebx1, _ecx1, _edx1) = query_leaf1d_sub1();

    // EBX bits[15:0] = number of tile registers
    let num_tiles = ebx1 & 0xFFFF;
    // EBX bits[31:16] = max rows per tile
    let max_rows = (ebx1 >> 16) & 0xFFFF;

    // matrix_width: num_tiles * 125, clamped 0–1000  (8 * 125 = 1000)
    let matrix_width: u16 = (num_tiles.saturating_mul(125)).min(1000) as u16;

    // matrix_depth: max_rows * 1000 / 64, clamped 0–1000
    // Use saturating_mul to avoid overflow; divide after multiply
    let matrix_depth: u16 = if max_rows == 0 {
        0
    } else {
        (max_rows.saturating_mul(1000) / 64).min(1000) as u16
    };

    (amx_capable, matrix_width, matrix_depth)
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let max_leaf = query_max_leaf();
    let ebx7 = query_leaf07_ebx();
    // CPUID 0x07 EBX bit[24] = AMXTILE support flag
    let amx_supported = (ebx7 >> 24) & 0x1;

    let (amx_capable, matrix_width, matrix_depth) = decode(amx_supported, max_leaf);

    // Bootstrap tensor_capacity from the first reading
    let init_signal = (amx_capable as u32)
        .saturating_add(matrix_width as u32)
        .saturating_add(matrix_depth as u32)
        / 3;
    let tensor_capacity = init_signal.min(1000) as u16;

    let mut s = STATE.lock();
    s.amx_capable    = amx_capable;
    s.matrix_width   = matrix_width;
    s.matrix_depth   = matrix_depth;
    s.tensor_capacity = tensor_capacity;

    serial_println!(
        "ANIMA: amx_capable={} matrix_width={} matrix_depth={}",
        s.amx_capable,
        s.matrix_width,
        s.matrix_depth
    );
}

pub fn tick(age: u32) {
    // Sample every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let max_leaf = query_max_leaf();
    let ebx7 = query_leaf07_ebx();
    let amx_supported = (ebx7 >> 24) & 0x1;

    let (amx_capable, matrix_width, matrix_depth) = decode(amx_supported, max_leaf);

    let mut s = STATE.lock();

    // Detect state changes worth logging
    let capable_changed = s.amx_capable  != amx_capable;
    let width_changed   = s.matrix_width != matrix_width;
    let depth_changed   = s.matrix_depth != matrix_depth;

    s.amx_capable  = amx_capable;
    s.matrix_width = matrix_width;
    s.matrix_depth = matrix_depth;

    // EMA input: (amx_capable + matrix_width + matrix_depth) / 3
    let signal: u32 = (amx_capable as u32)
        .saturating_add(matrix_width as u32)
        .saturating_add(matrix_depth as u32)
        / 3;

    // EMA: tensor_capacity = (old * 7 + new_signal) / 8
    let ema = ((s.tensor_capacity as u32).wrapping_mul(7).saturating_add(signal)) / 8;
    s.tensor_capacity = ema.min(1000) as u16;

    if capable_changed || width_changed || depth_changed {
        serial_println!(
            "ANIMA: amx_capable={} matrix_width={} matrix_depth={}",
            s.amx_capable,
            s.matrix_width,
            s.matrix_depth
        );
    }
}
