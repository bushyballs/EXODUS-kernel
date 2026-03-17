/// Bluetooth LE Audio (LC3 codec, broadcast/unicast).
///
/// LE Audio is the next-generation Bluetooth audio standard featuring:
///   - LC3 codec (Low Complexity Communication Codec)
///   - Isochronous channels (CIS for unicast, BIS for broadcast)
///   - CIG (Connected Isochronous Group) for unicast streams
///   - BIG (Broadcast Isochronous Group) for broadcast streams
///   - BAP (Basic Audio Profile) for stream management
///   - Auracast broadcast audio sharing
///
/// ISO channel parameters:
///   - SDU interval: 7.5ms or 10ms
///   - Max SDU size: depends on codec config (typically 40-155 bytes)
///   - Framing: unframed or framed
///   - PHY: 1M, 2M, or Coded
///   - Retransmission: 0..15
///   - Transport latency: depends on use case
///
/// LC3 codec configurations (from BAP):
///   - 8_1:   8kHz,  7.5ms, 26 octets
///   - 8_2:   8kHz,  10ms,  30 octets
///   - 16_1: 16kHz,  7.5ms, 30 octets
///   - 16_2: 16kHz,  10ms,  40 octets
///   - 24_1: 24kHz,  7.5ms, 45 octets
///   - 24_2: 24kHz,  10ms,  60 octets
///   - 32_1: 32kHz,  7.5ms, 60 octets
///   - 32_2: 32kHz,  10ms,  80 octets
///   - 48_1: 48kHz,  7.5ms, 75 octets
///   - 48_2: 48kHz,  10ms, 100 octets
///
/// Part of the AIOS bluetooth subsystem.

use alloc::vec::Vec;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// HCI ISO data path directions.
const ISO_DATA_PATH_HCI: u8 = 0x00;
const ISO_DATA_PATH_VENDOR: u8 = 0x01;

/// LC3 codec ID (Bluetooth SIG assigned).
const CODEC_LC3: u8 = 0x06;

/// Sampling frequencies supported by LC3.
const FREQ_8KHZ: u8 = 0x01;
const FREQ_16KHZ: u8 = 0x03;
const FREQ_24KHZ: u8 = 0x05;
const FREQ_32KHZ: u8 = 0x06;
const FREQ_44_1KHZ: u8 = 0x07;
const FREQ_48KHZ: u8 = 0x08;

/// Frame durations.
const FRAME_7_5MS: u8 = 0x00;
const FRAME_10MS: u8 = 0x01;

/// Audio locations (channel allocation).
const LOCATION_FRONT_LEFT: u32 = 0x00000001;
const LOCATION_FRONT_RIGHT: u32 = 0x00000002;
const LOCATION_MONO: u32 = 0x00000000;

/// Global LE Audio state.
static LE_AUDIO: Mutex<Option<LeAudioManager>> = Mutex::new(None);

/// LE Audio stream direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StreamDirection {
    Unicast,
    Broadcast,
}

/// LC3 codec configuration.
#[derive(Debug, Clone, Copy)]
struct Lc3Config {
    sampling_freq: u8,
    frame_duration: u8,
    octets_per_frame: u16,
    audio_locations: u32,
    frames_per_sdu: u8,
}

impl Lc3Config {
    /// Default high-quality config: 48kHz, 10ms, 100 octets.
    fn default_48_2() -> Self {
        Self {
            sampling_freq: FREQ_48KHZ,
            frame_duration: FRAME_10MS,
            octets_per_frame: 100,
            audio_locations: LOCATION_FRONT_LEFT | LOCATION_FRONT_RIGHT,
            frames_per_sdu: 1,
        }
    }

    /// Conversational config: 16kHz, 10ms, 40 octets.
    fn default_16_2() -> Self {
        Self {
            sampling_freq: FREQ_16KHZ,
            frame_duration: FRAME_10MS,
            octets_per_frame: 40,
            audio_locations: LOCATION_MONO,
            frames_per_sdu: 1,
        }
    }

    /// SDU interval in microseconds.
    fn sdu_interval_us(&self) -> u32 {
        match self.frame_duration {
            FRAME_7_5MS => 7500,
            FRAME_10MS => 10000,
            _ => 10000,
        }
    }

    /// Max SDU size in bytes.
    fn max_sdu_size(&self) -> u16 {
        self.octets_per_frame * self.frames_per_sdu as u16
    }
}

/// Stream state machine.
#[derive(Debug, Clone, Copy, PartialEq)]
enum StreamState {
    Idle,
    Configured,
    QosConfigured,
    Enabling,
    Streaming,
    Disabling,
    Releasing,
}

/// CIS (Connected Isochronous Stream) parameters.
struct CisParams {
    cis_id: u8,
    cig_id: u8,
    sdu_interval_us: u32,
    max_sdu: u16,
    max_transport_latency_ms: u16,
    rtn: u8,    // retransmission number
    phy: u8,    // 1=1M, 2=2M, 3=Coded
}

impl CisParams {
    fn default_unicast(cig_id: u8, cis_id: u8, config: &Lc3Config) -> Self {
        Self {
            cis_id,
            cig_id,
            sdu_interval_us: config.sdu_interval_us(),
            max_sdu: config.max_sdu_size(),
            max_transport_latency_ms: 20,
            rtn: 2,
            phy: 2, // 2M PHY
        }
    }
}

/// BIS (Broadcast Isochronous Stream) parameters.
struct BisParams {
    big_handle: u8,
    num_bis: u8,
    sdu_interval_us: u32,
    max_sdu: u16,
    max_transport_latency_ms: u16,
    rtn: u8,
    phy: u8,
    encryption: bool,
    broadcast_code: [u8; 16],
}

impl BisParams {
    fn default_broadcast(big_handle: u8, config: &Lc3Config) -> Self {
        Self {
            big_handle,
            num_bis: 2, // stereo
            sdu_interval_us: config.sdu_interval_us(),
            max_sdu: config.max_sdu_size(),
            max_transport_latency_ms: 40,
            rtn: 2,
            phy: 2,
            encryption: false,
            broadcast_code: [0u8; 16],
        }
    }
}

/// Per-stream state.
struct LeAudioStreamInner {
    direction: StreamDirection,
    config: Lc3Config,
    state: StreamState,
    cis: Option<CisParams>,
    bis: Option<BisParams>,
    handle: u16,
}

impl LeAudioStreamInner {
    fn new(direction: StreamDirection) -> Self {
        let config = Lc3Config::default_48_2();
        Self {
            direction,
            config,
            state: StreamState::Idle,
            cis: match direction {
                StreamDirection::Unicast => Some(CisParams::default_unicast(0, 0, &config)),
                StreamDirection::Broadcast => None,
            },
            bis: match direction {
                StreamDirection::Broadcast => Some(BisParams::default_broadcast(0, &config)),
                StreamDirection::Unicast => None,
            },
            handle: 0,
        }
    }

    /// Configure the stream with the given LC3 config.
    fn configure(&mut self, config: Lc3Config) {
        self.config = config;
        // Update ISO params.
        match self.direction {
            StreamDirection::Unicast => {
                if let Some(cis) = &mut self.cis {
                    cis.sdu_interval_us = config.sdu_interval_us();
                    cis.max_sdu = config.max_sdu_size();
                }
            }
            StreamDirection::Broadcast => {
                if let Some(bis) = &mut self.bis {
                    bis.sdu_interval_us = config.sdu_interval_us();
                    bis.max_sdu = config.max_sdu_size();
                }
            }
        }
        self.state = StreamState::Configured;
    }
}

/// LE Audio manager.
struct LeAudioManager {
    streams: Vec<LeAudioStreamInner>,
    next_cig_id: u8,
    next_big_handle: u8,
}

impl LeAudioManager {
    fn new() -> Self {
        Self {
            streams: Vec::new(),
            next_cig_id: 0,
            next_big_handle: 0,
        }
    }

    /// Create a new stream.
    fn create_stream(&mut self, direction: StreamDirection) -> usize {
        let mut stream = LeAudioStreamInner::new(direction);

        match direction {
            StreamDirection::Unicast => {
                if let Some(cis) = &mut stream.cis {
                    cis.cig_id = self.next_cig_id;
                    cis.cis_id = 0;
                    self.next_cig_id = self.next_cig_id.saturating_add(1);
                }
            }
            StreamDirection::Broadcast => {
                if let Some(bis) = &mut stream.bis {
                    bis.big_handle = self.next_big_handle;
                    self.next_big_handle = self.next_big_handle.saturating_add(1);
                }
            }
        }

        let idx = self.streams.len();
        serial_println!("    [le_audio] Created {:?} stream (idx={})", direction, idx);
        self.streams.push(stream);
        idx
    }

    /// Start a stream by index.
    fn start_stream(&mut self, idx: usize) {
        if let Some(stream) = self.streams.get_mut(idx) {
            if stream.state == StreamState::Idle {
                stream.configure(Lc3Config::default_48_2());
            }

            match stream.direction {
                StreamDirection::Unicast => {
                    // In a full stack: Create CIG, Create CIS, Setup ISO Data Path.
                    stream.state = StreamState::Streaming;
                    serial_println!("    [le_audio] Unicast stream {} started (LC3 48kHz/10ms/100oct)",
                        idx);
                }
                StreamDirection::Broadcast => {
                    // In a full stack: Create BIG, Start Periodic Advertising.
                    stream.state = StreamState::Streaming;
                    serial_println!("    [le_audio] Broadcast stream {} started (LC3 48kHz/10ms/100oct)",
                        idx);
                }
            }
        }
    }

    /// Stop a stream by index.
    fn stop_stream(&mut self, idx: usize) {
        if let Some(stream) = self.streams.get_mut(idx) {
            let prev_state = stream.state;
            stream.state = StreamState::Idle;

            if prev_state == StreamState::Streaming {
                match stream.direction {
                    StreamDirection::Unicast => {
                        serial_println!("    [le_audio] Unicast stream {} stopped", idx);
                    }
                    StreamDirection::Broadcast => {
                        serial_println!("    [le_audio] Broadcast stream {} stopped", idx);
                    }
                }
            }
        }
    }
}

/// LE Audio stream using the LC3 codec.
pub struct LeAudioStream {
    idx: usize,
    direction: StreamDirection,
}

impl LeAudioStream {
    pub fn new(direction: StreamDirection) -> Self {
        let idx = if let Some(mgr) = LE_AUDIO.lock().as_mut() {
            mgr.create_stream(direction)
        } else {
            0
        };
        Self { idx, direction }
    }

    /// Start the LE Audio stream.
    pub fn start(&mut self) {
        if let Some(mgr) = LE_AUDIO.lock().as_mut() {
            mgr.start_stream(self.idx);
        }
    }

    /// Stop the LE Audio stream.
    pub fn stop(&mut self) {
        if let Some(mgr) = LE_AUDIO.lock().as_mut() {
            mgr.stop_stream(self.idx);
        }
    }
}

pub fn init() {
    let mgr = LeAudioManager::new();

    serial_println!("    [le_audio] Initializing LE Audio subsystem (LC3 codec)");

    *LE_AUDIO.lock() = Some(mgr);
    serial_println!("    [le_audio] LE Audio initialized (ISO transport ready)");
}
