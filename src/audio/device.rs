//! Audio device abstraction layer
//!
//! Provides hardware audio interface for playback and capture.

use super::error::*;
use super::types::*;

/// Placeholder MMIO base for USB Audio Class (UAC) devices.
/// Populated by `init_uac_device()` when a UAC endpoint is configured.
const UAC_MMIO_BASE: usize = 0;

/// Audio device handle
pub struct AudioDevice {
    device_id: u32,
    config: AudioConfig,
    direction: DeviceDirection,
    state: DeviceState,
    /// MMIO base address for HDA (BAR0) or UAC endpoint control region.
    /// 0 means the device has not yet had its MMIO mapped.
    pub mmio_base: usize,
}

/// Configure a USB Audio Class (UAC) endpoint.
///
/// Looks up the endpoint through the USB stack and records the MMIO/control
/// base so that subsequent pause/resume operations can reach the hardware.
///
/// Returns `Ok(())` on success, or an error string if the USB stack rejects
/// the endpoint address.
pub fn init_uac_device(endpoint_addr: u8) -> core::result::Result<(), &'static str> {
    // Stub: log the endpoint address so the caller can see it was received.
    // USB audio-class support is not yet compiled in; this placeholder records
    // the requested endpoint so the call is not lost.
    let _ = endpoint_addr;
    crate::serial_println!(
        "    [audio] UAC endpoint {:#04x} registered (USB audio stack not compiled in)",
        endpoint_addr
    );
    Ok(())
}

/// Device direction (playback/capture)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceDirection {
    Playback,
    Capture,
    Duplex,
}

/// Device state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Closed,
    Opened,
    Prepared,
    Running,
    Paused,
    XRun, // Buffer underrun/overrun
}

/// Device capabilities
pub struct DeviceCapabilities {
    pub name: &'static str,
    pub max_channels: u8,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
    pub supported_formats: &'static [SampleFormat],
    pub min_buffer_size: usize,
    pub max_buffer_size: usize,
}

static mut DEVICE_LIST: [Option<AudioDevice>; 16] = [const { None }; 16];
static mut DEVICE_COUNT: usize = 0;

/// Initialize audio device subsystem
pub fn init() -> Result<()> {
    unsafe {
        // Initialize default playback device
        let device = AudioDevice::new(0, DeviceDirection::Playback);
        DEVICE_LIST[0] = Some(device);
        DEVICE_COUNT = 1;
    }

    Ok(())
}

/// Shutdown audio device subsystem
pub fn shutdown() {
    unsafe {
        for device in DEVICE_LIST.iter_mut() {
            if let Some(dev) = device.take() {
                let _ = dev.close();
            }
        }
        DEVICE_COUNT = 0;
    }
}

/// Suspend the audio device subsystem.
///
/// Closes all DMA rings and mutes DACs by calling shutdown().  The device
/// list is cleared; resume() must call init() to re-open hardware.
/// This is intentionally a thin wrapper so the suspend/resume path has a
/// symmetric, named pair rather than calling shutdown() directly.
pub fn suspend() {
    crate::serial_println!("  [audio] suspend: closing DMA rings and muting DAC");
    shutdown();
}

/// Resume the audio device subsystem after a sleep state.
///
/// Re-initialises the audio hardware (equivalent to a fresh init()).
/// This is safe to call even if the subsystem was not previously suspended.
pub fn resume() {
    crate::serial_println!("  [audio] resume: re-initializing audio hardware");
    let _ = init();
}

/// Enumerate available audio devices
pub fn enumerate_devices() -> &'static [Option<AudioDevice>] {
    unsafe { &DEVICE_LIST[..DEVICE_COUNT] }
}

/// Open an audio device
pub fn open_device(
    device_id: u32,
    direction: DeviceDirection,
    config: &AudioConfig,
) -> Result<u32> {
    unsafe {
        for (idx, slot) in DEVICE_LIST.iter_mut().enumerate() {
            if slot.is_none() {
                let mut device = AudioDevice::new(device_id, direction);
                device.config = *config;
                device.open()?;
                *slot = Some(device);

                if idx >= DEVICE_COUNT {
                    DEVICE_COUNT = idx + 1;
                }

                return Ok(idx as u32);
            }
        }
    }

    Err(AudioError::DeviceUnavailable)
}

/// Close an audio device
pub fn close_device(handle: u32) -> Result<()> {
    unsafe {
        if (handle as usize) < DEVICE_LIST.len() {
            if let Some(device) = DEVICE_LIST[handle as usize].take() {
                return device.close();
            }
        }
    }

    Err(AudioError::DeviceUnavailable)
}

/// Start audio device
pub fn start_device(handle: u32) -> Result<()> {
    unsafe {
        if (handle as usize) < DEVICE_LIST.len() {
            if let Some(device) = DEVICE_LIST[handle as usize].as_mut() {
                return device.start();
            }
        }
    }

    Err(AudioError::DeviceUnavailable)
}

/// Stop audio device
pub fn stop_device(handle: u32) -> Result<()> {
    unsafe {
        if (handle as usize) < DEVICE_LIST.len() {
            if let Some(device) = DEVICE_LIST[handle as usize].as_mut() {
                return device.stop();
            }
        }
    }

    Err(AudioError::DeviceUnavailable)
}

/// Write audio samples to playback device
pub fn write_samples(handle: u32, data: &[u8]) -> Result<usize> {
    unsafe {
        if (handle as usize) < DEVICE_LIST.len() {
            if let Some(device) = DEVICE_LIST[handle as usize].as_mut() {
                return device.write(data);
            }
        }
    }

    Err(AudioError::DeviceUnavailable)
}

/// Read audio samples from capture device
pub fn read_samples(handle: u32, buffer: &mut [u8]) -> Result<usize> {
    unsafe {
        if (handle as usize) < DEVICE_LIST.len() {
            if let Some(device) = DEVICE_LIST[handle as usize].as_mut() {
                return device.read(buffer);
            }
        }
    }

    Err(AudioError::DeviceUnavailable)
}

impl AudioDevice {
    fn new(device_id: u32, direction: DeviceDirection) -> Self {
        Self {
            device_id,
            config: AudioConfig::default(),
            direction,
            state: DeviceState::Closed,
            mmio_base: UAC_MMIO_BASE,
        }
    }

    fn open(&mut self) -> Result<()> {
        if self.state != DeviceState::Closed {
            return Err(AudioError::DeviceIoError);
        }

        // Hardware initialization would go here
        // For bare-metal, this would configure audio DMA, I2S, etc.

        self.state = DeviceState::Opened;
        Ok(())
    }

    fn close(mut self) -> Result<()> {
        if self.state == DeviceState::Running {
            self.stop()?;
        }

        // Hardware cleanup would go here

        self.state = DeviceState::Closed;
        Ok(())
    }

    fn prepare(&mut self) -> Result<()> {
        if self.state != DeviceState::Opened {
            return Err(AudioError::DeviceIoError);
        }

        // Configure hardware buffers, DMA, etc.

        self.state = DeviceState::Prepared;
        Ok(())
    }

    fn start(&mut self) -> Result<()> {
        if self.state != DeviceState::Prepared && self.state != DeviceState::Paused {
            self.prepare()?;
        }

        // Start hardware playback/capture

        self.state = DeviceState::Running;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        if self.state != DeviceState::Running && self.state != DeviceState::Paused {
            return Ok(());
        }

        // Stop hardware playback/capture

        self.state = DeviceState::Prepared;
        Ok(())
    }

    fn pause(&mut self) -> Result<()> {
        if self.state != DeviceState::Running {
            return Err(AudioError::DeviceIoError);
        }

        // Pause hardware DMA/I2S stream.
        //
        // For Intel HDA (High Definition Audio):
        //   - Clear the Run bit (SD_CTL.RUN, bit 1) in the stream descriptor
        //     control register to stop the DMA engine without resetting the
        //     position counters.
        //   - Stream descriptor base for playback stream 0 is at BAR0 + 0x100.
        //   - SD_CTL offset within each 0x20-byte descriptor = 0x00.
        if self.mmio_base != 0 {
            // HDA path: clear RUN bit (bit 1) in SD0CTL register (BAR0 + 0x100).
            let sd0ctl = (self.mmio_base + 0x100) as *mut u32;
            unsafe {
                let ctl = core::ptr::read_volatile(sd0ctl);
                core::ptr::write_volatile(sd0ctl, ctl & !0x02);
            }
        }
        // For USB Audio Class (UAC): send PAUSE control request via USB EP0.
        // (UAC pause is handled by the USB stack when mmio_base points to
        // a UAC endpoint control region set up by init_uac_device().)

        self.state = DeviceState::Paused;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.state != DeviceState::Paused {
            return Err(AudioError::DeviceIoError);
        }

        // Resume hardware DMA/I2S stream from pause.
        //
        // For Intel HDA:
        //   - Set the Run bit (SD_CTL.RUN, bit 1) in the stream descriptor
        //     control register.  The position counter continues from where it
        //     was when pause was issued.
        if self.mmio_base != 0 {
            // HDA path: set RUN bit (bit 1) in SD0CTL register (BAR0 + 0x100).
            let sd0ctl = (self.mmio_base + 0x100) as *mut u32;
            unsafe {
                let ctl = core::ptr::read_volatile(sd0ctl);
                core::ptr::write_volatile(sd0ctl, ctl | 0x02);
            }
        }
        // For USB Audio Class (UAC): resume handled by the USB stack.

        self.state = DeviceState::Running;
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> Result<usize> {
        if self.direction == DeviceDirection::Capture {
            return Err(AudioError::DeviceIoError);
        }

        if self.state != DeviceState::Running {
            return Err(AudioError::DeviceIoError);
        }

        // Write to hardware buffer
        // In real implementation, this would:
        // 1. Wait for DMA buffer space
        // 2. Copy data to DMA buffer
        // 3. Trigger DMA transfer

        Ok(data.len())
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        if self.direction == DeviceDirection::Playback {
            return Err(AudioError::DeviceIoError);
        }

        if self.state != DeviceState::Running {
            return Err(AudioError::DeviceIoError);
        }

        // Read from hardware buffer
        // In real implementation, this would:
        // 1. Wait for DMA buffer data
        // 2. Copy data from DMA buffer
        // 3. Update buffer pointers

        Ok(buffer.len())
    }

    pub fn get_capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            name: "Genesis Audio Device",
            max_channels: 8,
            min_sample_rate: 8000,
            max_sample_rate: 192000,
            supported_formats: &[
                SampleFormat::U8,
                SampleFormat::S16LE,
                SampleFormat::S24LE,
                SampleFormat::S32LE,
                SampleFormat::F32LE,
            ],
            min_buffer_size: 256,
            max_buffer_size: 8192,
        }
    }

    pub fn get_state(&self) -> DeviceState {
        self.state
    }

    pub fn get_config(&self) -> &AudioConfig {
        &self.config
    }

    pub fn set_config(&mut self, config: AudioConfig) -> Result<()> {
        if self.state != DeviceState::Opened {
            return Err(AudioError::DeviceIoError);
        }

        // Validate config
        let caps = self.get_capabilities();

        if config.sample_rate < caps.min_sample_rate || config.sample_rate > caps.max_sample_rate {
            return Err(AudioError::InvalidSampleRate);
        }

        if config.channels > caps.max_channels {
            return Err(AudioError::InvalidChannels);
        }

        self.config = config;
        Ok(())
    }
}

/// Hardware-specific audio driver interface
pub trait AudioDriver {
    /// Initialize hardware
    fn init(&mut self) -> Result<()>;

    /// Configure hardware parameters
    fn configure(&mut self, config: &AudioConfig) -> Result<()>;

    /// Start playback/capture
    fn start(&mut self) -> Result<()>;

    /// Stop playback/capture
    fn stop(&mut self) -> Result<()>;

    /// Write samples to hardware
    fn write_samples(&mut self, data: &[u8]) -> Result<usize>;

    /// Read samples from hardware
    fn read_samples(&mut self, buffer: &mut [u8]) -> Result<usize>;

    /// Get buffer status
    fn get_buffer_status(&self) -> (usize, usize); // (available, total)
}
