use crate::sync::Mutex;
/// GPU driver -- graphics acceleration
///
/// Supports:
///   1. VirtIO GPU (QEMU/KVM virtual GPU)
///   2. Basic VESA/VBE framebuffer (fallback)
///   3. Command submission for 2D acceleration
///   4. Display mode setting
///
/// Architecture:
///   Applications -> Display Server -> GPU Driver -> Hardware
///
/// The GPU driver provides:
///   - Mode setting (resolution, refresh rate, color depth)
///   - 2D blitting, filling, and alpha blending (hw accelerated or sw fallback)
///   - Command ring buffer (producer: CPU, consumer: GPU)
///   - VRAM management (allocate/free surfaces)
///   - Cursor plane support (hardware cursor sprite)
///   - VSync tracking (frame counter, vblank interrupt)
///   - Display output enumeration (HDMI, DP, VGA)
///   - Power management (idle detection, clock scaling)
///
/// Inspired by: DRM/KMS (Linux), Metal (Apple), Vulkan (Khronos).
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

static GPU_STATE: Mutex<Option<GpuDevice>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Display mode
// ---------------------------------------------------------------------------

/// Display mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub bpp: u8,    // bits per pixel
    pub pitch: u32, // bytes per scanline
}

impl DisplayMode {
    pub fn framebuffer_size(&self) -> usize {
        (self.pitch as usize).saturating_mul(self.height as usize)
    }

    /// Calculate pitch from width and bpp
    pub fn calculate_pitch(width: u32, bpp: u8) -> u32 {
        width.saturating_mul(bpp as u32 / 8)
    }
}

/// Standard display modes
pub const MODE_640X480: DisplayMode = DisplayMode {
    width: 640,
    height: 480,
    refresh_hz: 60,
    bpp: 32,
    pitch: 2560,
};
pub const MODE_800X600: DisplayMode = DisplayMode {
    width: 800,
    height: 600,
    refresh_hz: 60,
    bpp: 32,
    pitch: 3200,
};
pub const MODE_1024X768: DisplayMode = DisplayMode {
    width: 1024,
    height: 768,
    refresh_hz: 60,
    bpp: 32,
    pitch: 4096,
};
pub const MODE_1280X720: DisplayMode = DisplayMode {
    width: 1280,
    height: 720,
    refresh_hz: 60,
    bpp: 32,
    pitch: 5120,
};
pub const MODE_1920X1080: DisplayMode = DisplayMode {
    width: 1920,
    height: 1080,
    refresh_hz: 60,
    bpp: 32,
    pitch: 7680,
};
pub const MODE_2560X1440: DisplayMode = DisplayMode {
    width: 2560,
    height: 1440,
    refresh_hz: 60,
    bpp: 32,
    pitch: 10240,
};
pub const MODE_3840X2160: DisplayMode = DisplayMode {
    width: 3840,
    height: 2160,
    refresh_hz: 60,
    bpp: 32,
    pitch: 15360,
};

// ---------------------------------------------------------------------------
// GPU type and connector enums
// ---------------------------------------------------------------------------

/// GPU driver type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuType {
    VesaFb,    // Basic VESA framebuffer
    VirtioGpu, // VirtIO GPU (virtual)
    IntelHd,   // Intel integrated
    AmdRadeon, // AMD discrete
    NvidiaGpu, // NVIDIA discrete (basic, no proprietary)
}

/// Display connector type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorType {
    VGA,
    DVI,
    HDMI,
    DisplayPort,
    LVDS,
    EDP,
    Virtual,
    Unknown,
}

/// Connector status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorStatus {
    Connected,
    Disconnected,
    Unknown,
}

/// A display output connector
#[derive(Debug, Clone)]
pub struct DisplayConnector {
    pub id: u32,
    pub connector_type: ConnectorType,
    pub status: ConnectorStatus,
    pub name: String,
    /// Supported modes for this connector
    pub modes: Vec<DisplayMode>,
    /// Currently active mode (if connected and enabled)
    pub active_mode: Option<DisplayMode>,
    /// Physical size in mm (width, height) -- 0 if unknown
    pub phys_width_mm: u32,
    pub phys_height_mm: u32,
}

// ---------------------------------------------------------------------------
// GPU power management
// ---------------------------------------------------------------------------

/// DPMS control register offset from the GPU MMIO base.
/// Writing to gpu_mmio + DPMS_REG_OFFSET controls display power.
const DPMS_REG_OFFSET: u64 = 0x8010;

/// DPMS value: display on (normal operation).
const DPMS_ON: u32 = 0x0000_0000;

/// DPMS value: display off (suspend / blanked).
const DPMS_OFF: u32 = 0x0000_0001;

/// GPU power state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuPowerState {
    /// Full performance
    Active,
    /// Reduced clock speeds (idle)
    LowPower,
    /// Display off, GPU suspended
    Suspended,
    /// GPU powered off
    Off,
}

/// GPU clock scaling levels
#[derive(Debug, Clone, Copy)]
pub struct ClockState {
    /// Core clock in MHz
    pub core_mhz: u32,
    /// Memory clock in MHz
    pub mem_mhz: u32,
    /// Power level 0 = lowest, higher = more performance
    pub power_level: u8,
}

// ---------------------------------------------------------------------------
// Hardware cursor
// ---------------------------------------------------------------------------

/// Hardware cursor
#[derive(Debug, Clone)]
pub struct HwCursor {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub pixels: Vec<u32>, // ARGB
    pub visible: bool,
    /// VRAM offset where cursor image is stored (for HW cursor planes)
    pub vram_offset: u64,
}

// ---------------------------------------------------------------------------
// VRAM management
// ---------------------------------------------------------------------------

/// A VRAM allocation (surface or texture)
#[derive(Debug, Clone)]
pub struct VramAllocation {
    pub id: u32,
    pub offset: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u8,
    pub in_use: bool,
    pub name: String,
}

/// Simple VRAM allocator using a free-list approach
struct VramAllocator {
    /// Total VRAM size in bytes
    total_size: u64,
    /// List of all allocations (both free and in-use)
    allocations: Vec<VramAllocation>,
    /// Next allocation ID
    next_id: u32,
    /// Amount of VRAM currently allocated
    used: u64,
}

impl VramAllocator {
    const fn new() -> Self {
        VramAllocator {
            total_size: 0,
            allocations: Vec::new(),
            next_id: 1,
            used: 0,
        }
    }

    fn init(&mut self, total: u64) {
        self.total_size = total;
        self.allocations.clear();
        self.next_id = 1;
        self.used = 0;
    }

    /// Allocate a surface in VRAM. Returns allocation ID or None.
    fn alloc(&mut self, width: u32, height: u32, bpp: u8, name: &str) -> Option<u32> {
        let pitch = width.saturating_mul(bpp as u32 / 8);
        let size = (pitch as u64).saturating_mul(height as u64);
        if self.used.saturating_add(size) > self.total_size {
            return None;
        }

        // Find offset: after all existing allocations (simple bump allocator)
        let offset = self
            .allocations
            .iter()
            .filter(|a| a.in_use)
            .map(|a| a.offset.saturating_add(a.size))
            .max()
            .unwrap_or(0);

        if offset.saturating_add(size) > self.total_size {
            return None;
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.used = self.used.saturating_add(size);

        self.allocations.push(VramAllocation {
            id,
            offset,
            size,
            width,
            height,
            pitch,
            bpp,
            in_use: true,
            name: String::from(name),
        });
        Some(id)
    }

    /// Free a VRAM allocation by ID
    fn free(&mut self, id: u32) -> bool {
        if let Some(alloc) = self.allocations.iter_mut().find(|a| a.id == id && a.in_use) {
            alloc.in_use = false;
            self.used = self.used.saturating_sub(alloc.size);
            true
        } else {
            false
        }
    }

    /// Get allocation info by ID
    fn get(&self, id: u32) -> Option<&VramAllocation> {
        self.allocations.iter().find(|a| a.id == id && a.in_use)
    }

    /// Compact free space (defragment)
    fn compact(&mut self) {
        self.allocations.retain(|a| a.in_use);
        // Re-pack offsets
        let mut offset = 0u64;
        for alloc in &mut self.allocations {
            alloc.offset = offset;
            offset = offset.saturating_add(alloc.size);
        }
        self.used = offset;
    }

    /// Get usage statistics
    fn stats(&self) -> (u64, u64, usize) {
        (
            self.used,
            self.total_size,
            self.allocations.iter().filter(|a| a.in_use).count(),
        )
    }
}

// ---------------------------------------------------------------------------
// Command ring buffer
// ---------------------------------------------------------------------------

/// GPU 2D command
#[derive(Debug, Clone)]
pub enum GpuCommand {
    /// Fill a rectangle with a solid color
    FillRect {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: u32,
    },
    /// Copy a rectangle from one location to another
    Blit {
        src_x: u32,
        src_y: u32,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
    },
    /// Copy from a buffer to the framebuffer
    TransferToFb {
        buffer: Vec<u8>,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
        stride: u32,
    },
    /// Update the hardware cursor position
    MoveCursor { x: i32, y: i32 },
    /// Flip to a new framebuffer (page flip / vsync)
    PageFlip,
    /// Alpha-blended blit (src over dst)
    AlphaBlend {
        src_surface: u32,
        src_x: u32,
        src_y: u32,
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
    },
    /// Set cursor image from a VRAM surface
    SetCursorImage {
        surface_id: u32,
        hotspot_x: u32,
        hotspot_y: u32,
    },
    /// Noop / fence marker
    Fence { id: u64 },
}

/// Ring buffer for GPU commands
const CMD_RING_SIZE: usize = 512;

struct CommandRing {
    /// Commands waiting to be executed
    commands: Vec<GpuCommand>,
    /// Total commands ever submitted
    submitted: u64,
    /// Total commands completed
    completed: u64,
    /// Last fence ID completed
    last_fence: u64,
}

impl CommandRing {
    const fn new() -> Self {
        CommandRing {
            commands: Vec::new(),
            submitted: 0,
            completed: 0,
            last_fence: 0,
        }
    }

    fn push(&mut self, cmd: GpuCommand) -> bool {
        if self.commands.len() >= CMD_RING_SIZE {
            return false; // ring full
        }
        self.submitted = self.submitted.saturating_add(1);
        self.commands.push(cmd);
        true
    }

    fn pop(&mut self) -> Option<GpuCommand> {
        if self.commands.is_empty() {
            return None;
        }
        self.completed = self.completed.saturating_add(1);
        Some(self.commands.remove(0))
    }

    fn pending(&self) -> usize {
        self.commands.len()
    }
}

// ---------------------------------------------------------------------------
// VSync tracking
// ---------------------------------------------------------------------------

/// VSync state
struct VsyncState {
    /// Frame counter (incremented each vblank)
    frame_count: u64,
    /// Whether we are currently in the vblank period
    in_vblank: bool,
    /// Whether vsync is enabled (wait for vblank before flip)
    enabled: bool,
    /// Timestamp of last vblank (in ticks)
    last_vblank_tick: u64,
}

impl VsyncState {
    const fn new() -> Self {
        VsyncState {
            frame_count: 0,
            in_vblank: false,
            enabled: true,
            last_vblank_tick: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// GPU device
// ---------------------------------------------------------------------------

/// GPU device state
pub struct GpuDevice {
    pub gpu_type: GpuType,
    pub vendor_id: u16,
    pub device_id: u16,
    pub vendor_name: String,
    pub model_name: String,
    pub vram_size: u64,
    pub current_mode: DisplayMode,
    pub available_modes: Vec<DisplayMode>,
    pub framebuffer_addr: u64,
    pub framebuffer_size: usize,
    pub cursor: HwCursor,
    pub vsync_enabled: bool,
    pub connected_displays: u8,
    pub command_queue: Vec<GpuCommand>,
    connectors: Vec<DisplayConnector>,
    vram: VramAllocator,
    cmd_ring: CommandRing,
    vsync: VsyncState,
    power_state: GpuPowerState,
    clock: ClockState,
    /// Idle tick counter: incremented when no commands are submitted
    idle_ticks: u64,
    /// Threshold (in ticks) before entering low-power mode
    idle_threshold: u64,
}

impl GpuDevice {
    pub fn new_vesa(fb_addr: u64, mode: DisplayMode) -> Self {
        let mut vram = VramAllocator::new();
        // Assume 16 MB VRAM for VESA fallback
        vram.init(16 * 1024 * 1024);

        let connector = DisplayConnector {
            id: 0,
            connector_type: ConnectorType::VGA,
            status: ConnectorStatus::Connected,
            name: String::from("VGA-1"),
            modes: alloc::vec![
                MODE_640X480,
                MODE_800X600,
                MODE_1024X768,
                MODE_1280X720,
                MODE_1920X1080,
            ],
            active_mode: Some(mode),
            phys_width_mm: 0,
            phys_height_mm: 0,
        };

        GpuDevice {
            gpu_type: GpuType::VesaFb,
            vendor_id: 0,
            device_id: 0,
            vendor_name: String::from("Generic"),
            model_name: String::from("VESA VBE Framebuffer"),
            vram_size: 16 * 1024 * 1024,
            current_mode: mode,
            available_modes: alloc::vec![
                MODE_640X480,
                MODE_800X600,
                MODE_1024X768,
                MODE_1280X720,
                MODE_1920X1080,
            ],
            framebuffer_addr: fb_addr,
            framebuffer_size: mode.framebuffer_size(),
            cursor: HwCursor {
                x: 0,
                y: 0,
                width: 16,
                height: 16,
                hotspot_x: 0,
                hotspot_y: 0,
                pixels: Vec::new(),
                visible: true,
                vram_offset: 0,
            },
            vsync_enabled: true,
            connected_displays: 1,
            command_queue: Vec::new(),
            connectors: alloc::vec![connector],
            vram,
            cmd_ring: CommandRing::new(),
            vsync: VsyncState::new(),
            power_state: GpuPowerState::Active,
            clock: ClockState {
                core_mhz: 0,
                mem_mhz: 0,
                power_level: 0,
            },
            idle_ticks: 0,
            idle_threshold: 5000, // 5 seconds at 1kHz
        }
    }

    // -----------------------------------------------------------------------
    // Mode setting
    // -----------------------------------------------------------------------

    /// Set display mode
    pub fn set_mode(&mut self, mode: DisplayMode) -> Result<(), &'static str> {
        if !self.available_modes.contains(&mode) {
            return Err("unsupported mode");
        }

        self.current_mode = mode;
        self.framebuffer_size = mode.framebuffer_size();

        // Update the active connector's mode
        if let Some(conn) = self.connectors.first_mut() {
            conn.active_mode = Some(mode);
        }

        // Notify mouse driver of new screen dimensions
        crate::drivers::mouse::set_screen_size(mode.width as i32, mode.height as i32);

        serial_println!(
            "    [gpu] Mode set: {}x{}@{}Hz {}bpp",
            mode.width,
            mode.height,
            mode.refresh_hz,
            mode.bpp
        );
        Ok(())
    }

    /// Get current mode
    pub fn mode(&self) -> DisplayMode {
        self.current_mode
    }

    // -----------------------------------------------------------------------
    // Command submission
    // -----------------------------------------------------------------------

    /// Submit a 2D command via the ring buffer
    pub fn submit(&mut self, cmd: GpuCommand) {
        // Reset idle counter on any command
        self.idle_ticks = 0;
        if self.power_state == GpuPowerState::LowPower {
            self.power_state = GpuPowerState::Active;
            serial_println!("    [gpu] Waking from low-power state");
        }

        match &cmd {
            GpuCommand::FillRect {
                x,
                y,
                width,
                height,
                color,
            } => {
                if self.gpu_type == GpuType::VesaFb {
                    self.sw_fill_rect(*x, *y, *width, *height, *color);
                    return;
                }
            }
            GpuCommand::Blit {
                src_x,
                src_y,
                dst_x,
                dst_y,
                width,
                height,
            } => {
                if self.gpu_type == GpuType::VesaFb {
                    self.sw_blit(*src_x, *src_y, *dst_x, *dst_y, *width, *height);
                    return;
                }
            }
            GpuCommand::AlphaBlend {
                src_surface,
                src_x,
                src_y,
                dst_x,
                dst_y,
                width,
                height,
            } => {
                if self.gpu_type == GpuType::VesaFb {
                    self.sw_alpha_blend(
                        *src_surface,
                        *src_x,
                        *src_y,
                        *dst_x,
                        *dst_y,
                        *width,
                        *height,
                    );
                    return;
                }
            }
            GpuCommand::MoveCursor { x, y } => {
                self.cursor.x = *x;
                self.cursor.y = *y;
                return;
            }
            GpuCommand::Fence { id } => {
                self.cmd_ring.last_fence = *id;
                return;
            }
            _ => {}
        }

        // For hardware-accelerated GPUs, push to ring buffer
        if !self.cmd_ring.push(cmd.clone()) {
            // Ring full -- flush first
            self.flush();
            let _ = self.cmd_ring.push(cmd);
        }
    }

    /// Software fill rectangle (for VESA fallback)
    fn sw_fill_rect(&self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        let pitch = self.current_mode.pitch as u64;
        let bpp = self.current_mode.bpp as u64 / 8;
        let max_w = self.current_mode.width;
        let max_h = self.current_mode.height;

        for row in y..y.saturating_add(h).min(max_h) {
            for col in x..x.saturating_add(w).min(max_w) {
                let offset = (row as u64)
                    .saturating_mul(pitch)
                    .saturating_add((col as u64).saturating_mul(bpp));
                unsafe {
                    core::ptr::write_volatile(
                        self.framebuffer_addr.saturating_add(offset) as *mut u32,
                        color,
                    );
                }
            }
        }
    }

    /// Software blit (copy rectangle)
    fn sw_blit(&self, sx: u32, sy: u32, dx: u32, dy: u32, w: u32, h: u32) {
        let pitch = self.current_mode.pitch as u64;
        let bpp = self.current_mode.bpp as u64 / 8;

        // Handle overlapping regions by choosing copy direction
        let reverse_y = dy > sy && dy < sy.saturating_add(h);
        let reverse_x = dx > sx && dx < sx.saturating_add(w);

        let rows: Vec<u32> = if reverse_y {
            (0..h).rev().collect()
        } else {
            (0..h).collect()
        };

        let cols: Vec<u32> = if reverse_x {
            (0..w).rev().collect()
        } else {
            (0..w).collect()
        };

        for &row in &rows {
            for &col in &cols {
                let src_off = (sy.saturating_add(row) as u64)
                    .saturating_mul(pitch)
                    .saturating_add((sx.saturating_add(col) as u64).saturating_mul(bpp));
                let dst_off = (dy.saturating_add(row) as u64)
                    .saturating_mul(pitch)
                    .saturating_add((dx.saturating_add(col) as u64).saturating_mul(bpp));
                unsafe {
                    let pixel = core::ptr::read_volatile(
                        self.framebuffer_addr.saturating_add(src_off) as *const u32,
                    );
                    core::ptr::write_volatile(
                        self.framebuffer_addr.saturating_add(dst_off) as *mut u32,
                        pixel,
                    );
                }
            }
        }
    }

    /// Software alpha blend from a VRAM surface onto the framebuffer.
    /// Uses integer-only math: alpha * src + (255 - alpha) * dst, all divided by 255.
    fn sw_alpha_blend(
        &self,
        _src_surface: u32,
        _src_x: u32,
        _src_y: u32,
        _dst_x: u32,
        _dst_y: u32,
        _w: u32,
        _h: u32,
    ) {
        // Look up the source surface in VRAM
        let src_alloc = match self.vram.get(_src_surface) {
            Some(a) => a.clone(),
            None => return,
        };

        let pitch = self.current_mode.pitch as u64;
        let bpp = self.current_mode.bpp as u64 / 8;
        let max_w = self.current_mode.width;
        let max_h = self.current_mode.height;

        for row in 0.._h {
            let dst_y = _dst_y.saturating_add(row);
            if dst_y >= max_h {
                break;
            }
            let src_row = _src_y.saturating_add(row);
            if src_row >= src_alloc.height {
                break;
            }

            for col in 0.._w {
                let dst_x = _dst_x.saturating_add(col);
                if dst_x >= max_w {
                    break;
                }
                let src_col = _src_x.saturating_add(col);
                if src_col >= src_alloc.width {
                    break;
                }

                // Read source pixel from VRAM surface
                let src_pixel_off = src_alloc.offset.saturating_add(
                    (src_row as u64)
                        .saturating_mul(src_alloc.pitch as u64)
                        .saturating_add((src_col as u64).saturating_mul(src_alloc.bpp as u64 / 8)),
                );
                let src_pixel = unsafe {
                    core::ptr::read_volatile(
                        self.framebuffer_addr.saturating_add(src_pixel_off) as *const u32
                    )
                };

                // Read destination pixel from framebuffer
                let dst_off = (dst_y as u64)
                    .saturating_mul(pitch)
                    .saturating_add((dst_x as u64).saturating_mul(bpp));
                let dst_pixel = unsafe {
                    core::ptr::read_volatile(
                        self.framebuffer_addr.saturating_add(dst_off) as *const u32
                    )
                };

                // Alpha blend: ARGB format
                let sa = (src_pixel >> 24) & 0xFF;
                let sr = (src_pixel >> 16) & 0xFF;
                let sg = (src_pixel >> 8) & 0xFF;
                let sb = src_pixel & 0xFF;
                let da = 255u32.saturating_sub(sa);
                let dr = (dst_pixel >> 16) & 0xFF;
                let dg = (dst_pixel >> 8) & 0xFF;
                let db = dst_pixel & 0xFF;

                let r = (sr * sa + dr * da) / 255;
                let g = (sg * sa + dg * da) / 255;
                let b = (sb * sa + db * da) / 255;
                let result = 0xFF000000 | (r << 16) | (g << 8) | b;

                unsafe {
                    core::ptr::write_volatile(
                        self.framebuffer_addr.saturating_add(dst_off) as *mut u32,
                        result,
                    );
                }
            }
        }
    }

    /// Flush the command queue -- execute all pending commands
    pub fn flush(&mut self) {
        // Execute all pending commands in the ring
        while let Some(cmd) = self.cmd_ring.pop() {
            match cmd {
                GpuCommand::FillRect {
                    x,
                    y,
                    width,
                    height,
                    color,
                } => {
                    self.sw_fill_rect(x, y, width, height, color);
                }
                GpuCommand::Blit {
                    src_x,
                    src_y,
                    dst_x,
                    dst_y,
                    width,
                    height,
                } => {
                    self.sw_blit(src_x, src_y, dst_x, dst_y, width, height);
                }
                GpuCommand::TransferToFb {
                    ref buffer,
                    dst_x,
                    dst_y,
                    width,
                    height,
                    stride,
                } => {
                    self.sw_transfer_to_fb(buffer, dst_x, dst_y, width, height, stride);
                }
                GpuCommand::AlphaBlend {
                    src_surface,
                    src_x,
                    src_y,
                    dst_x,
                    dst_y,
                    width,
                    height,
                } => {
                    self.sw_alpha_blend(src_surface, src_x, src_y, dst_x, dst_y, width, height);
                }
                GpuCommand::MoveCursor { x, y } => {
                    self.cursor.x = x;
                    self.cursor.y = y;
                }
                GpuCommand::SetCursorImage {
                    surface_id,
                    hotspot_x,
                    hotspot_y,
                } => {
                    // Load cursor pixels from the VRAM surface
                    if let Some(alloc) = self.vram.get(surface_id) {
                        self.cursor.width = alloc.width;
                        self.cursor.height = alloc.height;
                        self.cursor.hotspot_x = hotspot_x;
                        self.cursor.hotspot_y = hotspot_y;
                        self.cursor.vram_offset = alloc.offset;
                    }
                }
                GpuCommand::PageFlip => {
                    if self.vsync.enabled {
                        self.wait_vblank();
                    }
                    self.vsync.frame_count = self.vsync.frame_count.saturating_add(1);
                }
                GpuCommand::Fence { id } => {
                    self.cmd_ring.last_fence = id;
                }
            }
        }
        // Also clear legacy queue
        self.command_queue.clear();
    }

    /// Software transfer: copy a pixel buffer to the framebuffer at (dst_x, dst_y)
    fn sw_transfer_to_fb(
        &self,
        buffer: &[u8],
        dst_x: u32,
        dst_y: u32,
        width: u32,
        height: u32,
        stride: u32,
    ) {
        let pitch = self.current_mode.pitch as u64;
        let bpp = self.current_mode.bpp as u64 / 8;
        let max_w = self.current_mode.width;
        let max_h = self.current_mode.height;

        for row in 0..height {
            let dy = dst_y.saturating_add(row);
            if dy >= max_h {
                break;
            }
            for col in 0..width {
                let dx = dst_x.saturating_add(col);
                if dx >= max_w {
                    break;
                }
                // Read pixel from source buffer (assumed 32-bit ARGB)
                let src_off = (row as usize)
                    .saturating_mul(stride as usize)
                    .saturating_add((col as usize).saturating_mul(bpp as usize));
                if src_off + 4 > buffer.len() {
                    break;
                }
                let pixel = u32::from_ne_bytes([
                    buffer[src_off],
                    buffer[src_off + 1],
                    buffer[src_off + 2],
                    buffer[src_off + 3],
                ]);
                let dst_off = (dy as u64)
                    .saturating_mul(pitch)
                    .saturating_add((dx as u64).saturating_mul(bpp));
                unsafe {
                    core::ptr::write_volatile(
                        self.framebuffer_addr.saturating_add(dst_off) as *mut u32,
                        pixel,
                    );
                }
            }
        }
    }

    /// Busy-wait for vertical blank period (VESA/VGA only)
    fn wait_vblank(&self) {
        // VGA Input Status Register 1: port 0x3DA
        // Bit 3: Vertical Retrace -- 1 during vblank

        // Wait for any current vblank to end
        while crate::io::inb(0x3DA) & 0x08 != 0 {
            core::hint::spin_loop();
        }
        // Wait for vblank to start
        while crate::io::inb(0x3DA) & 0x08 == 0 {
            core::hint::spin_loop();
        }
    }

    // -----------------------------------------------------------------------
    // VSync
    // -----------------------------------------------------------------------

    /// Handle a vblank interrupt (called from the interrupt handler)
    pub fn vblank_interrupt(&mut self) {
        self.vsync.frame_count = self.vsync.frame_count.saturating_add(1);
        self.vsync.in_vblank = true;
    }

    /// Get the current frame count
    pub fn frame_count(&self) -> u64 {
        self.vsync.frame_count
    }

    /// Enable/disable vsync
    pub fn set_vsync(&mut self, enabled: bool) {
        self.vsync.enabled = enabled;
        self.vsync_enabled = enabled;
        serial_println!(
            "    [gpu] VSync {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    // -----------------------------------------------------------------------
    // VRAM management
    // -----------------------------------------------------------------------

    /// Allocate a surface in VRAM
    pub fn alloc_surface(&mut self, width: u32, height: u32, bpp: u8, name: &str) -> Option<u32> {
        self.vram.alloc(width, height, bpp, name)
    }

    /// Free a VRAM surface
    pub fn free_surface(&mut self, id: u32) -> bool {
        self.vram.free(id)
    }

    /// Get VRAM usage: (used, total, allocation_count)
    pub fn vram_stats(&self) -> (u64, u64, usize) {
        self.vram.stats()
    }

    /// Compact (defragment) VRAM allocations
    pub fn compact_vram(&mut self) {
        self.vram.compact();
    }

    // -----------------------------------------------------------------------
    // Cursor plane
    // -----------------------------------------------------------------------

    /// Set hardware cursor image
    pub fn set_cursor_image(
        &mut self,
        width: u32,
        height: u32,
        hotspot_x: u32,
        hotspot_y: u32,
        pixels: Vec<u32>,
    ) {
        self.cursor.width = width;
        self.cursor.height = height;
        self.cursor.hotspot_x = hotspot_x;
        self.cursor.hotspot_y = hotspot_y;
        self.cursor.pixels = pixels;
    }

    /// Move hardware cursor
    pub fn move_cursor(&mut self, x: i32, y: i32) {
        self.cursor.x = x;
        self.cursor.y = y;
    }

    /// Show/hide hardware cursor
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor.visible = visible;
    }

    /// Get cursor position
    pub fn cursor_position(&self) -> (i32, i32) {
        (self.cursor.x, self.cursor.y)
    }

    // -----------------------------------------------------------------------
    // Display connector management
    // -----------------------------------------------------------------------

    /// Get the list of display connectors
    pub fn connectors(&self) -> &[DisplayConnector] {
        &self.connectors
    }

    /// Get number of connected displays
    pub fn connected_display_count(&self) -> u8 {
        self.connectors
            .iter()
            .filter(|c| c.status == ConnectorStatus::Connected)
            .count() as u8
    }

    /// Detect connector status changes (hotplug)
    pub fn detect_hotplug(&mut self) {
        // In a real driver, we'd read HPD registers from the GPU
        // For VESA fallback, just assume a single VGA is always connected
        if self.gpu_type == GpuType::VesaFb {
            if let Some(conn) = self.connectors.first_mut() {
                conn.status = ConnectorStatus::Connected;
            }
        }
        self.connected_displays = self.connected_display_count();
    }

    // -----------------------------------------------------------------------
    // Power management
    // -----------------------------------------------------------------------

    /// Get current GPU power state
    pub fn power_state(&self) -> GpuPowerState {
        self.power_state
    }

    /// Tick the idle counter. Call from timer ISR.
    pub fn idle_tick(&mut self) {
        if self.cmd_ring.pending() == 0 {
            self.idle_ticks = self.idle_ticks.saturating_add(1);
            if self.idle_ticks >= self.idle_threshold && self.power_state == GpuPowerState::Active {
                self.power_state = GpuPowerState::LowPower;
                self.clock.power_level = 0;
                serial_println!(
                    "    [gpu] Entering low-power state (idle {} ticks)",
                    self.idle_ticks
                );
            }
        } else {
            self.idle_ticks = 0;
        }
    }

    /// Suspend the GPU.
    ///
    /// Sequence:
    ///   1. Guard: skip if already suspended.
    ///   2. Flush the command ring — drain any pending 2D/3D commands so the
    ///      GPU is idle before power is cut.  For the VESA framebuffer there
    ///      is no hardware ring; for a real GPU this would wait for the fence
    ///      to be signalled.
    ///   3. Blank the framebuffer (zero all pixels) so the screen goes dark
    ///      before the display controller is powered off.
    ///   4. Stop the display controller scanout (DPMS off).
    ///   5. Gate GPU core clocks and set clock frequency to zero.
    ///   6. Transition PCI function to D3hot (write 0x03 to PM CSR).
    ///      TODO: read the real PCI PM cap pointer from config space and
    ///            write the D3hot value via crate::drivers::pci.
    pub fn suspend(&mut self) {
        if self.power_state == GpuPowerState::Suspended {
            return;
        }

        // Step 2: flush the command ring.
        self.flush();

        // Step 3: blank the framebuffer (volatile writes, no compiler skip).
        if self.framebuffer_addr != 0 && self.framebuffer_size > 0 {
            let fb_ptr = self.framebuffer_addr as *mut u32;
            let word_count = self.framebuffer_size / 4;
            for i in 0..word_count {
                // Safety: framebuffer_addr is a validated VESA/BAR MMIO mapping.
                unsafe {
                    core::ptr::write_volatile(fb_ptr.add(i), 0x0000_0000);
                }
            }
        }

        // Step 4: stop scanout — write DPMS_OFF to the display controller.
        // DPMS control register sits at GPU MMIO base + 0x8010.
        if self.framebuffer_addr != 0 {
            let gpu_mmio = self.framebuffer_addr;
            unsafe {
                core::ptr::write_volatile(
                    gpu_mmio.saturating_add(DPMS_REG_OFFSET) as *mut u32,
                    DPMS_OFF,
                );
            }
        }

        // Step 5: gate clocks.
        self.clock.core_mhz = 0;
        self.clock.mem_mhz = 0;
        self.clock.power_level = 0;

        // Step 6: PCI D3hot — write D-state 3 to PMCSR via PCI config space.
        // Find the GPU on the PCI bus (class 0x03 = display, any subclass) and
        // use the existing set_power_state helper which walks the PM capability.
        {
            let gpu_devs = crate::drivers::pci::find_by_class(0x03, 0x00);
            if let Some(ref gdev) = gpu_devs.first() {
                crate::drivers::pci::set_power_state(gdev.bus, gdev.device, gdev.function, 3);
            }
        }

        self.power_state = GpuPowerState::Suspended;
        serial_println!("    [gpu] suspended (D3hot)");
    }

    /// Resume the GPU from suspend.
    ///
    /// Sequence:
    ///   1. Guard: skip if not suspended.
    ///   2. Restore PCI function to D0 (active): write 0x00 to PM CSR.
    ///      TODO: crate::drivers::pci::set_power_state(bus, dev, func, 0);
    ///   3. Wait for PCI Tpdrh power-on delay (~10 ms).
    ///   4. Restore GPU clocks to their pre-suspend operating point.
    ///   5. Re-enable display controller scanout (DPMS on).
    ///   6. Reset idle tick counter.
    pub fn resume(&mut self) {
        if self.power_state != GpuPowerState::Suspended {
            return;
        }

        // Step 2: PCI D0 — restore power state via PMCSR.
        {
            let gpu_devs = crate::drivers::pci::find_by_class(0x03, 0x00);
            if let Some(ref gdev) = gpu_devs.first() {
                crate::drivers::pci::set_power_state(gdev.bus, gdev.device, gdev.function, 0);
            }
        }

        // Step 3: PCI Tpdrh delay.  Spin for ~10 ms worth of pauses.
        for _ in 0..100_000u32 {
            unsafe {
                core::arch::asm!("pause", options(nomem, nostack));
            }
        }

        // Step 4: restore clocks to the active operating point.
        // For VESA we restore the clock fields to reasonable defaults; a real
        // driver would re-program the PLL and memory controller frequency.
        self.clock.core_mhz = 300;
        self.clock.mem_mhz = 400;
        self.clock.power_level = 1;

        // Step 5: re-enable scanout — write DPMS_ON to the display controller.
        // DPMS control register sits at GPU MMIO base + 0x8010.
        if self.framebuffer_addr != 0 {
            let gpu_mmio = self.framebuffer_addr;
            unsafe {
                core::ptr::write_volatile(
                    gpu_mmio.saturating_add(DPMS_REG_OFFSET) as *mut u32,
                    DPMS_ON,
                );
            }
        }

        self.idle_ticks = 0;
        self.power_state = GpuPowerState::Active;
        serial_println!("    [gpu] resumed (D0, scanout re-enabled)");
    }

    /// Get clock state
    pub fn clock_state(&self) -> ClockState {
        self.clock
    }

    // -----------------------------------------------------------------------
    // Info / debug
    // -----------------------------------------------------------------------

    /// Get a summary string for display
    pub fn info_string(&self) -> String {
        let (used, total, count) = self.vram.stats();
        alloc::format!(
            "{} {} (VRAM: {}/{}KB, {}surfaces, {}x{}@{}Hz, {:?})",
            self.vendor_name,
            self.model_name,
            used / 1024,
            total / 1024,
            count,
            self.current_mode.width,
            self.current_mode.height,
            self.current_mode.refresh_hz,
            self.power_state,
        )
    }
}

// ---------------------------------------------------------------------------
// Module-level API
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("    [gpu] GPU driver loaded (VESA fallback)");
}

/// Initialize with a specific VESA framebuffer
pub fn init_vesa(fb_addr: u64, mode: DisplayMode) {
    let dev = GpuDevice::new_vesa(fb_addr, mode);
    *GPU_STATE.lock() = Some(dev);
    serial_println!(
        "    [gpu] VESA GPU initialized at {:#x} {}x{}",
        fb_addr,
        mode.width,
        mode.height
    );
}

/// Submit a command to the GPU
pub fn submit_command(cmd: GpuCommand) {
    if let Some(ref mut gpu) = *GPU_STATE.lock() {
        gpu.submit(cmd);
    }
}

/// Flush pending GPU commands
pub fn flush() {
    if let Some(ref mut gpu) = *GPU_STATE.lock() {
        gpu.flush();
    }
}

/// Get current frame count
pub fn frame_count() -> u64 {
    GPU_STATE.lock().as_ref().map_or(0, |g| g.frame_count())
}

/// Allocate a surface in VRAM
pub fn alloc_surface(width: u32, height: u32, name: &str) -> Option<u32> {
    if let Some(ref mut gpu) = *GPU_STATE.lock() {
        gpu.alloc_surface(width, height, 32, name)
    } else {
        None
    }
}

/// Free a VRAM surface
pub fn free_surface(id: u32) -> bool {
    GPU_STATE
        .lock()
        .as_mut()
        .map_or(false, |g| g.free_surface(id))
}

/// Suspend the GPU: stop scanout, blank framebuffer, enter D3hot.
/// Safe to call even if no GPU is initialized (no-op in that case).
pub fn suspend() {
    if let Some(ref mut gpu) = *GPU_STATE.lock() {
        gpu.suspend();
    } else {
        serial_println!("  [gpu] suspend: no GPU initialized, skipping");
    }
}

/// Resume the GPU from D3hot: restore D0, re-enable scanout.
/// Safe to call even if no GPU is initialized (no-op in that case).
pub fn resume() {
    if let Some(ref mut gpu) = *GPU_STATE.lock() {
        gpu.resume();
    } else {
        serial_println!("  [gpu] resume: no GPU initialized, skipping");
    }
}
