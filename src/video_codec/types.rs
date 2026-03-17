// video_codec/types.rs - Common types for video codec framework

#![no_std]

/// Supported video codec types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    H264,
    H265,
    VP9,
    AV1,
}

/// Video frame structure
#[repr(C)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub y_plane: *mut u8,
    pub u_plane: *mut u8,
    pub v_plane: *mut u8,
    pub y_stride: u32,
    pub u_stride: u32,
    pub v_stride: u32,
    pub timestamp: u64,
    pub frame_type: FrameType,
}

impl Frame {
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Self {
        Self {
            width,
            height,
            format,
            y_plane: core::ptr::null_mut(),
            u_plane: core::ptr::null_mut(),
            v_plane: core::ptr::null_mut(),
            y_stride: width,
            u_stride: width / 2,
            v_stride: width / 2,
            timestamp: 0,
            frame_type: FrameType::Unknown,
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.y_plane.is_null() && !self.u_plane.is_null() && !self.v_plane.is_null()
    }
}

/// Pixel format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    YUV420,
    YUV422,
    YUV444,
    NV12,
    NV21,
    RGB24,
    RGBA32,
}

/// Frame type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Unknown,
    I,      // Intra frame
    P,      // Predicted frame
    B,      // Bidirectional predicted frame
    IDR,    // Instantaneous Decoder Refresh (H.264/H.265)
}

/// Encoder configuration
#[derive(Debug, Clone, Copy)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate: u32,
    pub gop_size: u32,
    pub max_b_frames: u32,
    pub profile: Profile,
    pub preset: Preset,
    pub rate_control: RateControl,
    pub use_hardware: bool,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            framerate: 30,
            bitrate: 5_000_000,  // 5 Mbps
            gop_size: 30,
            max_b_frames: 2,
            profile: Profile::Main,
            preset: Preset::Medium,
            rate_control: RateControl::CBR,
            use_hardware: true,
        }
    }
}

/// Codec profile
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Baseline,
    Main,
    High,
    High10,
    High422,
    High444,
}

/// Encoding preset
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    UltraFast,
    SuperFast,
    VeryFast,
    Faster,
    Fast,
    Medium,
    Slow,
    Slower,
    VerySlow,
}

/// Rate control mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateControl {
    CBR,    // Constant bitrate
    VBR,    // Variable bitrate
    CQP,    // Constant quantization parameter
    AVBR,   // Average VBR
}

/// Hardware acceleration capabilities
#[repr(C)]
pub struct HardwareCapabilities {
    pub h264_decode: bool,
    pub h264_encode: bool,
    pub h265_decode: bool,
    pub h265_encode: bool,
    pub vp9_decode: bool,
    pub vp9_encode: bool,
    pub av1_decode: bool,
    pub av1_encode: bool,
    pub max_width: u32,
    pub max_height: u32,
    pub max_bitrate: u32,
    pub vendor: HardwareVendor,
}

impl Default for HardwareCapabilities {
    fn default() -> Self {
        Self {
            h264_decode: false,
            h264_encode: false,
            h265_decode: false,
            h265_encode: false,
            vp9_decode: false,
            vp9_encode: false,
            av1_decode: false,
            av1_encode: false,
            max_width: 3840,
            max_height: 2160,
            max_bitrate: 100_000_000,
            vendor: HardwareVendor::None,
        }
    }
}

/// Hardware vendor
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareVendor {
    None,
    Intel,
    AMD,
    Nvidia,
    Qualcomm,
    MediaTek,
}

/// Codec error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecError {
    InvalidParameter,
    DecoderNotInitialized,
    EncoderNotInitialized,
    HardwareNotAvailable,
    BufferTooSmall,
    InvalidBitstream,
    UnsupportedFeature,
    OutOfMemory,
    InternalError,
}

/// NAL unit types for H.264/H.265
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalUnitType {
    // H.264
    H264NonIDR = 1,
    H264IDR = 5,
    H264SEI = 6,
    H264SPS = 7,
    H264PPS = 8,
    H264AUD = 9,

    // H.265
    H265TrailN = 0,
    H265TrailR = 1,
    H265IDR = 19,
    H265VPS = 32,
    H265SPS = 33,
    H265PPS = 34,
    H265SEI = 39,
}

/// Motion vector
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MotionVector {
    pub x: i16,
    pub y: i16,
}

/// Macroblock (16x16 for H.264/H.265)
#[repr(C)]
pub struct Macroblock {
    pub mb_type: MacroblockType,
    pub qp: u8,
    pub mv: [MotionVector; 16],
    pub ref_idx: [i8; 2],
}

/// Macroblock type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroblockType {
    Intra4x4,
    Intra16x16,
    Inter16x16,
    Inter16x8,
    Inter8x16,
    Inter8x8,
    Skip,
}

/// Transform coefficients
pub type TransformBlock = [i16; 16];
pub type TransformBlock8x8 = [i16; 64];

/// Quantization parameter
pub struct QuantizationParams {
    pub qp: u8,
    pub qp_y: u8,
    pub qp_cb: u8,
    pub qp_cr: u8,
}
