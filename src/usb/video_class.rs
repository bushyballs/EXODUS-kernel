use crate::sync::Mutex;
/// USB Video Class (UVC) driver
///
/// Supports USB webcams, capture devices, and video streaming endpoints.
/// Handles frame format negotiation, resolution selection, isochronous
/// streaming, and camera controls (brightness, contrast, exposure, etc.).
///
/// References: USB Video Class 1.1 / 1.5 specifications.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static VIDEO_STATE: Mutex<Option<VideoClassState>> = Mutex::new(None);

pub struct VideoClassState {
    pub devices: Vec<VideoDevice>,
    pub next_device_id: u32,
}

impl VideoClassState {
    pub fn new() -> Self {
        VideoClassState {
            devices: Vec::new(),
            next_device_id: 1,
        }
    }

    pub fn register(&mut self, dev: VideoDevice) -> u32 {
        let id = self.next_device_id;
        self.next_device_id = self.next_device_id.saturating_add(1);
        self.devices.push(dev);
        id
    }
}

// ---------------------------------------------------------------------------
// UVC constants
// ---------------------------------------------------------------------------

pub const CLASS_VIDEO: u8 = 0x0E;

/// Video subclass codes.
pub const SC_VIDEOCONTROL: u8 = 0x01;
pub const SC_VIDEOSTREAMING: u8 = 0x02;
pub const SC_VIDEO_INTERFACE_COLLECTION: u8 = 0x03;

/// Video class-specific descriptor types.
pub const CS_INTERFACE: u8 = 0x24;
pub const CS_ENDPOINT: u8 = 0x25;

/// Video Control interface descriptor subtypes.
pub const VC_HEADER: u8 = 0x01;
pub const VC_INPUT_TERMINAL: u8 = 0x02;
pub const VC_OUTPUT_TERMINAL: u8 = 0x03;
pub const VC_SELECTOR_UNIT: u8 = 0x04;
pub const VC_PROCESSING_UNIT: u8 = 0x05;
pub const VC_EXTENSION_UNIT: u8 = 0x06;

/// Video Streaming interface descriptor subtypes.
pub const VS_INPUT_HEADER: u8 = 0x01;
pub const VS_OUTPUT_HEADER: u8 = 0x02;
pub const VS_FORMAT_UNCOMPRESSED: u8 = 0x04;
pub const VS_FRAME_UNCOMPRESSED: u8 = 0x05;
pub const VS_FORMAT_MJPEG: u8 = 0x06;
pub const VS_FRAME_MJPEG: u8 = 0x07;
pub const VS_FORMAT_FRAME_BASED: u8 = 0x10;
pub const VS_FRAME_FRAME_BASED: u8 = 0x11;

/// UVC request codes.
pub const UVC_SET_CUR: u8 = 0x01;
pub const UVC_GET_CUR: u8 = 0x81;
pub const UVC_GET_MIN: u8 = 0x82;
pub const UVC_GET_MAX: u8 = 0x83;
pub const UVC_GET_RES: u8 = 0x84;
pub const UVC_GET_DEF: u8 = 0x87;

/// Camera Terminal control selectors.
pub const CT_EXPOSURE_TIME_ABS: u8 = 0x04;
pub const CT_FOCUS_ABS: u8 = 0x06;
pub const CT_ZOOM_ABS: u8 = 0x09;
pub const CT_PANTILT_ABS: u8 = 0x0D;

/// Processing Unit control selectors.
pub const PU_BRIGHTNESS: u8 = 0x02;
pub const PU_CONTRAST: u8 = 0x03;
pub const PU_SATURATION: u8 = 0x07;
pub const PU_SHARPNESS: u8 = 0x08;
pub const PU_WHITE_BALANCE_TEMP: u8 = 0x0A;
pub const PU_GAIN: u8 = 0x0B;

/// Video streaming control selectors.
pub const VS_PROBE_CONTROL: u16 = 0x0100;
pub const VS_COMMIT_CONTROL: u16 = 0x0200;

// ---------------------------------------------------------------------------
// Frame formats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Yuy2,
    Nv12,
    Mjpeg,
    H264,
    Uncompressed,
    Unknown(u8),
}

impl PixelFormat {
    pub fn bits_per_pixel(&self) -> u32 {
        match self {
            PixelFormat::Yuy2 => 16,
            PixelFormat::Nv12 => 12,
            PixelFormat::Mjpeg => 16, // approximate for bandwidth calc
            PixelFormat::H264 => 12,
            PixelFormat::Uncompressed => 24,
            PixelFormat::Unknown(_) => 16,
        }
    }
}

/// A single frame size descriptor.
#[derive(Debug, Clone)]
pub struct FrameDescriptor {
    pub width: u16,
    pub height: u16,
    pub min_frame_interval: u32, // 100 ns units
    pub max_frame_interval: u32,
    pub default_frame_interval: u32,
    pub frame_index: u8,
}

impl FrameDescriptor {
    /// Maximum frames per second (Q16 fixed-point).
    pub fn max_fps_q16(&self) -> i32 {
        if self.min_frame_interval == 0 {
            return 30 << 16;
        }
        // fps = 10_000_000 / interval
        // Q16: (10_000_000 << 16) / interval
        // Avoid overflow: divide first
        let fps_int = 10_000_000_u32 / self.min_frame_interval;
        (fps_int as i32) << 16
    }

    /// Total pixels.
    pub fn pixel_count(&self) -> u32 {
        self.width as u32 * self.height as u32
    }
}

/// A video format with its associated frame descriptors.
#[derive(Debug, Clone)]
pub struct VideoFormatDescriptor {
    pub format_index: u8,
    pub pixel_format: PixelFormat,
    pub guid: [u8; 16],
    pub bits_per_pixel: u8,
    pub default_frame_index: u8,
    pub frames: Vec<FrameDescriptor>,
}

impl VideoFormatDescriptor {
    /// Find the frame descriptor closest to a target resolution.
    pub fn find_closest_frame(&self, target_w: u16, target_h: u16) -> Option<&FrameDescriptor> {
        let target_pixels = target_w as u32 * target_h as u32;
        self.frames.iter().min_by_key(|f| {
            let diff = f.pixel_count() as i64 - target_pixels as i64;
            if diff < 0 {
                -diff as u64
            } else {
                diff as u64
            }
        })
    }

    /// Find the frame that exactly matches a given resolution.
    pub fn find_exact_frame(&self, w: u16, h: u16) -> Option<&FrameDescriptor> {
        self.frames.iter().find(|f| f.width == w && f.height == h)
    }
}

// ---------------------------------------------------------------------------
// Camera controls (Q16 fixed-point)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct CameraControl {
    pub selector: u8,
    pub min_q16: i32,
    pub max_q16: i32,
    pub res_q16: i32,
    pub cur_q16: i32,
    pub supported: bool,
}

impl CameraControl {
    pub fn new(selector: u8) -> Self {
        CameraControl {
            selector,
            min_q16: 0,
            max_q16: 255 << 16,
            res_q16: 1 << 16,
            cur_q16: 128 << 16,
            supported: false,
        }
    }

    pub fn set(&mut self, value_q16: i32) {
        if value_q16 < self.min_q16 {
            self.cur_q16 = self.min_q16;
        } else if value_q16 > self.max_q16 {
            self.cur_q16 = self.max_q16;
        } else {
            self.cur_q16 = value_q16;
        }
    }

    /// Convert current Q16 value to a 16-bit register value for USB request.
    pub fn to_register(&self) -> i16 {
        (self.cur_q16 >> 16) as i16
    }
}

// ---------------------------------------------------------------------------
// Probe/Commit control block (26 bytes for UVC 1.1)
// ---------------------------------------------------------------------------

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ProbeCommitControl {
    pub hint: u16,
    pub format_index: u8,
    pub frame_index: u8,
    pub frame_interval: u32,
    pub key_frame_rate: u16,
    pub p_frame_rate: u16,
    pub comp_quality: u16,
    pub comp_window_size: u16,
    pub delay: u16,
    pub max_video_frame_size: u32,
    pub max_payload_transfer_size: u32,
}

impl ProbeCommitControl {
    pub fn new() -> Self {
        ProbeCommitControl {
            hint: 0,
            format_index: 1,
            frame_index: 1,
            frame_interval: 333333, // 30 fps (100 ns units)
            key_frame_rate: 0,
            p_frame_rate: 0,
            comp_quality: 0,
            comp_window_size: 0,
            delay: 0,
            max_video_frame_size: 0,
            max_payload_transfer_size: 0,
        }
    }

    /// Set the desired frame rate via interval (in 100 ns units).
    pub fn set_fps(&mut self, fps: u32) {
        if fps == 0 {
            return;
        }
        self.frame_interval = 10_000_000 / fps;
    }

    /// Serialize to bytes for USB control transfer.
    pub fn to_bytes(&self) -> [u8; 26] {
        let mut buf = [0u8; 26];
        let h = self.hint;
        buf[0] = (h & 0xFF) as u8;
        buf[1] = ((h >> 8) & 0xFF) as u8;
        buf[2] = self.format_index;
        buf[3] = self.frame_index;
        let fi = self.frame_interval;
        buf[4] = (fi & 0xFF) as u8;
        buf[5] = ((fi >> 8) & 0xFF) as u8;
        buf[6] = ((fi >> 16) & 0xFF) as u8;
        buf[7] = ((fi >> 24) & 0xFF) as u8;
        let kfr = self.key_frame_rate;
        buf[8] = (kfr & 0xFF) as u8;
        buf[9] = ((kfr >> 8) & 0xFF) as u8;
        let pfr = self.p_frame_rate;
        buf[10] = (pfr & 0xFF) as u8;
        buf[11] = ((pfr >> 8) & 0xFF) as u8;
        let cq = self.comp_quality;
        buf[12] = (cq & 0xFF) as u8;
        buf[13] = ((cq >> 8) & 0xFF) as u8;
        let cw = self.comp_window_size;
        buf[14] = (cw & 0xFF) as u8;
        buf[15] = ((cw >> 8) & 0xFF) as u8;
        let d = self.delay;
        buf[16] = (d & 0xFF) as u8;
        buf[17] = ((d >> 8) & 0xFF) as u8;
        let mvfs = self.max_video_frame_size;
        buf[18] = (mvfs & 0xFF) as u8;
        buf[19] = ((mvfs >> 8) & 0xFF) as u8;
        buf[20] = ((mvfs >> 16) & 0xFF) as u8;
        buf[21] = ((mvfs >> 24) & 0xFF) as u8;
        let mpts = self.max_payload_transfer_size;
        buf[22] = (mpts & 0xFF) as u8;
        buf[23] = ((mpts >> 8) & 0xFF) as u8;
        buf[24] = ((mpts >> 16) & 0xFF) as u8;
        buf[25] = ((mpts >> 24) & 0xFF) as u8;
        buf
    }

    /// Parse from a 26-byte response.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 26 {
            return None;
        }
        Some(ProbeCommitControl {
            hint: (data[0] as u16) | ((data[1] as u16) << 8),
            format_index: data[2],
            frame_index: data[3],
            frame_interval: (data[4] as u32)
                | ((data[5] as u32) << 8)
                | ((data[6] as u32) << 16)
                | ((data[7] as u32) << 24),
            key_frame_rate: (data[8] as u16) | ((data[9] as u16) << 8),
            p_frame_rate: (data[10] as u16) | ((data[11] as u16) << 8),
            comp_quality: (data[12] as u16) | ((data[13] as u16) << 8),
            comp_window_size: (data[14] as u16) | ((data[15] as u16) << 8),
            delay: (data[16] as u16) | ((data[17] as u16) << 8),
            max_video_frame_size: (data[18] as u32)
                | ((data[19] as u32) << 8)
                | ((data[20] as u32) << 16)
                | ((data[21] as u32) << 24),
            max_payload_transfer_size: (data[22] as u32)
                | ((data[23] as u32) << 8)
                | ((data[24] as u32) << 16)
                | ((data[25] as u32) << 24),
        })
    }
}

// ---------------------------------------------------------------------------
// Video device
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoDeviceState {
    Idle,
    Probed,
    Committed,
    Streaming,
    Error,
}

pub struct VideoDevice {
    pub slot_id: u8,
    pub state: VideoDeviceState,
    pub formats: Vec<VideoFormatDescriptor>,
    pub iso_endpoint: u8,
    pub iso_max_packet: u16,
    pub iso_interval: u8,
    pub brightness: CameraControl,
    pub contrast: CameraControl,
    pub saturation: CameraControl,
    pub sharpness: CameraControl,
    pub gain: CameraControl,
    pub probe: ProbeCommitControl,
    pub active_format: u8,
    pub active_frame: u8,
    pub active_width: u16,
    pub active_height: u16,
}

impl VideoDevice {
    pub fn new(slot_id: u8) -> Self {
        VideoDevice {
            slot_id,
            state: VideoDeviceState::Idle,
            formats: Vec::new(),
            iso_endpoint: 0,
            iso_max_packet: 0,
            iso_interval: 1,
            brightness: CameraControl::new(PU_BRIGHTNESS),
            contrast: CameraControl::new(PU_CONTRAST),
            saturation: CameraControl::new(PU_SATURATION),
            sharpness: CameraControl::new(PU_SHARPNESS),
            gain: CameraControl::new(PU_GAIN),
            probe: ProbeCommitControl::new(),
            active_format: 0,
            active_frame: 0,
            active_width: 0,
            active_height: 0,
        }
    }

    // ----- descriptor parsing -----

    /// Parse a VS_FORMAT_UNCOMPRESSED descriptor.
    pub fn parse_format_uncompressed(&mut self, data: &[u8]) {
        if data.len() < 27 {
            return;
        }
        let mut guid = [0u8; 16];
        guid.copy_from_slice(&data[5..21]);
        let bpp = data[21];
        let pixel_format = match &guid[..4] {
            [0x59, 0x55, 0x59, 0x32] => PixelFormat::Yuy2,
            [0x4E, 0x56, 0x31, 0x32] => PixelFormat::Nv12,
            _ => PixelFormat::Uncompressed,
        };
        self.formats.push(VideoFormatDescriptor {
            format_index: data[3],
            pixel_format,
            guid,
            bits_per_pixel: bpp,
            default_frame_index: data[22],
            frames: Vec::new(),
        });
    }

    /// Parse a VS_FORMAT_MJPEG descriptor.
    pub fn parse_format_mjpeg(&mut self, data: &[u8]) {
        if data.len() < 11 {
            return;
        }
        self.formats.push(VideoFormatDescriptor {
            format_index: data[3],
            pixel_format: PixelFormat::Mjpeg,
            guid: [0u8; 16],
            bits_per_pixel: 0,
            default_frame_index: data[5],
            frames: Vec::new(),
        });
    }

    /// Parse a VS_FRAME descriptor (uncompressed or MJPEG).
    pub fn parse_frame_descriptor(&mut self, data: &[u8]) {
        if data.len() < 26 {
            return;
        }
        let width = (data[5] as u16) | ((data[6] as u16) << 8);
        let height = (data[7] as u16) | ((data[8] as u16) << 8);
        let min_interval = (data[21] as u32)
            | ((data[22] as u32) << 8)
            | ((data[23] as u32) << 16)
            | ((data[24] as u32) << 24);
        let max_interval = if data.len() >= 30 {
            (data[25] as u32)
                | ((data[26] as u32) << 8)
                | ((data[27] as u32) << 16)
                | ((data[28] as u32) << 24)
        } else {
            min_interval
        };
        let default_interval = (data[17] as u32)
            | ((data[18] as u32) << 8)
            | ((data[19] as u32) << 16)
            | ((data[20] as u32) << 24);

        let frame = FrameDescriptor {
            width,
            height,
            min_frame_interval: min_interval,
            max_frame_interval: max_interval,
            default_frame_interval: default_interval,
            frame_index: data[3],
        };

        // Attach to the last format
        if let Some(fmt) = self.formats.last_mut() {
            fmt.frames.push(frame);
        }
    }

    // ----- negotiation -----

    /// Negotiate the best resolution, preferring common sizes.
    pub fn negotiate_resolution(&mut self) -> bool {
        let preferred: [(u16, u16); 6] = [
            (1920, 1080),
            (1280, 720),
            (640, 480),
            (320, 240),
            (800, 600),
            (1024, 768),
        ];

        for &(w, h) in &preferred {
            for fmt in &self.formats {
                if let Some(frame) = fmt.find_exact_frame(w, h) {
                    self.active_format = fmt.format_index;
                    self.active_frame = frame.frame_index;
                    self.active_width = w;
                    self.active_height = h;
                    self.probe.format_index = fmt.format_index;
                    self.probe.frame_index = frame.frame_index;
                    self.probe.frame_interval = frame.default_frame_interval;
                    self.state = VideoDeviceState::Probed;
                    return true;
                }
            }
        }

        // Fallback: pick the first available frame
        if let Some(fmt) = self.formats.first() {
            if let Some(frame) = fmt.frames.first() {
                self.active_format = fmt.format_index;
                self.active_frame = frame.frame_index;
                self.active_width = frame.width;
                self.active_height = frame.height;
                self.probe.format_index = fmt.format_index;
                self.probe.frame_index = frame.frame_index;
                self.probe.frame_interval = frame.default_frame_interval;
                self.state = VideoDeviceState::Probed;
                return true;
            }
        }
        false
    }

    /// Commit the negotiated settings.
    pub fn commit(&mut self) -> bool {
        if self.state != VideoDeviceState::Probed {
            return false;
        }
        self.state = VideoDeviceState::Committed;
        true
    }

    /// Compute estimated bandwidth in bytes per second (Q16).
    pub fn bandwidth_bps_q16(&self) -> i32 {
        if self.probe.frame_interval == 0 {
            return 0;
        }
        let fps = 10_000_000_u32 / self.probe.frame_interval;
        let frame_bytes = self.active_width as u32 * self.active_height as u32 * 2; // assume 16bpp
        let bps = fps * frame_bytes;
        // Clamp to avoid Q16 overflow for very high bandwidths
        if bps > 0x7FFF_0000 {
            return 0x7FFF_0000_u32 as i32;
        }
        bps as i32
    }

    /// Start video streaming.
    pub fn start_streaming(&mut self) -> bool {
        if self.state != VideoDeviceState::Committed {
            return false;
        }
        self.state = VideoDeviceState::Streaming;
        true
    }

    /// Stop video streaming.
    pub fn stop_streaming(&mut self) {
        if self.state == VideoDeviceState::Streaming {
            self.state = VideoDeviceState::Committed;
        }
    }

    /// Get a list of all supported resolutions across all formats.
    pub fn supported_resolutions(&self) -> Vec<(u16, u16)> {
        let mut result = Vec::new();
        for fmt in &self.formats {
            for frame in &fmt.frames {
                let pair = (frame.width, frame.height);
                if !result.contains(&pair) {
                    result.push(pair);
                }
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Payload header parsing
// ---------------------------------------------------------------------------

/// UVC payload header (first 2+ bytes of each isochronous packet).
#[derive(Debug, Clone, Copy)]
pub struct PayloadHeader {
    pub length: u8,
    pub frame_id: bool,
    pub end_of_frame: bool,
    pub has_pts: bool,
    pub has_scr: bool,
    pub still_image: bool,
    pub error: bool,
}

impl PayloadHeader {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 2 {
            return None;
        }
        let len = data[0];
        let bfh = data[1];
        Some(PayloadHeader {
            length: len,
            frame_id: bfh & 0x01 != 0,
            end_of_frame: bfh & 0x02 != 0,
            has_pts: bfh & 0x04 != 0,
            has_scr: bfh & 0x08 != 0,
            still_image: bfh & 0x20 != 0,
            error: bfh & 0x40 != 0,
        })
    }
}

// ---------------------------------------------------------------------------
// Class identification
// ---------------------------------------------------------------------------

pub fn is_video_control(class: u8, subclass: u8) -> bool {
    class == CLASS_VIDEO && subclass == SC_VIDEOCONTROL
}

pub fn is_video_streaming(class: u8, subclass: u8) -> bool {
    class == CLASS_VIDEO && subclass == SC_VIDEOSTREAMING
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut state = VIDEO_STATE.lock();
    *state = Some(VideoClassState::new());
    serial_println!("    [video] USB Video Class driver loaded (UVC)");
}
