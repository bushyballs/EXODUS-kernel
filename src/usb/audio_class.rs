use crate::sync::Mutex;
/// USB Audio Class (UAC) driver
///
/// Supports UAC1 and UAC2 devices: headsets, microphones, speakers, DACs.
/// Handles isochronous transfers, sample rate negotiation, volume control,
/// and audio format descriptor parsing.
///
/// References: USB Audio Class 1.0 / 2.0 specifications.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static AUDIO_STATE: Mutex<Option<AudioClassState>> = Mutex::new(None);

/// Top-level state for all attached USB audio devices.
pub struct AudioClassState {
    pub devices: Vec<AudioDevice>,
    pub next_stream_id: u32,
}

impl AudioClassState {
    pub fn new() -> Self {
        AudioClassState {
            devices: Vec::new(),
            next_stream_id: 1,
        }
    }

    pub fn register(&mut self, dev: AudioDevice) -> u32 {
        let id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.saturating_add(1);
        self.devices.push(dev);
        id
    }
}

// ---------------------------------------------------------------------------
// Audio Class constants
// ---------------------------------------------------------------------------

/// USB Audio interface class code.
pub const CLASS_AUDIO: u8 = 0x01;

/// Audio subclass codes.
pub const SUBCLASS_AUDIOCONTROL: u8 = 0x01;
pub const SUBCLASS_AUDIOSTREAMING: u8 = 0x02;
pub const SUBCLASS_MIDISTREAMING: u8 = 0x03;

/// Audio class-specific descriptor types.
pub const CS_INTERFACE: u8 = 0x24;
pub const CS_ENDPOINT: u8 = 0x25;

/// Audio Control interface descriptor subtypes (UAC1).
pub const AC_HEADER: u8 = 0x01;
pub const AC_INPUT_TERMINAL: u8 = 0x02;
pub const AC_OUTPUT_TERMINAL: u8 = 0x03;
pub const AC_MIXER_UNIT: u8 = 0x04;
pub const AC_SELECTOR_UNIT: u8 = 0x05;
pub const AC_FEATURE_UNIT: u8 = 0x06;

/// Audio Streaming interface descriptor subtypes.
pub const AS_GENERAL: u8 = 0x01;
pub const AS_FORMAT_TYPE: u8 = 0x02;

/// Feature unit control selectors.
pub const FU_MUTE: u8 = 0x01;
pub const FU_VOLUME: u8 = 0x02;
pub const FU_BASS: u8 = 0x03;
pub const FU_TREBLE: u8 = 0x04;

/// Terminal types.
pub const TT_USB_STREAMING: u16 = 0x0101;
pub const TT_SPEAKER: u16 = 0x0301;
pub const TT_HEADPHONES: u16 = 0x0302;
pub const TT_MICROPHONE: u16 = 0x0201;
pub const TT_HEADSET: u16 = 0x0402;

/// Audio class request codes.
pub const SET_CUR: u8 = 0x01;
pub const GET_CUR: u8 = 0x81;
pub const SET_MIN: u8 = 0x02;
pub const GET_MIN: u8 = 0x82;
pub const SET_MAX: u8 = 0x03;
pub const GET_MAX: u8 = 0x83;
pub const SET_RES: u8 = 0x04;
pub const GET_RES: u8 = 0x84;

// ---------------------------------------------------------------------------
// UAC version enumeration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UacVersion {
    Uac1,
    Uac2,
}

// ---------------------------------------------------------------------------
// Audio formats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Pcm,
    PcmFloat,
    Alaw,
    Mulaw,
    Unknown(u16),
}

impl AudioFormat {
    pub fn from_tag(tag: u16) -> Self {
        match tag {
            0x0001 => AudioFormat::Pcm,
            0x0003 => AudioFormat::PcmFloat,
            0x0006 => AudioFormat::Alaw,
            0x0007 => AudioFormat::Mulaw,
            other => AudioFormat::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDirection {
    Playback,
    Capture,
}

// ---------------------------------------------------------------------------
// Audio stream format
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AudioStreamFormat {
    pub format: AudioFormat,
    pub channels: u8,
    pub bit_depth: u8,
    pub sample_rates: Vec<u32>,
}

impl AudioStreamFormat {
    /// Check if a specific sample rate is supported.
    pub fn supports_rate(&self, rate: u32) -> bool {
        self.sample_rates.iter().any(|&r| r == rate)
    }

    /// Choose the best sample rate from a priority list.
    pub fn best_rate(&self, preferred: &[u32]) -> Option<u32> {
        for &pref in preferred {
            if self.supports_rate(pref) {
                return Some(pref);
            }
        }
        self.sample_rates.first().copied()
    }

    /// Bytes per sample frame (all channels).
    pub fn frame_bytes(&self) -> u32 {
        let bytes_per_sample = ((self.bit_depth + 7) / 8) as u32;
        bytes_per_sample * self.channels as u32
    }
}

// ---------------------------------------------------------------------------
// Terminal descriptor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AudioTerminal {
    pub terminal_id: u8,
    pub terminal_type: u16,
    pub assoc_terminal: u8,
    pub nr_channels: u8,
    pub description: String,
}

impl AudioTerminal {
    pub fn is_input(&self) -> bool {
        // Input terminal types are in range 0x0200-0x02FF
        (self.terminal_type & 0xFF00) == 0x0200 || self.terminal_type == TT_USB_STREAMING
    }

    pub fn type_name(&self) -> &'static str {
        match self.terminal_type {
            TT_USB_STREAMING => "usb-streaming",
            TT_SPEAKER => "speaker",
            TT_HEADPHONES => "headphones",
            TT_MICROPHONE => "microphone",
            TT_HEADSET => "headset",
            _ => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// Feature unit (volume/mute)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FeatureUnit {
    pub unit_id: u8,
    pub source_id: u8,
    pub controls_per_channel: Vec<u16>,
}

impl FeatureUnit {
    /// Check if mute is available on the master channel (channel 0).
    pub fn has_mute(&self) -> bool {
        if let Some(&ctrl) = self.controls_per_channel.first() {
            ctrl & (1 << (FU_MUTE - 1)) != 0
        } else {
            false
        }
    }

    /// Check if volume control is available on the master channel.
    pub fn has_volume(&self) -> bool {
        if let Some(&ctrl) = self.controls_per_channel.first() {
            ctrl & (1 << (FU_VOLUME - 1)) != 0
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Isochronous endpoint state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct IsoEndpoint {
    pub address: u8,
    pub direction: StreamDirection,
    pub max_packet_size: u16,
    pub interval: u8,
    pub sync_type: u8,  // bits 3:2 of bmAttributes
    pub usage_type: u8, // bits 5:4 of bmAttributes
}

impl IsoEndpoint {
    pub fn is_async(&self) -> bool {
        self.sync_type == 0x01
    }

    pub fn is_adaptive(&self) -> bool {
        self.sync_type == 0x02
    }

    pub fn is_sync(&self) -> bool {
        self.sync_type == 0x03
    }
}

// ---------------------------------------------------------------------------
// Volume control (Q16 fixed-point dB)
// ---------------------------------------------------------------------------

/// Volume in Q16.16 fixed-point decibels.
/// UAC uses 1/256 dB resolution internally; we convert to Q16.
#[derive(Debug, Clone, Copy)]
pub struct VolumeControl {
    pub min_db_q16: i32,
    pub max_db_q16: i32,
    pub res_db_q16: i32,
    pub cur_db_q16: i32,
    pub muted: bool,
}

impl VolumeControl {
    pub fn new() -> Self {
        VolumeControl {
            min_db_q16: -96 << 16, // -96 dB
            max_db_q16: 0,         //   0 dB
            res_db_q16: 1 << 16,   //   1 dB
            cur_db_q16: -12 << 16, // -12 dB
            muted: false,
        }
    }

    /// Convert a UAC 8.8 fixed-point dB value to our Q16 representation.
    pub fn from_uac_db(raw: i16) -> i32 {
        (raw as i32) << 8
    }

    /// Convert our Q16 dB value to UAC 8.8 format.
    pub fn to_uac_db(q16: i32) -> i16 {
        (q16 >> 8) as i16
    }

    /// Set volume, clamping to [min, max].
    pub fn set(&mut self, db_q16: i32) {
        if db_q16 < self.min_db_q16 {
            self.cur_db_q16 = self.min_db_q16;
        } else if db_q16 > self.max_db_q16 {
            self.cur_db_q16 = self.max_db_q16;
        } else {
            self.cur_db_q16 = db_q16;
        }
    }

    /// Step volume up by one resolution unit.
    pub fn step_up(&mut self) {
        self.set(self.cur_db_q16 + self.res_db_q16);
    }

    /// Step volume down by one resolution unit.
    pub fn step_down(&mut self) {
        self.set(self.cur_db_q16 - self.res_db_q16);
    }

    /// Compute a linear gain approximation in Q16 from current dB value.
    /// Uses a piece-wise linear approximation; NO floats.
    pub fn linear_gain_q16(&self) -> i32 {
        if self.muted {
            return 0;
        }
        let db = self.cur_db_q16 >> 16; // integer part of dB
        if db >= 0 {
            return 1 << 16; // 1.0 in Q16
        }
        if db <= -96 {
            return 0;
        }
        // Approximate 10^(dB/20) with a linear ramp for simplicity.
        // Map -96 dB..0 dB  -->  0..65536
        let range = 96_i32;
        let offset = db + range; // 0..96
        (offset << 16) / range
    }
}

// ---------------------------------------------------------------------------
// Audio device
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDeviceState {
    Idle,
    Configured,
    Streaming,
    Error,
}

pub struct AudioDevice {
    pub slot_id: u8,
    pub uac_version: UacVersion,
    pub state: AudioDeviceState,
    pub terminals: Vec<AudioTerminal>,
    pub feature_units: Vec<FeatureUnit>,
    pub stream_formats: Vec<AudioStreamFormat>,
    pub iso_endpoints: Vec<IsoEndpoint>,
    pub volume: VolumeControl,
    pub active_sample_rate: u32,
    pub active_channels: u8,
    pub active_bit_depth: u8,
}

impl AudioDevice {
    pub fn new(slot_id: u8, uac_version: UacVersion) -> Self {
        AudioDevice {
            slot_id,
            uac_version,
            state: AudioDeviceState::Idle,
            terminals: Vec::new(),
            feature_units: Vec::new(),
            stream_formats: Vec::new(),
            iso_endpoints: Vec::new(),
            volume: VolumeControl::new(),
            active_sample_rate: 0,
            active_channels: 0,
            active_bit_depth: 0,
        }
    }

    // ----- descriptor parsing -----

    /// Parse an Audio Control interface header.
    pub fn parse_ac_header(&mut self, data: &[u8]) {
        if data.len() < 8 {
            return;
        }
        let bcd = (data[3] as u16) | ((data[4] as u16) << 8);
        self.uac_version = if bcd >= 0x0200 {
            UacVersion::Uac2
        } else {
            UacVersion::Uac1
        };
    }

    /// Parse an Input Terminal descriptor.
    pub fn parse_input_terminal(&mut self, data: &[u8]) {
        if data.len() < 12 {
            return;
        }
        let terminal = AudioTerminal {
            terminal_id: data[3],
            terminal_type: (data[4] as u16) | ((data[5] as u16) << 8),
            assoc_terminal: data[6],
            nr_channels: data[7],
            description: String::new(),
        };
        self.terminals.push(terminal);
    }

    /// Parse an Output Terminal descriptor.
    pub fn parse_output_terminal(&mut self, data: &[u8]) {
        if data.len() < 9 {
            return;
        }
        let terminal = AudioTerminal {
            terminal_id: data[3],
            terminal_type: (data[4] as u16) | ((data[5] as u16) << 8),
            assoc_terminal: data[6],
            nr_channels: 0,
            description: String::new(),
        };
        self.terminals.push(terminal);
    }

    /// Parse a Feature Unit descriptor (UAC1).
    pub fn parse_feature_unit(&mut self, data: &[u8]) {
        if data.len() < 7 {
            return;
        }
        let unit_id = data[3];
        let source_id = data[4];
        let control_size = data[5] as usize;
        if control_size == 0 {
            return;
        }
        let num_channels = (data.len() - 7) / control_size;
        let mut controls = Vec::new();
        for ch in 0..=num_channels {
            let offset = 6 + ch * control_size;
            if offset >= data.len() {
                break;
            }
            let ctrl = if control_size >= 2 && offset + 1 < data.len() {
                (data[offset] as u16) | ((data[offset + 1] as u16) << 8)
            } else {
                data[offset] as u16
            };
            controls.push(ctrl);
        }
        self.feature_units.push(FeatureUnit {
            unit_id,
            source_id,
            controls_per_channel: controls,
        });
    }

    /// Parse an Audio Streaming Format Type I descriptor.
    pub fn parse_format_type(&mut self, data: &[u8]) {
        if data.len() < 8 {
            return;
        }
        let channels = data[4];
        let bit_depth = data[6];
        let num_rates = data[7] as usize;
        let mut rates = Vec::new();
        if num_rates == 0 {
            // Continuous range: min, max (3 bytes each starting at offset 8)
            if data.len() >= 14 {
                let min = (data[8] as u32) | ((data[9] as u32) << 8) | ((data[10] as u32) << 16);
                let max = (data[11] as u32) | ((data[12] as u32) << 8) | ((data[13] as u32) << 16);
                // Populate common rates within range
                let common = [
                    8000, 11025, 16000, 22050, 32000, 44100, 48000, 96000, 192000,
                ];
                for &r in &common {
                    if r >= min && r <= max {
                        rates.push(r);
                    }
                }
            }
        } else {
            for i in 0..num_rates {
                let off = 8 + i * 3;
                if off + 2 < data.len() {
                    let rate = (data[off] as u32)
                        | ((data[off + 1] as u32) << 8)
                        | ((data[off + 2] as u32) << 16);
                    rates.push(rate);
                }
            }
        }
        let fmt_tag = (data[2] as u16) | ((data[3] as u16) << 8);
        self.stream_formats.push(AudioStreamFormat {
            format: AudioFormat::from_tag(fmt_tag),
            channels,
            bit_depth,
            sample_rates: rates,
        });
    }

    /// Register an isochronous endpoint for this device.
    pub fn add_iso_endpoint(&mut self, addr: u8, max_pkt: u16, interval: u8, attrs: u8) {
        let direction = if addr & 0x80 != 0 {
            StreamDirection::Capture
        } else {
            StreamDirection::Playback
        };
        self.iso_endpoints.push(IsoEndpoint {
            address: addr,
            direction,
            max_packet_size: max_pkt,
            interval,
            sync_type: (attrs >> 2) & 0x03,
            usage_type: (attrs >> 4) & 0x03,
        });
    }

    // ----- stream control -----

    /// Negotiate and select the best format for streaming.
    pub fn negotiate_format(&mut self) -> bool {
        let preferred_rates: [u32; 4] = [48000, 44100, 96000, 16000];
        for fmt in &self.stream_formats {
            if let Some(rate) = fmt.best_rate(&preferred_rates) {
                self.active_sample_rate = rate;
                self.active_channels = fmt.channels;
                self.active_bit_depth = fmt.bit_depth;
                self.state = AudioDeviceState::Configured;
                return true;
            }
        }
        false
    }

    /// Build a SET_CUR sample rate control request payload (UAC1, 3 bytes).
    pub fn build_set_sample_rate(&self) -> [u8; 3] {
        let r = self.active_sample_rate;
        [
            (r & 0xFF) as u8,
            ((r >> 8) & 0xFF) as u8,
            ((r >> 16) & 0xFF) as u8,
        ]
    }

    /// Build a SET_CUR volume control request (UAC1, 2 bytes little-endian).
    pub fn build_set_volume(&self) -> [u8; 2] {
        let raw = VolumeControl::to_uac_db(self.volume.cur_db_q16);
        [(raw & 0xFF) as u8, ((raw >> 8) & 0xFF) as u8]
    }

    /// Build a SET_CUR mute control request (UAC1, 1 byte).
    pub fn build_set_mute(&self) -> [u8; 1] {
        [if self.volume.muted { 1 } else { 0 }]
    }

    /// Compute how many bytes per millisecond the active format needs.
    pub fn bytes_per_ms(&self) -> u32 {
        if self.active_sample_rate == 0 {
            return 0;
        }
        let bytes_per_sample = ((self.active_bit_depth + 7) / 8) as u32;
        let frame = bytes_per_sample * self.active_channels as u32;
        // rate / 1000 * frame_bytes, but avoid truncation
        (self.active_sample_rate / 1000) * frame
    }

    /// Determine the required isochronous transfer size for one interval.
    pub fn iso_transfer_size(&self, interval_ms: u32) -> u32 {
        self.bytes_per_ms() * interval_ms
    }

    /// Transition to streaming state.
    pub fn start_streaming(&mut self) -> bool {
        if self.state != AudioDeviceState::Configured {
            return false;
        }
        self.state = AudioDeviceState::Streaming;
        true
    }

    /// Stop streaming.
    pub fn stop_streaming(&mut self) {
        if self.state == AudioDeviceState::Streaming {
            self.state = AudioDeviceState::Configured;
        }
    }
}

// ---------------------------------------------------------------------------
// Class identification
// ---------------------------------------------------------------------------

/// Check if a USB interface is an audio control interface.
pub fn is_audio_control(class: u8, subclass: u8) -> bool {
    class == CLASS_AUDIO && subclass == SUBCLASS_AUDIOCONTROL
}

/// Check if a USB interface is an audio streaming interface.
pub fn is_audio_streaming(class: u8, subclass: u8) -> bool {
    class == CLASS_AUDIO && subclass == SUBCLASS_AUDIOSTREAMING
}

/// Check if a USB interface is a MIDI streaming interface.
pub fn is_midi_streaming(class: u8, subclass: u8) -> bool {
    class == CLASS_AUDIO && subclass == SUBCLASS_MIDISTREAMING
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut state = AUDIO_STATE.lock();
    *state = Some(AudioClassState::new());
    serial_println!("    [audio] USB Audio Class driver loaded (UAC1/UAC2)");
}
