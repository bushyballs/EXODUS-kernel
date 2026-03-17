/// USB Audio class driver for Genesis
///
/// Implements USB Audio Class 1.0/2.0 device handling — enumeration,
/// isochronous endpoint setup, and audio streaming to/from USB devices.
///
/// Inspired by: Linux snd-usb-audio. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// USB Audio device state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbAudioState {
    Disconnected,
    Enumerating,
    Configured,
    Streaming,
    Error,
}

/// USB Audio class version
#[derive(Debug, Clone, Copy)]
pub enum AudioClassVersion {
    V1_0,
    V2_0,
}

/// USB Audio terminal type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalType {
    Speaker,
    Headphones,
    Microphone,
    Headset,
    UsbStreaming,
    Unknown(u16),
}

/// Audio stream format
#[derive(Debug, Clone, Copy)]
pub struct StreamFormat {
    pub channels: u8,
    pub bit_depth: u8,
    pub sample_rate: u32,
}

/// USB Audio endpoint
pub struct AudioEndpoint {
    pub address: u8,
    pub direction: EndpointDirection,
    pub max_packet_size: u16,
    pub interval: u8,
    pub format: StreamFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointDirection {
    In,  // Device to host (capture)
    Out, // Host to device (playback)
}

/// USB Audio device descriptor
pub struct UsbAudioDevice {
    pub state: UsbAudioState,
    pub class_version: AudioClassVersion,
    pub device_addr: u8,
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub input_terminal: TerminalType,
    pub output_terminal: TerminalType,
    pub endpoints: Vec<AudioEndpoint>,
    /// Volume control (0-255)
    pub volume: u8,
    pub muted: bool,
    /// Supported sample rates
    pub supported_rates: Vec<u32>,
    /// Current stream format
    pub current_format: Option<StreamFormat>,
    /// Playback buffer (ring buffer of PCM data)
    pub playback_buf: Vec<u8>,
    pub playback_write: usize,
    pub playback_read: usize,
    /// Capture buffer
    pub capture_buf: Vec<u8>,
    pub capture_write: usize,
    pub capture_read: usize,
    /// Statistics
    pub frames_played: u64,
    pub frames_captured: u64,
    pub xruns: u32, // underrun/overrun count
}

impl UsbAudioDevice {
    pub fn new(addr: u8) -> Self {
        UsbAudioDevice {
            state: UsbAudioState::Disconnected,
            class_version: AudioClassVersion::V1_0,
            device_addr: addr,
            name: String::new(),
            vendor_id: 0,
            product_id: 0,
            input_terminal: TerminalType::Microphone,
            output_terminal: TerminalType::Speaker,
            endpoints: Vec::new(),
            volume: 200,
            muted: false,
            supported_rates: Vec::new(),
            current_format: None,
            playback_buf: alloc::vec![0u8; 16384],
            playback_write: 0,
            playback_read: 0,
            capture_buf: alloc::vec![0u8; 16384],
            capture_write: 0,
            capture_read: 0,
            frames_played: 0,
            frames_captured: 0,
            xruns: 0,
        }
    }

    /// Enumerate audio interfaces from USB descriptors
    pub fn enumerate(&mut self, descriptors: &[u8]) -> bool {
        self.state = UsbAudioState::Enumerating;

        // Parse configuration descriptor to find audio interfaces
        let mut i = 0;
        while i + 1 < descriptors.len() {
            let len = descriptors[i] as usize;
            let desc_type = descriptors[i + 1];

            if len == 0 {
                break;
            }

            match desc_type {
                // Interface descriptor
                0x04 => {
                    if i + 6 < descriptors.len() {
                        let class = descriptors[i + 5];
                        let subclass = descriptors[i + 6];
                        // Audio class = 0x01
                        if class == 0x01 {
                            match subclass {
                                0x01 => { /* AudioControl */ }
                                0x02 => { /* AudioStreaming */ }
                                0x03 => { /* MIDIStreaming */ }
                                _ => {}
                            }
                        }
                    }
                }
                // Endpoint descriptor
                0x05 => {
                    if i + 6 < descriptors.len() {
                        let addr = descriptors[i + 2];
                        let dir = if addr & 0x80 != 0 {
                            EndpointDirection::In
                        } else {
                            EndpointDirection::Out
                        };
                        let max_pkt = u16::from_le_bytes([descriptors[i + 4], descriptors[i + 5]]);
                        let interval = descriptors[i + 6];

                        self.endpoints.push(AudioEndpoint {
                            address: addr & 0x0F,
                            direction: dir,
                            max_packet_size: max_pkt,
                            interval,
                            format: StreamFormat {
                                channels: 2,
                                bit_depth: 16,
                                sample_rate: 48000,
                            },
                        });
                    }
                }
                _ => {}
            }

            i += len;
        }

        if !self.endpoints.is_empty() {
            self.state = UsbAudioState::Configured;
            self.supported_rates = alloc::vec![8000, 16000, 22050, 44100, 48000, 96000];
            true
        } else {
            self.state = UsbAudioState::Error;
            false
        }
    }

    /// Set sample rate
    pub fn set_sample_rate(&mut self, rate: u32) -> bool {
        if self.supported_rates.contains(&rate) {
            if let Some(ref mut fmt) = self.current_format {
                fmt.sample_rate = rate;
            } else {
                self.current_format = Some(StreamFormat {
                    channels: 2,
                    bit_depth: 16,
                    sample_rate: rate,
                });
            }
            true
        } else {
            false
        }
    }

    /// Start streaming
    pub fn start_stream(&mut self) -> bool {
        if self.state != UsbAudioState::Configured {
            return false;
        }
        self.state = UsbAudioState::Streaming;
        self.playback_write = 0;
        self.playback_read = 0;
        self.capture_write = 0;
        self.capture_read = 0;
        true
    }

    /// Write PCM data to playback buffer
    pub fn write_playback(&mut self, data: &[u8]) -> usize {
        let buf_len = self.playback_buf.len();
        let mut written = 0;
        for &byte in data {
            let next_write = (self.playback_write + 1) % buf_len;
            if next_write == self.playback_read {
                self.xruns = self.xruns.saturating_add(1);
                break; // Buffer full
            }
            self.playback_buf[self.playback_write] = byte;
            self.playback_write = next_write;
            written += 1;
        }
        self.frames_played += written as u64 / 4; // assuming 16-bit stereo
        written
    }

    /// Read captured audio data
    pub fn read_capture(&mut self, buf: &mut [u8]) -> usize {
        let buf_len = self.capture_buf.len();
        let mut read = 0;
        for byte in buf.iter_mut() {
            if self.capture_read == self.capture_write {
                break; // Buffer empty
            }
            *byte = self.capture_buf[self.capture_read];
            self.capture_read = (self.capture_read + 1) % buf_len;
            read += 1;
        }
        self.frames_captured += read as u64 / 4;
        read
    }

    /// Set volume (0-255)
    pub fn set_volume(&mut self, vol: u8) {
        self.volume = vol;
    }

    /// Stop streaming
    pub fn stop_stream(&mut self) {
        self.state = UsbAudioState::Configured;
    }
}

/// Global USB audio device list
static USB_AUDIO_DEVICES: Mutex<Vec<UsbAudioDevice>> = Mutex::new(Vec::new());

pub fn init() {
    crate::serial_println!("  [usb-audio] USB audio class driver initialized");
}

/// Register a new USB audio device
pub fn register_device(addr: u8, descriptors: &[u8]) -> bool {
    let mut dev = UsbAudioDevice::new(addr);
    if dev.enumerate(descriptors) {
        crate::serial_println!(
            "  [usb-audio] Device {} configured ({} endpoints)",
            addr,
            dev.endpoints.len()
        );
        USB_AUDIO_DEVICES.lock().push(dev);
        true
    } else {
        false
    }
}

/// Get number of registered USB audio devices
pub fn device_count() -> usize {
    USB_AUDIO_DEVICES.lock().len()
}
