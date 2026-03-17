/// Camera framework for Genesis — capture device management
///
/// Provides a camera HAL (Hardware Abstraction Layer) for USB
/// and built-in cameras. Supports capture, preview, and settings.
///
/// Inspired by: Android Camera2, V4L2. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Camera state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraState {
    Closed,
    Opened,
    Previewing,
    Recording,
    Error,
}

/// Camera facing direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraFacing {
    Front,
    Back,
    External,
}

/// Pixel format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb888,
    Argb8888,
    Yuyv,
    Nv21,
    Jpeg,
}

/// Camera resolution
#[derive(Debug, Clone, Copy)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    pub const VGA: Resolution = Resolution {
        width: 640,
        height: 480,
    };
    pub const HD: Resolution = Resolution {
        width: 1280,
        height: 720,
    };
    pub const FULL_HD: Resolution = Resolution {
        width: 1920,
        height: 1080,
    };
    pub const UHD_4K: Resolution = Resolution {
        width: 3840,
        height: 2160,
    };
}

/// Camera capabilities
pub struct CameraInfo {
    pub id: u8,
    pub name: String,
    pub facing: CameraFacing,
    pub resolutions: Vec<Resolution>,
    pub max_fps: u8,
    pub has_flash: bool,
    pub has_autofocus: bool,
    pub has_zoom: bool,
}

/// Camera device
pub struct Camera {
    pub info: CameraInfo,
    pub state: CameraState,
    pub current_resolution: Resolution,
    pub current_fps: u8,
    pub format: PixelFormat,
    /// Capture settings
    pub exposure: i8, // -4 to +4 EV
    pub white_balance: WhiteBalance,
    pub flash_mode: FlashMode,
    pub zoom: u16, // 10 = 1.0x, 20 = 2.0x
    pub autofocus: bool,
    /// Frame buffer for preview
    pub preview_buf: Vec<u8>,
    /// Statistics
    pub frames_captured: u64,
    pub photos_taken: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum WhiteBalance {
    Auto,
    Daylight,
    Cloudy,
    Tungsten,
    Fluorescent,
}

#[derive(Debug, Clone, Copy)]
pub enum FlashMode {
    Off,
    On,
    Auto,
    Torch,
}

impl Camera {
    pub fn new(id: u8, facing: CameraFacing) -> Self {
        let name = match facing {
            CameraFacing::Front => format!("Front Camera {}", id),
            CameraFacing::Back => format!("Rear Camera {}", id),
            CameraFacing::External => format!("USB Camera {}", id),
        };

        Camera {
            info: CameraInfo {
                id,
                name,
                facing,
                resolutions: alloc::vec![Resolution::VGA, Resolution::HD, Resolution::FULL_HD,],
                max_fps: 30,
                has_flash: facing == CameraFacing::Back,
                has_autofocus: true,
                has_zoom: true,
            },
            state: CameraState::Closed,
            current_resolution: Resolution::HD,
            current_fps: 30,
            format: PixelFormat::Argb8888,
            exposure: 0,
            white_balance: WhiteBalance::Auto,
            flash_mode: FlashMode::Auto,
            zoom: 10,
            autofocus: true,
            preview_buf: Vec::new(),
            frames_captured: 0,
            photos_taken: 0,
        }
    }

    /// Open the camera
    pub fn open(&mut self) -> bool {
        if self.state != CameraState::Closed {
            return false;
        }
        let buf_size =
            (self.current_resolution.width * self.current_resolution.height * 4) as usize;
        self.preview_buf = alloc::vec![0u8; buf_size];
        self.state = CameraState::Opened;
        true
    }

    /// Start preview
    pub fn start_preview(&mut self) -> bool {
        if self.state != CameraState::Opened {
            return false;
        }
        self.state = CameraState::Previewing;
        true
    }

    /// Capture a photo
    pub fn take_photo(&mut self) -> Option<Vec<u8>> {
        if self.state != CameraState::Previewing {
            return None;
        }
        self.photos_taken = self.photos_taken.saturating_add(1);
        // Return current preview buffer as the photo
        Some(self.preview_buf.clone())
    }

    /// Start recording
    pub fn start_recording(&mut self) -> bool {
        if self.state != CameraState::Previewing {
            return false;
        }
        self.state = CameraState::Recording;
        true
    }

    /// Stop recording
    pub fn stop_recording(&mut self) -> bool {
        if self.state != CameraState::Recording {
            return false;
        }
        self.state = CameraState::Previewing;
        true
    }

    /// Close the camera
    pub fn close(&mut self) {
        self.state = CameraState::Closed;
        self.preview_buf.clear();
    }

    /// Set resolution
    pub fn set_resolution(&mut self, res: Resolution) -> bool {
        if self.state == CameraState::Previewing || self.state == CameraState::Recording {
            return false; // can't change while streaming
        }
        self.current_resolution = res;
        true
    }

    /// Set zoom level (10 = 1.0x)
    pub fn set_zoom(&mut self, zoom: u16) {
        self.zoom = zoom.max(10).min(80); // 1.0x to 8.0x
    }
}

/// Camera manager
pub struct CameraManager {
    pub cameras: Vec<Camera>,
}

impl CameraManager {
    const fn new() -> Self {
        CameraManager {
            cameras: Vec::new(),
        }
    }

    fn setup(&mut self) {
        // Simulated cameras
        self.cameras.push(Camera::new(0, CameraFacing::Back));
        self.cameras.push(Camera::new(1, CameraFacing::Front));
    }

    pub fn get_camera(&mut self, id: u8) -> Option<&mut Camera> {
        self.cameras.iter_mut().find(|c| c.info.id == id)
    }

    pub fn camera_count(&self) -> usize {
        self.cameras.len()
    }

    pub fn list(&self) -> Vec<(u8, String)> {
        self.cameras
            .iter()
            .map(|c| (c.info.id, c.info.name.clone()))
            .collect()
    }
}

static CAMERAS: Mutex<CameraManager> = Mutex::new(CameraManager::new());

pub fn init() {
    CAMERAS.lock().setup();
    crate::serial_println!(
        "  [camera] Camera framework initialized ({} cameras)",
        CAMERAS.lock().camera_count()
    );
}

pub fn camera_count() -> usize {
    CAMERAS.lock().camera_count()
}
