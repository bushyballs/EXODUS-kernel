/// Advanced Audio Distribution Profile (A2DP).
///
/// A2DP provides high-quality audio streaming over Bluetooth.
/// This module implements:
///   - AVDTP (Audio/Video Distribution Transport Protocol) signaling
///   - Stream endpoint discovery and configuration
///   - Codec negotiation (SBC mandatory, AAC/aptX/LDAC optional)
///   - Audio stream state machine (Idle->Configured->Open->Streaming)
///   - Media transport channel management
///
/// SBC (Sub-Band Coding) is the mandatory codec:
///   - Sampling: 16/32/44.1/48 kHz
///   - Channels: Mono, Dual, Stereo, Joint Stereo
///   - Blocks: 4, 8, 12, 16
///   - Subbands: 4 or 8
///   - Allocation: SNR or Loudness
///   - Bitpool: 2..250 (quality vs bitrate tradeoff)
///
/// Part of the AIOS bluetooth subsystem.

use alloc::vec::Vec;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// L2CAP PSM for AVDTP.
const PSM_AVDTP: u16 = 0x0019;

/// AVDTP signaling message types.
const AVDTP_DISCOVER: u8 = 0x01;
const AVDTP_GET_CAPABILITIES: u8 = 0x02;
const AVDTP_SET_CONFIG: u8 = 0x03;
const AVDTP_GET_CONFIG: u8 = 0x04;
const AVDTP_RECONFIGURE: u8 = 0x05;
const AVDTP_OPEN: u8 = 0x06;
const AVDTP_START: u8 = 0x07;
const AVDTP_CLOSE: u8 = 0x08;
const AVDTP_SUSPEND: u8 = 0x09;
const AVDTP_ABORT: u8 = 0x0A;

/// AVDTP service category IDs.
const CAT_MEDIA_TRANSPORT: u8 = 0x01;
const CAT_REPORTING: u8 = 0x02;
const CAT_RECOVERY: u8 = 0x03;
const CAT_MEDIA_CODEC: u8 = 0x07;

/// Media types.
const MEDIA_TYPE_AUDIO: u8 = 0x00;
const MEDIA_TYPE_VIDEO: u8 = 0x01;

/// Codec types.
const CODEC_SBC: u8 = 0x00;
const CODEC_MPEG_AAC: u8 = 0x02;
const CODEC_VENDOR: u8 = 0xFF; // aptX, LDAC use vendor-specific

/// SEP (Stream Endpoint) types.
const SEP_SOURCE: u8 = 0x00;
const SEP_SINK: u8 = 0x01;

/// SBC codec configuration constants.
const SBC_SAMPLING_44100: u8 = 0x20;
const SBC_SAMPLING_48000: u8 = 0x10;
const SBC_CHANNEL_JOINT_STEREO: u8 = 0x01;
const SBC_CHANNEL_STEREO: u8 = 0x02;
const SBC_BLOCKS_16: u8 = 0x10;
const SBC_SUBBANDS_8: u8 = 0x04;
const SBC_ALLOC_LOUDNESS: u8 = 0x01;
const SBC_MIN_BITPOOL: u8 = 2;
const SBC_MAX_BITPOOL: u8 = 53; // High quality

/// Global A2DP state.
static A2DP: Mutex<Option<A2dpManager>> = Mutex::new(None);

/// Supported A2DP codec types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum A2dpCodec {
    Sbc,
    Aac,
    AptX,
    Ldac,
}

/// A2DP stream states.
#[derive(Debug, Clone, Copy, PartialEq)]
enum StreamState {
    Idle,
    Configured,
    Open,
    Streaming,
    Closing,
    Aborting,
}

/// SBC codec configuration.
#[derive(Debug, Clone, Copy)]
struct SbcConfig {
    sampling_freq: u8,
    channel_mode: u8,
    block_length: u8,
    subbands: u8,
    allocation_method: u8,
    min_bitpool: u8,
    max_bitpool: u8,
}

impl SbcConfig {
    fn default_high_quality() -> Self {
        Self {
            sampling_freq: SBC_SAMPLING_44100,
            channel_mode: SBC_CHANNEL_JOINT_STEREO,
            block_length: SBC_BLOCKS_16,
            subbands: SBC_SUBBANDS_8,
            allocation_method: SBC_ALLOC_LOUDNESS,
            min_bitpool: SBC_MIN_BITPOOL,
            max_bitpool: SBC_MAX_BITPOOL,
        }
    }

    /// Encode SBC capabilities into 4 bytes (for AVDTP service capabilities).
    fn to_bytes(&self) -> [u8; 4] {
        [
            self.sampling_freq | self.channel_mode,
            self.block_length | self.subbands | self.allocation_method,
            self.min_bitpool,
            self.max_bitpool,
        ]
    }
}

/// Stream Endpoint information.
struct StreamEndpoint {
    seid: u8,               // Stream Endpoint Identifier (1..63)
    sep_type: u8,           // Source or Sink
    codec: A2dpCodec,
    sbc_config: SbcConfig,
    state: StreamState,
    media_transport_cid: u16,
    in_use: bool,
}

impl StreamEndpoint {
    fn new(seid: u8, sep_type: u8, codec: A2dpCodec) -> Self {
        Self {
            seid,
            sep_type,
            codec,
            sbc_config: SbcConfig::default_high_quality(),
            state: StreamState::Idle,
            media_transport_cid: 0,
            in_use: false,
        }
    }
}

/// A2DP manager handling AVDTP signaling and stream endpoints.
struct A2dpManager {
    endpoints: Vec<StreamEndpoint>,
    next_seid: u8,
    transaction_label: u8,
}

impl A2dpManager {
    fn new() -> Self {
        Self {
            endpoints: Vec::new(),
            next_seid: 1,
            transaction_label: 0,
        }
    }

    /// Get the next AVDTP transaction label.
    fn next_label(&mut self) -> u8 {
        let label = self.transaction_label;
        self.transaction_label = (self.transaction_label + 1) & 0x0F;
        label
    }

    /// Register a stream endpoint (source or sink).
    fn register_endpoint(&mut self, sep_type: u8, codec: A2dpCodec) -> u8 {
        let seid = self.next_seid;
        self.next_seid = self.next_seid.saturating_add(1);

        let ep = StreamEndpoint::new(seid, sep_type, codec);
        serial_println!("    [a2dp] Registered {} endpoint SEID={} codec={:?}",
            if sep_type == SEP_SOURCE { "source" } else { "sink" },
            seid, codec);

        self.endpoints.push(ep);
        seid
    }

    /// Find an endpoint by SEID.
    fn find_endpoint(&mut self, seid: u8) -> Option<&mut StreamEndpoint> {
        self.endpoints.iter_mut().find(|ep| ep.seid == seid)
    }

    /// Configure a stream endpoint.
    fn configure(&mut self, seid: u8, codec: A2dpCodec) -> bool {
        if let Some(ep) = self.find_endpoint(seid) {
            if ep.state != StreamState::Idle {
                serial_println!("    [a2dp] SEID {} not in Idle state", seid);
                return false;
            }
            ep.codec = codec;
            ep.state = StreamState::Configured;
            serial_println!("    [a2dp] SEID {} configured with {:?}", seid, codec);
            true
        } else {
            false
        }
    }

    /// Open a configured stream.
    fn open(&mut self, seid: u8) -> bool {
        if let Some(ep) = self.find_endpoint(seid) {
            if ep.state != StreamState::Configured {
                serial_println!("    [a2dp] SEID {} not configured", seid);
                return false;
            }
            ep.state = StreamState::Open;
            serial_println!("    [a2dp] SEID {} opened", seid);
            true
        } else {
            false
        }
    }

    /// Start streaming on an open endpoint.
    fn start_stream(&mut self, seid: u8) -> bool {
        if let Some(ep) = self.find_endpoint(seid) {
            if ep.state != StreamState::Open {
                serial_println!("    [a2dp] SEID {} not open", seid);
                return false;
            }
            ep.state = StreamState::Streaming;
            ep.in_use = true;
            serial_println!("    [a2dp] SEID {} streaming started ({:?})", seid, ep.codec);
            true
        } else {
            false
        }
    }

    /// Suspend a streaming endpoint.
    fn suspend(&mut self, seid: u8) -> bool {
        if let Some(ep) = self.find_endpoint(seid) {
            if ep.state != StreamState::Streaming {
                return false;
            }
            ep.state = StreamState::Open;
            serial_println!("    [a2dp] SEID {} suspended", seid);
            true
        } else {
            false
        }
    }

    /// Stop streaming and close the endpoint.
    fn stop_stream(&mut self, seid: u8) -> bool {
        if let Some(ep) = self.find_endpoint(seid) {
            ep.state = StreamState::Idle;
            ep.in_use = false;
            serial_println!("    [a2dp] SEID {} stopped", seid);
            true
        } else {
            false
        }
    }

    /// Handle an incoming AVDTP signaling message.
    fn handle_signal(&mut self, data: &[u8]) -> Vec<u8> {
        if data.len() < 2 {
            return Vec::new();
        }

        let _label = (data[0] >> 4) & 0x0F;
        let _msg_type = data[0] & 0x03; // 0=command, 2=response accept, 3=response reject
        let signal_id = data[1] & 0x3F;

        match signal_id {
            AVDTP_DISCOVER => {
                // Return list of SEIDs.
                let mut rsp = Vec::new();
                rsp.push((self.next_label() << 4) | 0x02); // response accept
                rsp.push(AVDTP_DISCOVER);
                for ep in &self.endpoints {
                    let seid_byte = (ep.seid << 2) | if ep.in_use { 0x02 } else { 0x00 };
                    let type_byte = (ep.sep_type << 3) | MEDIA_TYPE_AUDIO;
                    rsp.push(seid_byte);
                    rsp.push(type_byte);
                }
                rsp
            }
            AVDTP_GET_CAPABILITIES => {
                if data.len() >= 3 {
                    let seid = data[2] >> 2;
                    let mut rsp = Vec::new();
                    rsp.push((self.next_label() << 4) | 0x02);
                    rsp.push(AVDTP_GET_CAPABILITIES);
                    // Media Transport capability.
                    rsp.push(CAT_MEDIA_TRANSPORT);
                    rsp.push(0); // length
                    // Media Codec capability.
                    rsp.push(CAT_MEDIA_CODEC);
                    rsp.push(6); // length: media_type(1) + codec_type(1) + sbc_config(4)
                    rsp.push(MEDIA_TYPE_AUDIO << 4);
                    rsp.push(CODEC_SBC);
                    let config = SbcConfig::default_high_quality();
                    let config_bytes = config.to_bytes();
                    rsp.extend_from_slice(&config_bytes);
                    let _ = seid;
                    rsp
                } else {
                    Vec::new()
                }
            }
            _ => {
                serial_println!("    [a2dp] Unhandled AVDTP signal {:#04x}", signal_id);
                Vec::new()
            }
        }
    }
}

/// A2DP audio stream endpoint.
pub struct A2dpEndpoint {
    seid: u8,
    codec: A2dpCodec,
}

impl A2dpEndpoint {
    pub fn new(codec: A2dpCodec) -> Self {
        let seid = if let Some(mgr) = A2DP.lock().as_mut() {
            let s = mgr.register_endpoint(SEP_SOURCE, codec);
            mgr.configure(s, codec);
            mgr.open(s);
            s
        } else {
            0
        };

        Self { seid, codec }
    }

    /// Start streaming audio to a connected sink.
    pub fn start_stream(&mut self) {
        if let Some(mgr) = A2DP.lock().as_mut() {
            mgr.start_stream(self.seid);
        }
    }

    /// Stop the audio stream.
    pub fn stop_stream(&mut self) {
        if let Some(mgr) = A2DP.lock().as_mut() {
            mgr.stop_stream(self.seid);
        }
    }
}

/// Handle incoming AVDTP signaling data.
pub fn handle_avdtp(data: &[u8]) -> Vec<u8> {
    if let Some(mgr) = A2DP.lock().as_mut() {
        mgr.handle_signal(data)
    } else {
        Vec::new()
    }
}

pub fn init() {
    let mut mgr = A2dpManager::new();

    serial_println!("    [a2dp] Initializing A2DP profile");

    // Register default source and sink endpoints with SBC codec.
    mgr.register_endpoint(SEP_SOURCE, A2dpCodec::Sbc);
    mgr.register_endpoint(SEP_SINK, A2dpCodec::Sbc);

    *A2DP.lock() = Some(mgr);
    serial_println!("    [a2dp] A2DP initialized (SBC source + sink endpoints)");
}
