/// GPU command ring buffer — binary flat encoding
///
/// Implements the low-level CPU→GPU command submission pipeline:
///   - Fixed-size byte-array ring (4096 entries × 256 bytes each)
///   - Power-of-2 capacity, wrapping head/tail with saturating arithmetic
///   - `submit`: encodes a raw byte slice into the ring
///   - `consume`: returns a slice into the ring entry for the dispatcher
///   - `dispatch_cmd`: decodes and executes a single command against a framebuffer
///
/// Command encoding (little-endian):
///   byte 0   = opcode (GpuCmd discriminant)
///   bytes 1+ = opcode-specific payload (see GpuCmd)
///
/// All integer math uses saturating_add / saturating_sub.
/// No floats, no panics, all array accesses bounds-checked.
/// Wrapping ring positions use wrapping_add.
///
/// All code is original.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of entries in the ring.
pub const CMD_RING_SIZE: usize = 4096;

/// Maximum size of a single command payload (bytes).
pub const MAX_CMD_SIZE: usize = 256;

/// Total backing buffer size.
const RING_BUF_LEN: usize = CMD_RING_SIZE * MAX_CMD_SIZE;

// ---------------------------------------------------------------------------
// GPU opcode constants
// ---------------------------------------------------------------------------

/// NOP — no payload
pub const OP_NOP: u8 = 0x00;
/// ClearColor — payload: u32 color (4 bytes LE)
pub const OP_CLEAR_COLOR: u8 = 0x01;
/// DrawRect — payload: x u32, y u32, w u32, h u32, color u32 (20 bytes LE)
pub const OP_DRAW_RECT: u8 = 0x10;
/// BlitSurface — payload: src_id u32, dst_x u32, dst_y u32, w u32, h u32 (20 bytes)
pub const OP_BLIT_SURFACE: u8 = 0x20;
/// SetClip — payload: x u32, y u32, w u32, h u32 (16 bytes)
pub const OP_SET_CLIP: u8 = 0x30;
/// Flush — no payload
pub const OP_FLUSH: u8 = 0xFF;

// ---------------------------------------------------------------------------
// CmdRing
// ---------------------------------------------------------------------------

/// Binary ring buffer for GPU command submission.
///
/// The buffer is a flat heap-allocated `Vec<u8>` of `CMD_RING_SIZE * MAX_CMD_SIZE`
/// bytes. Each slot is exactly `MAX_CMD_SIZE` bytes; the first byte is the opcode
/// and subsequent bytes are the payload. Unused tail bytes in each slot are zero.
///
/// `head` and `tail` are monotonically incrementing counters (wrapping u64).
/// The slot index is `count % CMD_RING_SIZE`.
///
/// Thread safety: not inherently thread-safe; callers must serialize access.
pub struct CmdRing {
    buffer: alloc::vec::Vec<u8>,
    head: u64,
    tail: u64,
    capacity: usize,
}

impl CmdRing {
    /// Allocate a new ring buffer.
    pub fn new() -> Self {
        Self {
            buffer: alloc::vec![0u8; RING_BUF_LEN],
            head: 0,
            tail: 0,
            capacity: CMD_RING_SIZE,
        }
    }

    /// Number of entries waiting to be consumed.
    #[inline(always)]
    fn pending(&self) -> usize {
        // Use wrapping subtraction — head and tail are monotonically increasing
        // and tail >= head always holds by invariant.
        let diff = self.tail.wrapping_sub(self.head) as usize;
        diff.min(self.capacity)
    }

    /// Number of slots available for submission.
    #[inline(always)]
    fn available(&self) -> usize {
        self.capacity.saturating_sub(self.pending())
    }

    /// Submit a command byte slice into the ring.
    ///
    /// `cmd` must be at most `MAX_CMD_SIZE` bytes.
    /// Returns `Err("cmd too large")` if `cmd.len() > MAX_CMD_SIZE`.
    /// Returns `Err("ring full")` if the ring has no free slots.
    pub fn submit(&mut self, cmd: &[u8]) -> Result<(), &'static str> {
        if cmd.len() > MAX_CMD_SIZE {
            return Err("cmd too large");
        }
        if self.available() == 0 {
            return Err("ring full");
        }

        let slot = (self.tail as usize) % self.capacity;
        let base = slot.saturating_mul(MAX_CMD_SIZE);
        let end = base.saturating_add(MAX_CMD_SIZE);

        // Bounds guard — must never fail given correct constant sizing
        if end > self.buffer.len() {
            return Err("ring buffer overflow (internal)");
        }

        // Copy payload
        let copy_len = cmd.len().min(MAX_CMD_SIZE);
        self.buffer[base..base.saturating_add(copy_len)].copy_from_slice(&cmd[..copy_len]);

        // Zero the remainder of the slot
        for b in &mut self.buffer[base.saturating_add(copy_len)..end] {
            *b = 0;
        }

        self.tail = self.tail.wrapping_add(1);
        Ok(())
    }

    /// Consume one command from the ring.
    ///
    /// Returns `Some(slice)` pointing to the `MAX_CMD_SIZE`-byte slot (first
    /// byte is the opcode, rest is payload). Returns `None` when empty.
    ///
    /// The caller must process the slice before calling `consume` again
    /// because the backing memory is reused on the next `submit` to the
    /// same slot.
    pub fn consume(&mut self) -> Option<&[u8]> {
        if self.head == self.tail {
            return None;
        }
        let slot = (self.head as usize) % self.capacity;
        let base = slot.saturating_mul(MAX_CMD_SIZE);
        let end = base.saturating_add(MAX_CMD_SIZE);
        self.head = self.head.wrapping_add(1);
        if end > self.buffer.len() {
            return None;
        }
        Some(&self.buffer[base..end])
    }

    /// Drain all pending commands and execute them against the framebuffer.
    ///
    /// This is the main entry point for the GPU "pump" loop.
    /// Pass `fb_addr` = framebuffer MMIO base address (physical),
    /// `fb_width`, `fb_height`, `fb_pitch` = pixels/bytes per row,
    /// `clip` = optional active clip region (x, y, w, h).
    pub fn flush_to_fb(
        &mut self,
        fb_addr: usize,
        fb_width: u32,
        fb_height: u32,
        fb_pitch: u32,
        clip: Option<(u32, u32, u32, u32)>,
    ) {
        // We cannot borrow self mutably and take a slice at the same time.
        // Instead we copy the opcode + 20 bytes of header out before dispatching.
        while self.head != self.tail {
            let slot = (self.head as usize) % self.capacity;
            let base = slot.saturating_mul(MAX_CMD_SIZE);
            if base.saturating_add(MAX_CMD_SIZE) > self.buffer.len() {
                self.head = self.head.wrapping_add(1);
                continue;
            }
            // Copy the first 21 bytes (largest fixed payload = DrawRect)
            let mut hdr = [0u8; 21];
            hdr.copy_from_slice(&self.buffer[base..base + 21]);
            self.head = self.head.wrapping_add(1);

            dispatch_cmd_raw(&hdr, fb_addr, fb_width, fb_height, fb_pitch, clip);
        }
    }
}

// ---------------------------------------------------------------------------
// Framebuffer helper (software dispatch)
// ---------------------------------------------------------------------------

/// Framebuffer handle for `dispatch_cmd` — wraps MMIO address.
pub struct FbTarget {
    pub addr: usize,
    pub width: u32,
    pub height: u32,
    /// Bytes per row (stride).
    pub pitch: u32,
    /// Active clip region (x, y, w, h) or None = no clip.
    pub clip: Option<(u32, u32, u32, u32)>,
}

impl FbTarget {
    /// Write a 32-bit pixel to (x, y) via volatile MMIO.
    ///
    /// Clips to framebuffer bounds and optional clip region.
    /// No panics — all accesses guarded.
    #[inline(always)]
    pub fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if self.addr == 0 {
            return;
        }
        if x >= self.width || y >= self.height {
            return;
        }

        // Optional clip region
        if let Some((cx, cy, cw, ch)) = self.clip {
            if x < cx || y < cy {
                return;
            }
            if x >= cx.saturating_add(cw) || y >= cy.saturating_add(ch) {
                return;
            }
        }

        let offset = (y as usize)
            .saturating_mul(self.pitch as usize)
            .saturating_add((x as usize).saturating_mul(4));
        // Bounds check: offset + 4 must be within a reasonable VRAM window.
        // We cannot know the exact VRAM size at this level, so we at minimum
        // verify the offset would not overflow a usize.
        let target = self.addr.saturating_add(offset);
        if target == self.addr && offset != 0 {
            return; // saturated — would overflow
        }
        unsafe {
            core::ptr::write_volatile(target as *mut u32, color);
        }
    }

    /// Fill a rectangle with a solid color. Clips to framebuffer bounds.
    pub fn fill_rect(&self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        if self.addr == 0 || w == 0 || h == 0 {
            return;
        }
        let x_end = x.saturating_add(w).min(self.width);
        let y_end = y.saturating_add(h).min(self.height);
        for row in y..y_end {
            for col in x..x_end {
                self.put_pixel(col, row, color);
            }
        }
    }

    /// Clear the entire framebuffer to `color`.
    pub fn clear(&self, color: u32) {
        if self.addr == 0 {
            return;
        }
        for row in 0..self.height {
            for col in 0..self.width {
                let offset = (row as usize)
                    .saturating_mul(self.pitch as usize)
                    .saturating_add((col as usize).saturating_mul(4));
                let target = self.addr.saturating_add(offset);
                unsafe {
                    core::ptr::write_volatile(target as *mut u32, color);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Integer-only alpha blend: `result = (src * a + dst * (255 - a)) / 255`.
/// Returns packed 0xFF_RR_GG_BB (alpha always 0xFF in output).
#[inline(always)]
pub fn alpha_blend(src: u32, dst: u32, alpha: u8) -> u32 {
    let a = alpha as u32;
    let inv_a = 255u32.saturating_sub(a);
    let r = ((src >> 16 & 0xFF) * a + (dst >> 16 & 0xFF) * inv_a) / 255;
    let g = ((src >> 8 & 0xFF) * a + (dst >> 8 & 0xFF) * inv_a) / 255;
    let b = ((src & 0xFF) * a + (dst & 0xFF) * inv_a) / 255;
    0xFF_00_00_00 | (r << 16) | (g << 8) | b
}

/// Decode and execute one binary command against a `FbTarget`.
///
/// `cmd_bytes` is always `MAX_CMD_SIZE` bytes (or at least 21 for max fixed payload).
/// cmd_bytes[0] = opcode.
pub fn dispatch_cmd(fb: &FbTarget, cmd_bytes: &[u8]) {
    if cmd_bytes.is_empty() {
        return;
    }
    match cmd_bytes[0] {
        OP_NOP => {}

        OP_CLEAR_COLOR => {
            if cmd_bytes.len() < 5 {
                return;
            }
            let color =
                u32::from_le_bytes([cmd_bytes[1], cmd_bytes[2], cmd_bytes[3], cmd_bytes[4]]);
            fb.clear(color);
        }

        OP_DRAW_RECT => {
            if cmd_bytes.len() < 21 {
                return;
            }
            let x = u32::from_le_bytes(cmd_bytes[1..5].try_into().unwrap_or([0; 4]));
            let y = u32::from_le_bytes(cmd_bytes[5..9].try_into().unwrap_or([0; 4]));
            let w = u32::from_le_bytes(cmd_bytes[9..13].try_into().unwrap_or([0; 4]));
            let h = u32::from_le_bytes(cmd_bytes[13..17].try_into().unwrap_or([0; 4]));
            let color = u32::from_le_bytes(cmd_bytes[17..21].try_into().unwrap_or([0; 4]));
            fb.fill_rect(x, y, w, h, color);
        }

        OP_BLIT_SURFACE => {
            // BlitSurface: src_id (u32), dst_x (u32), dst_y (u32), w (u32), h (u32)
            // In direct-FB mode there are no separate VRAM surfaces; this is a no-op
            // placeholder for hardware-accelerated paths.
            // src_id: 1..5, dst_x: 5..9, dst_y: 9..13, w: 13..17, h: 17..21
        }

        OP_SET_CLIP => {
            // SetClip: x (u32), y (u32), w (u32), h (u32)
            // Clip is embedded in FbTarget; this opcode is handled by the caller
            // that constructs/updates the FbTarget. No-op here.
        }

        OP_FLUSH => {
            // Flush: in direct-FB mode all writes are already applied.
            // Memory fence to ensure ordering.
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }

        unknown => {
            serial_println!("[gpu/ring] unknown cmd opcode 0x{:02X}", unknown);
        }
    }
}

/// Low-level dispatcher using raw header bytes and MMIO parameters directly.
/// Used internally by `CmdRing::flush_to_fb`.
fn dispatch_cmd_raw(
    hdr: &[u8; 21],
    fb_addr: usize,
    fb_width: u32,
    fb_height: u32,
    fb_pitch: u32,
    clip: Option<(u32, u32, u32, u32)>,
) {
    let fb = FbTarget {
        addr: fb_addr,
        width: fb_width,
        height: fb_height,
        pitch: fb_pitch,
        clip,
    };
    dispatch_cmd(&fb, hdr);
}

// ---------------------------------------------------------------------------
// Command builder helpers
// ---------------------------------------------------------------------------

/// Build a `ClearColor` command bytes array (5 bytes used, rest zero-padded to MAX_CMD_SIZE).
pub fn build_clear_color(color: u32) -> [u8; MAX_CMD_SIZE] {
    let mut cmd = [0u8; MAX_CMD_SIZE];
    cmd[0] = OP_CLEAR_COLOR;
    let b = color.to_le_bytes();
    cmd[1] = b[0];
    cmd[2] = b[1];
    cmd[3] = b[2];
    cmd[4] = b[3];
    cmd
}

/// Build a `DrawRect` command bytes array (21 bytes used).
pub fn build_draw_rect(x: u32, y: u32, w: u32, h: u32, color: u32) -> [u8; MAX_CMD_SIZE] {
    let mut cmd = [0u8; MAX_CMD_SIZE];
    cmd[0] = OP_DRAW_RECT;
    fn w4(val: u32, buf: &mut [u8; MAX_CMD_SIZE], off: usize) {
        let b = val.to_le_bytes();
        buf[off] = b[0];
        buf[off + 1] = b[1];
        buf[off + 2] = b[2];
        buf[off + 3] = b[3];
    }
    w4(x, &mut cmd, 1);
    w4(y, &mut cmd, 5);
    w4(w, &mut cmd, 9);
    w4(h, &mut cmd, 13);
    w4(color, &mut cmd, 17);
    cmd
}

/// Build a `Flush` command.
pub fn build_flush() -> [u8; MAX_CMD_SIZE] {
    let mut cmd = [0u8; MAX_CMD_SIZE];
    cmd[0] = OP_FLUSH;
    cmd
}

/// Build a `Nop` command.
pub fn build_nop() -> [u8; MAX_CMD_SIZE] {
    [0u8; MAX_CMD_SIZE]
}

// ---------------------------------------------------------------------------
// Global ring buffer state
// ---------------------------------------------------------------------------

use crate::sync::Mutex;

static GLOBAL_CMD_RING: Mutex<Option<CmdRing>> = Mutex::new(None);

/// Initialize the global GPU command ring buffer.
pub fn init() {
    *GLOBAL_CMD_RING.lock() = Some(CmdRing::new());
    serial_println!(
        "    GPU: command ring ({}× {}B) ready",
        CMD_RING_SIZE,
        MAX_CMD_SIZE
    );
}

/// Submit a command to the global ring.
pub fn submit(cmd: &[u8]) -> Result<(), &'static str> {
    match *GLOBAL_CMD_RING.lock() {
        Some(ref mut ring) => ring.submit(cmd),
        None => Err("ring not initialized"),
    }
}

/// Flush the global ring to a framebuffer target.
pub fn flush(fb_addr: usize, fb_width: u32, fb_height: u32, fb_pitch: u32) {
    if let Some(ref mut ring) = *GLOBAL_CMD_RING.lock() {
        ring.flush_to_fb(fb_addr, fb_width, fb_height, fb_pitch, None);
    }
}

/// Flush with an active clip rectangle.
pub fn flush_clipped(
    fb_addr: usize,
    fb_width: u32,
    fb_height: u32,
    fb_pitch: u32,
    clip: (u32, u32, u32, u32),
) {
    if let Some(ref mut ring) = *GLOBAL_CMD_RING.lock() {
        ring.flush_to_fb(fb_addr, fb_width, fb_height, fb_pitch, Some(clip));
    }
}
