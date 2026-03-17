// compositor/display.rs - Display management and output

use crate::compositor::{
    composition::Framebuffer,
    types::{PixelFormat, Rect},
    vsync::DisplayTiming,
};

/// Display connection type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayConnection {
    Internal,   // Built-in display (laptop/tablet)
    HDMI,
    DisplayPort,
    DVI,
    VGA,
    Unknown,
}

/// Display state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayState {
    Off,
    On,
    Standby,
    Suspend,
}

/// Display configuration
#[derive(Clone)]
pub struct DisplayConfig {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub refresh_rate: u32,
    pub timing: DisplayTiming,
}

impl DisplayConfig {
    pub fn new(width: u32, height: u32, refresh_rate: u32) -> Self {
        Self {
            width,
            height,
            format: PixelFormat::RGBA8888,
            refresh_rate,
            timing: DisplayTiming::from_mode(width, height, refresh_rate),
        }
    }

    /// Create 1080p60 config
    pub fn mode_1080p60() -> Self {
        Self::new(1920, 1080, 60)
    }

    /// Create 4K60 config
    pub fn mode_4k60() -> Self {
        Self::new(3840, 2160, 60)
    }

    /// Create 1440p144 config
    pub fn mode_1440p144() -> Self {
        Self::new(2560, 1440, 144)
    }
}

/// Physical display device
pub struct Display {
    pub id: usize,
    pub name: [u8; 32],
    pub name_len: usize,
    pub connection: DisplayConnection,
    pub config: DisplayConfig,
    pub state: DisplayState,
    pub primary: bool,

    // Hardware resources
    framebuffer_addr: usize,
    framebuffer_size: usize,

    // Capabilities
    pub supported_formats: [PixelFormat; 8],
    pub format_count: usize,
}

impl Display {
    /// Create a new display
    pub fn new(id: usize, name: &str, connection: DisplayConnection, config: DisplayConfig) -> Self {
        let mut name_bytes = [0u8; 32];
        let name_len = name.len().min(32);
        name_bytes[..name_len].copy_from_slice(&name.as_bytes()[..name_len]);

        Self {
            id,
            name: name_bytes,
            name_len,
            connection,
            config,
            state: DisplayState::Off,
            primary: false,
            framebuffer_addr: 0,
            framebuffer_size: 0,
            supported_formats: [
                PixelFormat::RGBA8888,
                PixelFormat::RGBX8888,
                PixelFormat::BGRA8888,
                PixelFormat::RGB888,
                PixelFormat::RGB565,
                PixelFormat::RGBA8888,
                PixelFormat::RGBA8888,
                PixelFormat::RGBA8888,
            ],
            format_count: 5,
        }
    }

    /// Get display name
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("invalid")
    }

    /// Initialize display hardware
    pub fn init(&mut self) -> Result<(), &'static str> {
        // Allocate framebuffer
        let bpp = self.config.format.bytes_per_pixel();
        self.framebuffer_size = (self.config.width * self.config.height * bpp as u32) as usize;

        // In real implementation:
        // 1. Detect connected display via DDC/EDID
        // 2. Parse EDID for capabilities and modes
        // 3. Configure display controller (CRTC)
        // 4. Set up scanout buffer
        // 5. Program display timing registers
        // 6. Enable output

        log::info!(
            "Display {}: Initialized {} ({}x{}@{}Hz)",
            self.id,
            self.name(),
            self.config.width,
            self.config.height,
            self.config.refresh_rate
        );

        self.state = DisplayState::On;
        Ok(())
    }

    /// Set display mode
    pub fn set_mode(&mut self, config: DisplayConfig) -> Result<(), &'static str> {
        // Validate mode is supported
        if !self.is_mode_supported(&config) {
            return Err("Display mode not supported");
        }

        self.config = config;

        // Reconfigure hardware
        self.configure_hardware()?;

        log::info!(
            "Display {}: Mode set to {}x{}@{}Hz",
            self.id,
            self.config.width,
            self.config.height,
            self.config.refresh_rate
        );

        Ok(())
    }

    /// Check if mode is supported
    fn is_mode_supported(&self, config: &DisplayConfig) -> bool {
        // Check format
        let format_supported = self.supported_formats[..self.format_count]
            .iter()
            .any(|&f| f == config.format);

        if !format_supported {
            return false;
        }

        // In real implementation, check EDID for supported modes
        true
    }

    /// Configure display hardware
    fn configure_hardware(&mut self) -> Result<(), &'static str> {
        // Program display controller registers:
        // - CRTC timing
        // - Framebuffer address and stride
        // - Pixel format
        // - Output enable

        Ok(())
    }

    /// Present framebuffer to display
    pub fn present(&mut self, framebuffer: &Framebuffer) -> Result<(), &'static str> {
        if self.state != DisplayState::On {
            return Err("Display not active");
        }

        // Validate framebuffer matches display config
        if framebuffer.width != self.config.width || framebuffer.height != self.config.height {
            return Err("Framebuffer size mismatch");
        }

        if framebuffer.format != self.config.format {
            return Err("Framebuffer format mismatch");
        }

        // Copy framebuffer to display
        self.blit_framebuffer(framebuffer)?;

        Ok(())
    }

    /// Blit framebuffer to display scanout buffer
    fn blit_framebuffer(&mut self, framebuffer: &Framebuffer) -> Result<(), &'static str> {
        if framebuffer.data.is_null() {
            return Err("Invalid framebuffer");
        }

        if self.framebuffer_addr == 0 {
            return Err("Display framebuffer not allocated");
        }

        unsafe {
            // Copy to display memory
            let src = core::slice::from_raw_parts(framebuffer.data, framebuffer.size);
            let dst = core::slice::from_raw_parts_mut(self.framebuffer_addr as *mut u8, self.framebuffer_size);

            dst[..src.len().min(dst.len())].copy_from_slice(&src[..src.len().min(dst.len())]);
        }

        Ok(())
    }

    /// Get display bounds
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.config.width, self.config.height)
    }

    /// Power off display (DPMS off / ACPI D3).
    ///
    /// Sequence:
    ///   1. Guard: skip if already off.
    ///   2. Blank the framebuffer (write zeros) so no stale image is
    ///      left in VRAM if the panel stays partially powered.
    ///   3. Transition state to Off (display controller logic gate).
    ///   4. For a real GPU driver, write DPMS-off to the CRTC control register
    ///      and assert the display panel power-off GPIO here.
    pub fn power_off(&mut self) {
        if self.state == DisplayState::Off {
            return;
        }
        // Blank the framebuffer before cutting power to avoid visual artifacts.
        if self.framebuffer_addr != 0 && self.framebuffer_size != 0 {
            // Safety: framebuffer_addr and framebuffer_size are set during init
            // from a validated VESA/PCI BAR mapping.  We use volatile writes so
            // the compiler cannot elide the blanking.
            let fb_ptr = self.framebuffer_addr as *mut u32;
            let word_count = self.framebuffer_size / 4;
            for i in 0..word_count {
                unsafe {
                    core::ptr::write_volatile(fb_ptr.add(i), 0x0000_0000);
                }
            }
        }
        self.state = DisplayState::Off;
        // Assert DPMS-off on the appropriate output driver.
        match self.connection {
            DisplayConnection::DisplayPort => {
                crate::drivers::display_port::disable_output();
            }
            DisplayConnection::HDMI => {
                crate::drivers::hdmi::disable_output();
            }
            _ => {
                // Internal / VGA / DVI / Unknown: no software DPMS control available.
                // A real implementation would write to the Bochs VBE ENABLE register
                // or the VGA sequencer to cut the scanout.
            }
        }
        crate::serial_println!("  [display] display {} powered off", self.id);
    }

    /// Power on display (DPMS on / ACPI D0).
    ///
    /// Sequence:
    ///   1. Guard: skip if already on.
    ///   2. Transition state to On.
    ///   3. For a real GPU driver, de-assert DPMS-off and re-enable the
    ///      display controller scanout here.
    pub fn power_on(&mut self) {
        if self.state == DisplayState::On {
            return;
        }
        self.state = DisplayState::On;
        // Re-enable the output on the appropriate driver.
        match self.connection {
            DisplayConnection::DisplayPort => {
                crate::drivers::display_port::enable_output();
            }
            DisplayConnection::HDMI => {
                crate::drivers::hdmi::enable_output();
            }
            _ => {}
        }
        crate::serial_println!("  [display] display {} powered on", self.id);
    }

    /// Suspend display to standby (DPMS standby — backlight off, sync signals active).
    ///
    /// For DP/HDMI outputs: disables the video stream but keeps the link active
    /// so resume is fast (no re-training required).
    /// For internal panels: sets backlight brightness to 0 via PWM channel 0.
    pub fn suspend(&mut self) {
        if self.state == DisplayState::On {
            self.state = DisplayState::Suspend;
            match self.connection {
                DisplayConnection::DisplayPort => {
                    // Cut video stream, keep AUX / link alive for fast resume.
                    crate::drivers::display_port::disable_output();
                    // Blank backlight.
                    crate::drivers::display_port::set_brightness(0);
                }
                DisplayConnection::HDMI => {
                    crate::drivers::hdmi::disable_output();
                    crate::drivers::hdmi::set_brightness(0);
                }
                DisplayConnection::Internal | DisplayConnection::Unknown => {
                    // Internal panel: just kill the backlight via PWM.
                    crate::drivers::pwm::set_duty_percent(0, 0);
                }
                _ => {}
            }
            crate::serial_println!("  [display] display {} suspended", self.id);
        }
    }

    /// Resume display from standby back to active.
    ///
    /// Re-enables the video stream and restores maximum brightness.
    pub fn resume(&mut self) {
        if self.state == DisplayState::Suspend || self.state == DisplayState::Standby {
            self.state = DisplayState::On;
            match self.connection {
                DisplayConnection::DisplayPort => {
                    crate::drivers::display_port::enable_output();
                    // Restore full brightness.
                    crate::drivers::display_port::set_brightness(255);
                }
                DisplayConnection::HDMI => {
                    crate::drivers::hdmi::enable_output();
                    crate::drivers::hdmi::set_brightness(255);
                }
                DisplayConnection::Internal | DisplayConnection::Unknown => {
                    crate::drivers::pwm::set_duty_percent(0, 255);
                }
                _ => {}
            }
            crate::serial_println!("  [display] display {} resumed", self.id);
        }
    }
}

/// Display manager - manages multiple displays
pub struct DisplayManager {
    displays: [Option<Display>; 4],
    display_count: usize,
    primary_display: usize,
}

impl DisplayManager {
    /// Create a new display manager
    pub fn new() -> Self {
        Self {
            displays: [None, None, None, None],
            display_count: 0,
            primary_display: 0,
        }
    }

    /// Initialize and detect displays
    pub fn init(&mut self) {
        log::info!("DisplayManager: Detecting displays...");

        // Detect connected displays
        self.detect_displays();

        // Initialize each display
        for i in 0..self.display_count {
            if let Some(ref mut display) = self.displays[i] {
                if let Err(e) = display.init() {
                    log::error!("Failed to initialize display {}: {}", i, e);
                }
            }
        }

        log::info!("DisplayManager: Initialized {} displays", self.display_count);
    }

    /// Detect connected displays
    fn detect_displays(&mut self) {
        // In real implementation:
        // 1. Enumerate display outputs
        // 2. Check connection status
        // 3. Read EDID
        // 4. Create Display objects

        // For now, create a default display
        let config = DisplayConfig::mode_1080p60();
        let display = Display::new(0, "Primary Display", DisplayConnection::Internal, config);

        self.displays[0] = Some(display);
        self.displays[0].as_mut().unwrap().primary = true;
        self.display_count = 1;
        self.primary_display = 0;
    }

    /// Add a display
    pub fn add_display(&mut self, display: Display) -> Option<usize> {
        if self.display_count >= 4 {
            return None;
        }

        let id = self.display_count;
        self.displays[id] = Some(display);
        self.display_count = self.display_count.saturating_add(1);

        Some(id)
    }

    /// Get primary display
    pub fn primary(&mut self) -> Option<&mut Display> {
        self.displays[self.primary_display].as_mut()
    }

    /// Get display by ID
    pub fn get_display(&mut self, id: usize) -> Option<&mut Display> {
        if id < self.display_count {
            self.displays[id].as_mut()
        } else {
            None
        }
    }

    /// Present framebuffer to primary display
    pub fn present(&mut self, framebuffer: &Framebuffer) -> Result<(), &'static str> {
        if let Some(display) = self.primary() {
            display.present(framebuffer)
        } else {
            Err("No primary display")
        }
    }

    /// Present framebuffer to specific display
    pub fn present_to_display(&mut self, display_id: usize, framebuffer: &Framebuffer) -> Result<(), &'static str> {
        if let Some(display) = self.get_display(display_id) {
            display.present(framebuffer)
        } else {
            Err("Display not found")
        }
    }

    /// Get display count
    pub fn count(&self) -> usize {
        self.display_count
    }

    /// Hot-plug event handler
    pub fn on_hotplug(&mut self, _connector_id: usize, connected: bool) {
        if connected {
            log::info!("Display connected");
            self.detect_displays();
        } else {
            log::info!("Display disconnected");
            // Remove disconnected display
        }
    }
}
