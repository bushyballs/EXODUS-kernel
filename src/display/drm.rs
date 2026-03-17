/// DRM/KMS — Direct Rendering Manager / Kernel Mode Setting for Genesis
///
/// Manages display hardware: CRTCs, encoders, connectors, planes, framebuffers.
/// Provides mode setting (resolution, refresh rate) and framebuffer management.
///
/// Inspired by: Linux DRM/KMS (drivers/gpu/drm/). All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Display mode (resolution + timing)
#[derive(Debug, Clone)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
    pub pixel_clock: u32, // kHz
    pub hsync_start: u32,
    pub hsync_end: u32,
    pub htotal: u32,
    pub vsync_start: u32,
    pub vsync_end: u32,
    pub vtotal: u32,
    pub flags: u32,
    pub name: String,
}

impl DisplayMode {
    pub fn new(w: u32, h: u32, refresh: u32) -> Self {
        DisplayMode {
            width: w,
            height: h,
            refresh,
            pixel_clock: 0,
            hsync_start: 0,
            hsync_end: 0,
            htotal: 0,
            vsync_start: 0,
            vsync_end: 0,
            vtotal: 0,
            flags: 0,
            name: format!("{}x{}@{}", w, h, refresh),
        }
    }
}

/// Connector types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorType {
    VGA,
    DVII,
    DVID,
    DVIA,
    HDMIA,
    HDMIB,
    DisplayPort,
    EDP,
    Virtual,
    DSI,
    DPI,
    LVDS,
}

/// Connector status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorStatus {
    Connected,
    Disconnected,
    Unknown,
}

/// Pixel format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    ARGB8888,
    XRGB8888,
    RGB888,
    RGB565,
    ABGR8888,
    XBGR8888,
    NV12,
    NV21,
    YUV420,
}

/// A CRTC (display controller pipeline)
pub struct Crtc {
    pub id: u32,
    pub active: bool,
    pub mode: Option<DisplayMode>,
    pub fb_id: Option<u32>,
    pub gamma_size: u32,
    pub x: u32,
    pub y: u32,
}

/// An encoder (signal converter)
pub struct Encoder {
    pub id: u32,
    pub encoder_type: u32,
    pub crtc_id: Option<u32>,
    pub possible_crtcs: u32,
}

/// A connector (physical output port)
pub struct Connector {
    pub id: u32,
    pub connector_type: ConnectorType,
    pub status: ConnectorStatus,
    pub encoder_id: Option<u32>,
    pub modes: Vec<DisplayMode>,
    pub properties: Vec<(String, u64)>,
    pub edid: Vec<u8>,
}

/// A plane (overlay/primary/cursor)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneType {
    Primary,
    Overlay,
    Cursor,
}

pub struct Plane {
    pub id: u32,
    pub plane_type: PlaneType,
    pub crtc_id: Option<u32>,
    pub fb_id: Option<u32>,
    pub src_x: u32,
    pub src_y: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub dst_x: u32,
    pub dst_y: u32,
    pub dst_w: u32,
    pub dst_h: u32,
    pub formats: Vec<PixelFormat>,
}

/// A framebuffer object
pub struct Framebuffer {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub format: PixelFormat,
    pub handle: usize, // GEM/dumb buffer handle
}

/// DRM device
pub struct DrmDevice {
    crtcs: Vec<Crtc>,
    encoders: Vec<Encoder>,
    connectors: Vec<Connector>,
    planes: Vec<Plane>,
    framebuffers: Vec<Framebuffer>,
    next_id: u32,
}

impl DrmDevice {
    const fn new() -> Self {
        DrmDevice {
            crtcs: Vec::new(),
            encoders: Vec::new(),
            connectors: Vec::new(),
            planes: Vec::new(),
            framebuffers: Vec::new(),
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    /// Add a CRTC
    pub fn add_crtc(&mut self) -> u32 {
        let id = self.alloc_id();
        self.crtcs.push(Crtc {
            id,
            active: false,
            mode: None,
            fb_id: None,
            gamma_size: 256,
            x: 0,
            y: 0,
        });
        id
    }

    /// Add an encoder
    pub fn add_encoder(&mut self, possible_crtcs: u32) -> u32 {
        let id = self.alloc_id();
        self.encoders.push(Encoder {
            id,
            encoder_type: 0,
            crtc_id: None,
            possible_crtcs,
        });
        id
    }

    /// Add a connector
    pub fn add_connector(&mut self, conn_type: ConnectorType) -> u32 {
        let id = self.alloc_id();
        let mut modes = Vec::new();
        // Add common modes
        modes.push(DisplayMode::new(1920, 1080, 60));
        modes.push(DisplayMode::new(1280, 720, 60));
        modes.push(DisplayMode::new(1024, 768, 60));
        modes.push(DisplayMode::new(800, 600, 60));
        modes.push(DisplayMode::new(640, 480, 60));

        self.connectors.push(Connector {
            id,
            connector_type: conn_type,
            status: ConnectorStatus::Connected,
            encoder_id: None,
            modes,
            properties: Vec::new(),
            edid: Vec::new(),
        });
        id
    }

    /// Add a plane
    pub fn add_plane(&mut self, plane_type: PlaneType) -> u32 {
        let id = self.alloc_id();
        self.planes.push(Plane {
            id,
            plane_type,
            crtc_id: None,
            fb_id: None,
            src_x: 0,
            src_y: 0,
            src_w: 0,
            src_h: 0,
            dst_x: 0,
            dst_y: 0,
            dst_w: 0,
            dst_h: 0,
            formats: alloc::vec![PixelFormat::ARGB8888, PixelFormat::XRGB8888],
        });
        id
    }

    /// Create a framebuffer
    pub fn create_fb(&mut self, width: u32, height: u32, format: PixelFormat) -> Option<u32> {
        let id = self.alloc_id();
        let bpp = match format {
            PixelFormat::ARGB8888
            | PixelFormat::XRGB8888
            | PixelFormat::ABGR8888
            | PixelFormat::XBGR8888 => 4,
            PixelFormat::RGB888 => 3,
            PixelFormat::RGB565 => 2,
            _ => 4,
        };
        let pitch = width * bpp;
        let size = (pitch * height) as usize;

        let handle = match crate::memory::vmalloc::vmalloc(size) {
            Some(ptr) => ptr as usize,
            None => return None,
        };

        self.framebuffers.push(Framebuffer {
            id,
            width,
            height,
            pitch,
            format,
            handle,
        });
        Some(id)
    }

    /// Set mode on a CRTC
    pub fn set_mode(
        &mut self,
        crtc_id: u32,
        mode: &DisplayMode,
        fb_id: u32,
        connector_id: u32,
    ) -> bool {
        // Find and configure CRTC
        if let Some(crtc) = self.crtcs.iter_mut().find(|c| c.id == crtc_id) {
            crtc.mode = Some(mode.clone());
            crtc.fb_id = Some(fb_id);
            crtc.active = true;
        } else {
            return false;
        }

        // Connect encoder to connector
        if let Some(conn) = self.connectors.iter_mut().find(|c| c.id == connector_id) {
            if let Some(enc) = self.encoders.first_mut() {
                enc.crtc_id = Some(crtc_id);
                conn.encoder_id = Some(enc.id);
            }
        }
        true
    }

    /// Page flip (swap framebuffer on next vsync)
    pub fn page_flip(&mut self, crtc_id: u32, fb_id: u32) -> bool {
        if let Some(crtc) = self.crtcs.iter_mut().find(|c| c.id == crtc_id) {
            crtc.fb_id = Some(fb_id);
            true
        } else {
            false
        }
    }

    /// Get current mode info
    pub fn get_mode(&self, crtc_id: u32) -> Option<&DisplayMode> {
        self.crtcs
            .iter()
            .find(|c| c.id == crtc_id)
            .and_then(|c| c.mode.as_ref())
    }

    /// List connectors
    pub fn list_connectors(&self) -> Vec<(u32, ConnectorType, ConnectorStatus)> {
        self.connectors
            .iter()
            .map(|c| (c.id, c.connector_type, c.status))
            .collect()
    }
}

static DRM: Mutex<DrmDevice> = Mutex::new(DrmDevice::new());

pub fn init() {
    let mut drm = DRM.lock();

    // Create a default display pipeline
    let crtc_id = drm.add_crtc();
    drm.add_encoder(1);
    drm.add_connector(ConnectorType::Virtual);
    drm.add_plane(PlaneType::Primary);
    drm.add_plane(PlaneType::Cursor);

    crate::serial_println!(
        "  [drm] DRM/KMS initialized (CRTC {}, virtual connector)",
        crtc_id
    );
}

pub fn create_fb(w: u32, h: u32, fmt: PixelFormat) -> Option<u32> {
    DRM.lock().create_fb(w, h, fmt)
}
pub fn set_mode(crtc: u32, mode: &DisplayMode, fb: u32, conn: u32) -> bool {
    DRM.lock().set_mode(crtc, mode, fb, conn)
}
pub fn page_flip(crtc: u32, fb: u32) -> bool {
    DRM.lock().page_flip(crtc, fb)
}

// ---------------------------------------------------------------------------
// EDID parser — extracts monitor information from 128-byte EDID blocks
// ---------------------------------------------------------------------------

/// Parsed EDID information
#[derive(Debug, Clone)]
pub struct EdidInfo {
    /// Manufacturer ID (3-letter PNP code)
    pub manufacturer: [u8; 3],
    /// Product code
    pub product_code: u16,
    /// Serial number (from EDID header)
    pub serial_number: u32,
    /// Manufacture week
    pub mfg_week: u8,
    /// Manufacture year (offset from 1990)
    pub mfg_year: u16,
    /// EDID version
    pub version: u8,
    /// EDID revision
    pub revision: u8,
    /// Max horizontal image size in cm
    pub max_h_cm: u8,
    /// Max vertical image size in cm
    pub max_v_cm: u8,
    /// Preferred mode width
    pub preferred_width: u32,
    /// Preferred mode height
    pub preferred_height: u32,
    /// Preferred refresh rate
    pub preferred_refresh: u32,
    /// Monitor name (from descriptor block)
    pub monitor_name: String,
    /// Supported standard timings
    pub standard_timings: Vec<(u32, u32, u32)>, // (width, height, refresh)
}

impl EdidInfo {
    /// Parse a 128-byte EDID block
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 128 {
            return None;
        }

        // Verify EDID header: 00 FF FF FF FF FF FF 00
        let header: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
        if data[0..8] != header {
            return None;
        }

        // Verify checksum
        let mut sum: u8 = 0;
        for i in 0..128 {
            sum = sum.wrapping_add(data[i]);
        }
        if sum != 0 {
            return None;
        }

        // Manufacturer ID (bytes 8-9): 3 letters packed in 16 bits
        let mfg_raw = ((data[8] as u16) << 8) | data[9] as u16;
        let mfg_a = ((mfg_raw >> 10) & 0x1F) as u8 + b'A' - 1;
        let mfg_b = ((mfg_raw >> 5) & 0x1F) as u8 + b'A' - 1;
        let mfg_c = (mfg_raw & 0x1F) as u8 + b'A' - 1;

        let product_code = (data[10] as u16) | ((data[11] as u16) << 8);
        let serial_number = (data[12] as u32)
            | ((data[13] as u32) << 8)
            | ((data[14] as u32) << 16)
            | ((data[15] as u32) << 24);

        let mfg_week = data[16];
        let mfg_year = data[17] as u16 + 1990;

        let version = data[18];
        let revision = data[19];

        let max_h_cm = data[21];
        let max_v_cm = data[22];

        // Parse preferred timing from Detailed Timing Descriptor (DTD) at byte 54
        let (preferred_width, preferred_height, preferred_refresh) = Self::parse_dtd(&data[54..72]);

        // Parse monitor name from descriptor blocks (bytes 54-125, 4 blocks of 18 bytes)
        let mut monitor_name = String::new();
        for block_start in (54..126).step_by(18) {
            if block_start + 17 >= data.len() {
                break;
            }
            // Monitor name descriptor: tag = 0x000000FC
            if data[block_start] == 0
                && data[block_start + 1] == 0
                && data[block_start + 2] == 0
                && data[block_start + 3] == 0xFC
            {
                // Name is in bytes 5..17 of the descriptor, padded with 0x0A/0x20
                for i in 5..18 {
                    let ch = data[block_start + i];
                    if ch == 0x0A || ch == 0x00 {
                        break;
                    }
                    if ch >= 0x20 && ch < 0x7F {
                        monitor_name.push(ch as char);
                    }
                }
            }
        }

        // Parse standard timings (bytes 38-53, 8 entries of 2 bytes each)
        let mut standard_timings = Vec::new();
        for i in 0..8 {
            let offset = 38 + i * 2;
            let b0 = data[offset];
            let b1 = data[offset + 1];
            if b0 == 0x01 && b1 == 0x01 {
                continue;
            } // Unused entry
            let width = (b0 as u32 + 31) * 8;
            let aspect = (b1 >> 6) & 0x03;
            let height = match aspect {
                0 => width * 10 / 16, // 16:10
                1 => width * 3 / 4,   // 4:3
                2 => width * 4 / 5,   // 5:4
                3 => width * 9 / 16,  // 16:9
                _ => width * 3 / 4,
            };
            let refresh = (b1 & 0x3F) as u32 + 60;
            standard_timings.push((width, height, refresh));
        }

        Some(EdidInfo {
            manufacturer: [mfg_a, mfg_b, mfg_c],
            product_code,
            serial_number,
            mfg_week,
            mfg_year,
            version,
            revision,
            max_h_cm,
            max_v_cm,
            preferred_width,
            preferred_height,
            preferred_refresh,
            monitor_name,
            standard_timings,
        })
    }

    /// Parse a Detailed Timing Descriptor (18 bytes) for resolution and refresh
    fn parse_dtd(dtd: &[u8]) -> (u32, u32, u32) {
        if dtd.len() < 18 || (dtd[0] == 0 && dtd[1] == 0) {
            return (0, 0, 0);
        }

        let pixel_clock_khz = ((dtd[0] as u32) | ((dtd[1] as u32) << 8)) * 10;
        if pixel_clock_khz == 0 {
            return (0, 0, 0);
        }

        let h_active = (dtd[2] as u32) | (((dtd[4] >> 4) as u32) << 8);
        let h_blank = (dtd[3] as u32) | (((dtd[4] & 0x0F) as u32) << 8);
        let v_active = (dtd[5] as u32) | (((dtd[7] >> 4) as u32) << 8);
        let v_blank = (dtd[6] as u32) | (((dtd[7] & 0x0F) as u32) << 8);

        let h_total = h_active + h_blank;
        let v_total = v_active + v_blank;

        let refresh = if h_total > 0 && v_total > 0 {
            (pixel_clock_khz * 1000) / (h_total * v_total)
        } else {
            60
        };

        (h_active, v_active, refresh)
    }

    /// Get manufacturer as a string
    pub fn manufacturer_string(&self) -> String {
        let mut s = String::with_capacity(3);
        s.push(self.manufacturer[0] as char);
        s.push(self.manufacturer[1] as char);
        s.push(self.manufacturer[2] as char);
        s
    }

    /// Diagonal size in inches (approximate, from cm dimensions, integer)
    pub fn diagonal_inches(&self) -> u32 {
        if self.max_h_cm == 0 || self.max_v_cm == 0 {
            return 0;
        }
        let h = self.max_h_cm as u32;
        let v = self.max_v_cm as u32;
        // sqrt(h^2 + v^2) / 2.54, approximate with integer sqrt
        let sq = h * h + v * v;
        let diag_cm = isqrt(sq);
        // Convert cm to inches: diag * 100 / 254
        (diag_cm * 100 + 127) / 254
    }
}

/// Integer square root
fn isqrt(val: u32) -> u32 {
    if val == 0 {
        return 0;
    }
    let mut x = val;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + val / x) / 2;
    }
    x
}

/// Parse EDID data from a connector
pub fn parse_edid(connector_id: u32) -> Option<EdidInfo> {
    let drm = DRM.lock();
    let conn = drm.connectors.iter().find(|c| c.id == connector_id)?;
    if conn.edid.is_empty() {
        return None;
    }
    EdidInfo::parse(&conn.edid)
}

/// List all connectors with their status
pub fn list_connectors() -> Vec<(u32, ConnectorType, ConnectorStatus)> {
    DRM.lock().list_connectors()
}
