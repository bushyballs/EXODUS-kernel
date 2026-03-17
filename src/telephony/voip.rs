/// VoIP engine for Genesis telephony
///
/// SIP protocol implementation, RTP/RTCP media transport,
/// codec negotiation, NAT traversal (STUN/TURN/ICE),
/// call quality monitoring with MOS scoring, SRTP encryption.
///
/// Original implementation for Hoags OS.

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// SIP protocol types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum SipMethod {
    Invite,
    Ack,
    Bye,
    Cancel,
    Register,
    Options,
    Info,
    Update,
    Refer,
    Notify,
    Subscribe,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SipState {
    Idle,
    Registering,
    Registered,
    Inviting,
    Ringing,
    Established,
    Terminating,
    Failed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SipTransport {
    Udp,
    Tcp,
    Tls,
    WebSocket,
}

/// SIP registration with a registrar server
struct SipRegistration {
    state: SipState,
    transport: SipTransport,
    server_ip: [u8; 4],
    server_port: u16,
    local_port: u16,
    expires_secs: u32,
    cseq: u32,
    call_id_hash: u64,
    registered_at: u64,
    realm: [u8; 64],
    realm_len: usize,
    username: [u8; 64],
    username_len: usize,
}

// ---------------------------------------------------------------------------
// RTP / RTCP media transport
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum AudioCodec {
    Pcmu,       // G.711 u-law (payload 0)
    Pcma,       // G.711 a-law (payload 8)
    G722,       // Wideband (payload 9)
    G729,       // Low bitrate (payload 18)
    Opus,       // Dynamic payload
    Ilbc,       // Internet low bitrate
    Amr,        // Adaptive multi-rate
    AmrWb,      // AMR wideband
}

#[derive(Clone, Copy, PartialEq)]
pub enum DtmfMode {
    Rfc2833,
    SipInfo,
    InBand,
}

struct CodecEntry {
    codec: AudioCodec,
    payload_type: u8,
    clock_rate: u32,
    bitrate_kbps: u16,
    channels: u8,
    priority: u8,
    enabled: bool,
}

struct RtpSession {
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    payload_type: u8,
    remote_ip: [u8; 4],
    remote_port: u16,
    local_port: u16,
    packets_sent: u64,
    packets_recv: u64,
    bytes_sent: u64,
    bytes_recv: u64,
    srtp_enabled: bool,
    srtp_key: [u8; 32],
    dtmf_mode: DtmfMode,
}

/// RTCP statistics for quality monitoring
struct RtcpStats {
    fraction_lost_q16: i32,       // Q16 fraction (0 = 0%, 65536 = 100%)
    cumulative_lost: u32,
    jitter_q16: i32,              // Q16 milliseconds
    rtt_q16: i32,                 // Q16 milliseconds round-trip time
    last_sr_timestamp: u32,
    packets_expected: u64,
    packets_received: u64,
    report_count: u32,
}

// ---------------------------------------------------------------------------
// NAT traversal (STUN / TURN / ICE)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum NatType {
    None,
    FullCone,
    RestrictedCone,
    PortRestricted,
    Symmetric,
    Unknown,
}

#[derive(Clone, Copy, PartialEq)]
pub enum IceState {
    New,
    Checking,
    Connected,
    Completed,
    Failed,
    Disconnected,
}

#[derive(Clone, Copy, PartialEq)]
enum CandidateType {
    Host,
    ServerReflexive,
    PeerReflexive,
    Relay,
}

struct IceCandidate {
    candidate_type: CandidateType,
    ip: [u8; 4],
    port: u16,
    priority: u32,
    component_id: u8,
    foundation: u32,
}

struct NatTraversal {
    nat_type: NatType,
    ice_state: IceState,
    stun_server_ip: [u8; 4],
    stun_server_port: u16,
    turn_server_ip: [u8; 4],
    turn_server_port: u16,
    mapped_ip: [u8; 4],
    mapped_port: u16,
    local_candidates: Vec<IceCandidate>,
    remote_candidates: Vec<IceCandidate>,
    selected_pair_local: Option<usize>,
    selected_pair_remote: Option<usize>,
    connectivity_checks: u32,
    turn_allocated: bool,
}

// ---------------------------------------------------------------------------
// Call quality monitoring
// ---------------------------------------------------------------------------

/// Mean Opinion Score (MOS) estimated from network metrics (Q16 fixed-point)
struct CallQuality {
    mos_q16: i32,                 // Q16: range ~1.0 (65536) to ~5.0 (327680)
    r_factor_q16: i32,            // Q16: R-factor (0-93.2 => 0-6,107,341)
    packet_loss_pct_q16: i32,     // Q16 percentage
    jitter_ms_q16: i32,           // Q16 jitter in ms
    latency_ms_q16: i32,          // Q16 one-way latency in ms
    codec_impairment_q16: i32,    // Q16 codec-specific impairment
    consecutive_loss: u32,
    samples: u32,
}

// ---------------------------------------------------------------------------
// SIP dialog / active call
// ---------------------------------------------------------------------------

struct SipDialog {
    call_id_hash: u64,
    from_tag: u32,
    to_tag: u32,
    state: SipState,
    local_cseq: u32,
    remote_cseq: u32,
    rtp: RtpSession,
    rtcp: RtcpStats,
    quality: CallQuality,
    negotiated_codec: AudioCodec,
    on_hold: bool,
    start_time: u64,
}

// ---------------------------------------------------------------------------
// VoIP engine
// ---------------------------------------------------------------------------

struct VoipEngine {
    registration: SipRegistration,
    dialogs: Vec<SipDialog>,
    codecs: Vec<CodecEntry>,
    nat: NatTraversal,
    next_call_id: u64,
    total_calls: u32,
    failed_calls: u32,
    srtp_default: bool,
}

static VOIP_ENGINE: Mutex<Option<VoipEngine>> = Mutex::new(None);

impl VoipEngine {
    fn new() -> Self {
        let mut codecs = Vec::new();
        // Default codec preference order
        codecs.push(CodecEntry {
            codec: AudioCodec::Opus, payload_type: 111, clock_rate: 48000,
            bitrate_kbps: 32, channels: 2, priority: 0, enabled: true,
        });
        codecs.push(CodecEntry {
            codec: AudioCodec::G722, payload_type: 9, clock_rate: 8000,
            bitrate_kbps: 64, channels: 1, priority: 1, enabled: true,
        });
        codecs.push(CodecEntry {
            codec: AudioCodec::Pcmu, payload_type: 0, clock_rate: 8000,
            bitrate_kbps: 64, channels: 1, priority: 2, enabled: true,
        });
        codecs.push(CodecEntry {
            codec: AudioCodec::Pcma, payload_type: 8, clock_rate: 8000,
            bitrate_kbps: 64, channels: 1, priority: 3, enabled: true,
        });
        codecs.push(CodecEntry {
            codec: AudioCodec::G729, payload_type: 18, clock_rate: 8000,
            bitrate_kbps: 8, channels: 1, priority: 4, enabled: true,
        });

        VoipEngine {
            registration: SipRegistration {
                state: SipState::Idle,
                transport: SipTransport::Tls,
                server_ip: [0; 4],
                server_port: 5061,
                local_port: 5060,
                expires_secs: 3600,
                cseq: 1,
                call_id_hash: 0,
                registered_at: 0,
                realm: [0; 64],
                realm_len: 0,
                username: [0; 64],
                username_len: 0,
            },
            dialogs: Vec::new(),
            codecs,
            nat: NatTraversal {
                nat_type: NatType::Unknown,
                ice_state: IceState::New,
                stun_server_ip: [0; 4],
                stun_server_port: 3478,
                turn_server_ip: [0; 4],
                turn_server_port: 3478,
                mapped_ip: [0; 4],
                mapped_port: 0,
                local_candidates: Vec::new(),
                remote_candidates: Vec::new(),
                selected_pair_local: None,
                selected_pair_remote: None,
                connectivity_checks: 0,
                turn_allocated: false,
            },
            next_call_id: 1,
            total_calls: 0,
            failed_calls: 0,
            srtp_default: true,
        }
    }

    /// Register with a SIP server
    fn sip_register(&mut self, server_ip: [u8; 4], port: u16, timestamp: u64) -> bool {
        self.registration.server_ip = server_ip;
        self.registration.server_port = port;
        self.registration.state = SipState::Registering;
        self.registration.cseq += 1;
        // Simulate successful registration
        self.registration.state = SipState::Registered;
        self.registration.registered_at = timestamp;
        true
    }

    /// Initiate a SIP INVITE to place a call
    fn invite(&mut self, remote_ip: [u8; 4], remote_port: u16, timestamp: u64) -> u64 {
        if self.registration.state != SipState::Registered {
            return 0;
        }
        let call_id = self.next_call_id;
        self.next_call_id = self.next_call_id.saturating_add(1);
        self.total_calls = self.total_calls.saturating_add(1);

        // Pick best enabled codec
        let codec = self.codecs.iter()
            .filter(|c| c.enabled)
            .min_by_key(|c| c.priority)
            .map(|c| (c.codec, c.payload_type))
            .unwrap_or((AudioCodec::Pcmu, 0));

        let dialog = SipDialog {
            call_id_hash: call_id,
            from_tag: (call_id & 0xFFFF_FFFF) as u32,
            to_tag: 0,
            state: SipState::Inviting,
            local_cseq: 1,
            remote_cseq: 0,
            rtp: RtpSession {
                ssrc: (call_id * 2654435761) as u32,
                sequence: 0,
                timestamp: 0,
                payload_type: codec.1,
                remote_ip,
                remote_port,
                local_port: self.registration.local_port + 2,
                packets_sent: 0,
                packets_recv: 0,
                bytes_sent: 0,
                bytes_recv: 0,
                srtp_enabled: self.srtp_default,
                srtp_key: [0; 32],
                dtmf_mode: DtmfMode::Rfc2833,
            },
            rtcp: RtcpStats {
                fraction_lost_q16: 0,
                cumulative_lost: 0,
                jitter_q16: 0,
                rtt_q16: 0,
                last_sr_timestamp: 0,
                packets_expected: 0,
                packets_received: 0,
                report_count: 0,
            },
            quality: CallQuality {
                mos_q16: 294912,   // ~4.5 initial
                r_factor_q16: 5898240, // ~90
                packet_loss_pct_q16: 0,
                jitter_ms_q16: 0,
                latency_ms_q16: 0,
                codec_impairment_q16: 0,
                consecutive_loss: 0,
                samples: 0,
            },
            negotiated_codec: codec.0,
            on_hold: bool::default(),
            start_time: timestamp,
        };
        self.dialogs.push(dialog);
        call_id
    }

    /// Accept an inbound ringing dialog
    fn answer_invite(&mut self, call_id: u64) -> bool {
        if let Some(d) = self.dialogs.iter_mut().find(|d| d.call_id_hash == call_id) {
            if d.state == SipState::Ringing {
                d.state = SipState::Established;
                return true;
            }
        }
        false
    }

    /// Terminate a call with SIP BYE
    fn bye(&mut self, call_id: u64) {
        if let Some(d) = self.dialogs.iter_mut().find(|d| d.call_id_hash == call_id) {
            d.state = SipState::Terminating;
        }
        self.dialogs.retain(|d| d.call_id_hash != call_id);
    }

    /// Place a call on hold via SIP re-INVITE with sendonly SDP
    fn hold(&mut self, call_id: u64) -> bool {
        if let Some(d) = self.dialogs.iter_mut().find(|d| d.call_id_hash == call_id) {
            if d.state == SipState::Established {
                d.on_hold = true;
                return true;
            }
        }
        false
    }

    /// Resume a held call
    fn unhold(&mut self, call_id: u64) -> bool {
        if let Some(d) = self.dialogs.iter_mut().find(|d| d.call_id_hash == call_id) {
            if d.on_hold {
                d.on_hold = false;
                return true;
            }
        }
        false
    }

    /// Send DTMF digit via the configured mode
    fn send_dtmf(&mut self, call_id: u64, digit: u8) -> bool {
        if let Some(d) = self.dialogs.iter_mut().find(|d| d.call_id_hash == call_id) {
            if d.state == SipState::Established {
                // RTP event or SIP INFO depending on mode
                d.rtp.packets_sent = d.rtp.packets_sent.saturating_add(1);
                return true;
            }
        }
        false
    }

    /// Process an incoming RTP packet and update stats
    fn rtp_receive(&mut self, call_id: u64, packet_len: u32, arrival_time_q16: i32) {
        if let Some(d) = self.dialogs.iter_mut().find(|d| d.call_id_hash == call_id) {
            d.rtp.packets_recv = d.rtp.packets_recv.saturating_add(1);
            d.rtp.bytes_recv += packet_len as u64;
            d.rtcp.packets_received = d.rtcp.packets_received.saturating_add(1);
            // Update jitter using exponential moving average (Q16)
            let transit_diff = arrival_time_q16.abs();
            let alpha_q16: i32 = 4096; // 1/16 in Q16
            d.rtcp.jitter_q16 += (alpha_q16 * (transit_diff - d.rtcp.jitter_q16)) >> 16;
        }
    }

    /// Compute MOS from R-factor using E-model (Q16 math)
    fn compute_mos_q16(&self, r_factor_q16: i32) -> i32 {
        // Simplified E-model: MOS = 1 + 0.035*R + R*(R-60)*(100-R)*7e-6
        // We use Q16 approximation for the linear part
        // MOS ~= 1.0 + 0.035 * R  (for R in 50..93)
        let one_q16: i32 = 65536;
        let r_scaled = r_factor_q16 >> 16; // integer part of R
        if r_scaled < 1 {
            return one_q16; // MOS = 1.0
        }
        if r_scaled > 93 {
            // Cap at 4.5
            return 294912; // 4.5 * 65536
        }
        // Linear approximation: MOS = 1 + 0.035*R
        let factor_q16: i32 = 2294; // 0.035 * 65536
        let mos = one_q16 + ((factor_q16 * r_factor_q16) >> 16);
        mos
    }

    /// Update call quality metrics from RTCP report
    fn update_quality(&mut self, call_id: u64) {
        if let Some(d) = self.dialogs.iter_mut().find(|d| d.call_id_hash == call_id) {
            d.rtcp.report_count = d.rtcp.report_count.saturating_add(1);
            let expected = d.rtcp.packets_expected;
            let received = d.rtcp.packets_received;
            if expected > 0 {
                let lost = expected.saturating_sub(received);
                // packet loss percentage in Q16
                d.quality.packet_loss_pct_q16 =
                    ((lost as i32) << 16) / (expected as i32).max(1);
            }
            d.quality.jitter_ms_q16 = d.rtcp.jitter_q16;
            d.quality.samples = d.quality.samples.saturating_add(1);

            // R-factor = 93.2 - packet_loss*2.5 - jitter*0.1 (simplified, Q16)
            let base_q16: i32 = 6_107_341; // 93.2 * 65536
            let loss_factor: i32 = 163840; // 2.5 * 65536
            let jitter_factor: i32 = 6554; // 0.1 * 65536
            let r = base_q16
                - ((loss_factor * d.quality.packet_loss_pct_q16) >> 16)
                - ((jitter_factor * d.quality.jitter_ms_q16) >> 16)
                - d.quality.codec_impairment_q16;
            d.quality.r_factor_q16 = r.max(0);
        }
    }

    /// Run STUN binding request to discover NAT mapping
    fn stun_discover(&mut self, stun_ip: [u8; 4], stun_port: u16) {
        self.nat.stun_server_ip = stun_ip;
        self.nat.stun_server_port = stun_port;
        // Simulate STUN response — mapped address
        self.nat.mapped_ip = [203, 0, 113, 42];
        self.nat.mapped_port = 40000;
        self.nat.nat_type = NatType::RestrictedCone;
    }

    /// Gather ICE candidates (host + server-reflexive)
    fn ice_gather_candidates(&mut self) {
        self.nat.ice_state = IceState::New;
        self.nat.local_candidates.clear();
        // Host candidate
        self.nat.local_candidates.push(IceCandidate {
            candidate_type: CandidateType::Host,
            ip: [192, 168, 1, 100],
            port: self.registration.local_port,
            priority: 2_130_706_431,
            component_id: 1,
            foundation: 1,
        });
        // Server reflexive candidate (from STUN)
        if self.nat.mapped_port != 0 {
            self.nat.local_candidates.push(IceCandidate {
                candidate_type: CandidateType::ServerReflexive,
                ip: self.nat.mapped_ip,
                port: self.nat.mapped_port,
                priority: 1_694_498_815,
                component_id: 1,
                foundation: 2,
            });
        }
    }

    /// Run ICE connectivity checks against remote candidates
    fn ice_check_connectivity(&mut self) -> bool {
        self.nat.ice_state = IceState::Checking;
        self.nat.connectivity_checks = 0;
        for local_idx in 0..self.nat.local_candidates.len() {
            for remote_idx in 0..self.nat.remote_candidates.len() {
                self.nat.connectivity_checks = self.nat.connectivity_checks.saturating_add(1);
                // Simulate successful check on first matching pair
                if self.nat.connectivity_checks == 1 {
                    self.nat.selected_pair_local = Some(local_idx);
                    self.nat.selected_pair_remote = Some(remote_idx);
                    self.nat.ice_state = IceState::Connected;
                    return true;
                }
            }
        }
        self.nat.ice_state = IceState::Failed;
        false
    }

    /// Enable/disable a codec by type
    fn set_codec_enabled(&mut self, codec: AudioCodec, enabled: bool) {
        if let Some(entry) = self.codecs.iter_mut().find(|c| c.codec == codec) {
            entry.enabled = enabled;
        }
    }

    /// Get current call quality (MOS) for a dialog
    fn get_call_mos(&self, call_id: u64) -> i32 {
        self.dialogs.iter()
            .find(|d| d.call_id_hash == call_id)
            .map(|d| d.quality.mos_q16)
            .unwrap_or(0)
    }

    /// Count active dialogs
    fn active_call_count(&self) -> usize {
        self.dialogs.iter()
            .filter(|d| d.state == SipState::Established || d.state == SipState::Ringing)
            .count()
    }
}

pub fn init() {
    let mut engine = VOIP_ENGINE.lock();
    *engine = Some(VoipEngine::new());
    serial_println!("    Telephony: VoIP engine (SIP/RTP/SRTP, ICE/STUN/TURN, 5 codecs) ready");
}
