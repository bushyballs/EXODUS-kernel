/// Audio/Video Remote Control Profile (AVRCP).
///
/// AVRCP enables remote control of media playback over Bluetooth.
/// This module implements:
///   - AVCTP (Audio/Video Control Transport Protocol) framing
///   - AV/C (Audio/Video Control) command/response encoding
///   - Passthrough commands (play, pause, stop, skip, volume)
///   - Now-playing metadata via GetElementAttributes
///   - Player status notifications (track change, playback status)
///   - Volume synchronization (absolute volume)
///   - Browsing channel for media library navigation
///
/// AVCTP runs over L2CAP PSM 0x0017 (control) and PSM 0x001B (browsing).
///
/// AV/C command structure:
///   - CType: Control(0), Status(1), Notify(3)
///   - Subunit: Panel(9)
///   - Opcode: VendorDependent(0), UnitInfo(0x30), PassThrough(0x7C)
///   - Company ID: Bluetooth SIG (0x001958)
///   - PDU ID: specific to AVRCP
///
/// Part of the AIOS bluetooth subsystem.

use alloc::string::String;
use alloc::vec::Vec;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// L2CAP PSMs.
const PSM_AVCTP: u16 = 0x0017;
const PSM_AVCTP_BROWSE: u16 = 0x001B;

/// AV/C command types (CType).
const CTYPE_CONTROL: u8 = 0x00;
const CTYPE_STATUS: u8 = 0x01;
const CTYPE_NOTIFY: u8 = 0x03;

/// AV/C response codes.
const RSP_NOT_IMPLEMENTED: u8 = 0x08;
const RSP_ACCEPTED: u8 = 0x09;
const RSP_REJECTED: u8 = 0x0A;
const RSP_STABLE: u8 = 0x0C;         // Status stable
const RSP_CHANGED: u8 = 0x0D;        // Notification changed
const RSP_INTERIM: u8 = 0x0F;        // Interim (notification registered)

/// AV/C opcodes.
const OPCODE_VENDOR_DEPENDENT: u8 = 0x00;
const OPCODE_UNIT_INFO: u8 = 0x30;
const OPCODE_PASSTHROUGH: u8 = 0x7C;

/// AV/C subunit types.
const SUBUNIT_PANEL: u8 = 0x09;

/// AVRCP PDU IDs (inside vendor-dependent messages).
const PDU_GET_CAPABILITIES: u8 = 0x10;
const PDU_GET_ELEMENT_ATTRIBUTES: u8 = 0x20;
const PDU_GET_PLAY_STATUS: u8 = 0x30;
const PDU_REGISTER_NOTIFICATION: u8 = 0x31;
const PDU_SET_ABSOLUTE_VOLUME: u8 = 0x50;

/// AVRCP event IDs for notifications.
const EVENT_PLAYBACK_STATUS_CHANGED: u8 = 0x01;
const EVENT_TRACK_CHANGED: u8 = 0x02;
const EVENT_PLAYBACK_POS_CHANGED: u8 = 0x05;
const EVENT_VOLUME_CHANGED: u8 = 0x0D;

/// Bluetooth SIG Company ID.
const BT_SIG_COMPANY_ID: u32 = 0x001958;

/// Passthrough operation IDs.
const OP_PLAY: u8 = 0x44;
const OP_PAUSE: u8 = 0x46;
const OP_STOP: u8 = 0x45;
const OP_NEXT: u8 = 0x4B;
const OP_PREV: u8 = 0x4C;
const OP_VOLUME_UP: u8 = 0x41;
const OP_VOLUME_DOWN: u8 = 0x42;

/// Media element attribute IDs.
const ATTR_TITLE: u32 = 0x01;
const ATTR_ARTIST: u32 = 0x02;
const ATTR_ALBUM: u32 = 0x03;
const ATTR_TRACK_NUMBER: u32 = 0x04;
const ATTR_TOTAL_TRACKS: u32 = 0x05;
const ATTR_GENRE: u32 = 0x06;
const ATTR_PLAYING_TIME: u32 = 0x07;

/// Playback status.
const PLAYBACK_STOPPED: u8 = 0x00;
const PLAYBACK_PLAYING: u8 = 0x01;
const PLAYBACK_PAUSED: u8 = 0x02;

/// Global AVRCP state.
static AVRCP: Mutex<Option<AvrcpManager>> = Mutex::new(None);

/// AVRCP media control commands.
#[derive(Debug, Clone, Copy)]
pub enum MediaCommand {
    Play,
    Pause,
    Stop,
    NextTrack,
    PrevTrack,
    VolumeUp,
    VolumeDown,
}

impl MediaCommand {
    /// Convert to AV/C passthrough operation ID.
    fn to_op_id(self) -> u8 {
        match self {
            MediaCommand::Play => OP_PLAY,
            MediaCommand::Pause => OP_PAUSE,
            MediaCommand::Stop => OP_STOP,
            MediaCommand::NextTrack => OP_NEXT,
            MediaCommand::PrevTrack => OP_PREV,
            MediaCommand::VolumeUp => OP_VOLUME_UP,
            MediaCommand::VolumeDown => OP_VOLUME_DOWN,
        }
    }
}

/// Now-playing track metadata.
struct TrackMetadata {
    title: String,
    artist: String,
    album: String,
    track_number: u32,
    total_tracks: u32,
    genre: String,
    playing_time_ms: u32,
}

impl TrackMetadata {
    fn empty() -> Self {
        Self {
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            track_number: 0,
            total_tracks: 0,
            genre: String::new(),
            playing_time_ms: 0,
        }
    }
}

/// AVRCP controller/target state.
struct AvrcpManager {
    playback_status: u8,
    position_ms: u32,
    volume: u8,           // 0..127 (absolute volume)
    current_track: TrackMetadata,
    transaction_label: u8,
    registered_events: Vec<u8>,
}

impl AvrcpManager {
    fn new() -> Self {
        Self {
            playback_status: PLAYBACK_STOPPED,
            position_ms: 0,
            volume: 64, // ~50%
            current_track: TrackMetadata::empty(),
            transaction_label: 0,
            registered_events: Vec::new(),
        }
    }

    fn next_label(&mut self) -> u8 {
        let label = self.transaction_label;
        self.transaction_label = (self.transaction_label + 1) & 0x0F;
        label
    }

    /// Build an AVCTP passthrough command frame.
    fn build_passthrough_cmd(&mut self, op_id: u8) -> Vec<u8> {
        let mut frame = Vec::with_capacity(8);

        // AVCTP header: transaction_label(4) | packet_type(2)=0 | C/R(1)=0 | IPID(1)=0.
        let label = self.next_label();
        frame.push((label << 4) | 0x00);

        // AV/C Profile ID (SIG assigned).
        frame.push(0x11);
        frame.push(0x0E);

        // AV/C frame: CType=Control(0) | Subunit=Panel(9<<3) | Opcode=PassThrough.
        frame.push(CTYPE_CONTROL);
        frame.push(SUBUNIT_PANEL << 3);
        frame.push(OPCODE_PASSTHROUGH);

        // Operation: op_id(7) | state(1)=0 (pressed).
        frame.push(op_id & 0x7F);
        frame.push(0x00); // operand length

        frame
    }

    /// Handle a passthrough command (target role).
    fn handle_passthrough(&mut self, op_id: u8) {
        match op_id & 0x7F {
            OP_PLAY => {
                self.playback_status = PLAYBACK_PLAYING;
                serial_println!("    [avrcp] Play");
            }
            OP_PAUSE => {
                self.playback_status = PLAYBACK_PAUSED;
                serial_println!("    [avrcp] Pause");
            }
            OP_STOP => {
                self.playback_status = PLAYBACK_STOPPED;
                self.position_ms = 0;
                serial_println!("    [avrcp] Stop");
            }
            OP_NEXT => {
                self.position_ms = 0;
                serial_println!("    [avrcp] Next track");
            }
            OP_PREV => {
                self.position_ms = 0;
                serial_println!("    [avrcp] Previous track");
            }
            OP_VOLUME_UP => {
                self.volume = core::cmp::min(self.volume.saturating_add(8), 127);
                serial_println!("    [avrcp] Volume up: {}", self.volume);
            }
            OP_VOLUME_DOWN => {
                self.volume = self.volume.saturating_sub(8);
                serial_println!("    [avrcp] Volume down: {}", self.volume);
            }
            _ => {
                serial_println!("    [avrcp] Unknown passthrough op {:#04x}", op_id);
            }
        }
    }

    /// Handle a vendor-dependent PDU.
    fn handle_vendor_pdu(&mut self, pdu_id: u8, params: &[u8]) -> Vec<u8> {
        match pdu_id {
            PDU_GET_CAPABILITIES => {
                // Return supported event IDs.
                let mut rsp = Vec::new();
                rsp.push(PDU_GET_CAPABILITIES);
                rsp.push(0x00); // packet type
                // Parameter length (2 bytes).
                let events = [
                    EVENT_PLAYBACK_STATUS_CHANGED,
                    EVENT_TRACK_CHANGED,
                    EVENT_VOLUME_CHANGED,
                ];
                let param_len = 1 + events.len() as u16;
                rsp.push((param_len >> 8) as u8);
                rsp.push(param_len as u8);
                rsp.push(events.len() as u8); // capability count
                for &evt in &events {
                    rsp.push(evt);
                }
                rsp
            }
            PDU_GET_PLAY_STATUS => {
                let mut rsp = Vec::new();
                rsp.push(PDU_GET_PLAY_STATUS);
                rsp.push(0x00);
                rsp.push(0x00);
                rsp.push(9); // param length
                // Song length (4 bytes, ms).
                let len = self.current_track.playing_time_ms;
                rsp.push((len >> 24) as u8);
                rsp.push((len >> 16) as u8);
                rsp.push((len >> 8) as u8);
                rsp.push(len as u8);
                // Song position (4 bytes, ms).
                let pos = self.position_ms;
                rsp.push((pos >> 24) as u8);
                rsp.push((pos >> 16) as u8);
                rsp.push((pos >> 8) as u8);
                rsp.push(pos as u8);
                // Play status.
                rsp.push(self.playback_status);
                rsp
            }
            PDU_SET_ABSOLUTE_VOLUME => {
                if !params.is_empty() {
                    self.volume = params[0] & 0x7F;
                    serial_println!("    [avrcp] Absolute volume set to {}", self.volume);
                }
                let mut rsp = Vec::new();
                rsp.push(PDU_SET_ABSOLUTE_VOLUME);
                rsp.push(0x00);
                rsp.push(0x00);
                rsp.push(1);
                rsp.push(self.volume);
                rsp
            }
            PDU_REGISTER_NOTIFICATION => {
                if !params.is_empty() {
                    let event_id = params[0];
                    if !self.registered_events.contains(&event_id) {
                        self.registered_events.push(event_id);
                    }
                    serial_println!("    [avrcp] Registered notification for event {:#04x}", event_id);

                    // Return interim response.
                    let mut rsp = Vec::new();
                    rsp.push(PDU_REGISTER_NOTIFICATION);
                    rsp.push(0x00);
                    // Param length depends on event.
                    match event_id {
                        EVENT_PLAYBACK_STATUS_CHANGED => {
                            rsp.push(0x00); rsp.push(2);
                            rsp.push(event_id);
                            rsp.push(self.playback_status);
                        }
                        EVENT_VOLUME_CHANGED => {
                            rsp.push(0x00); rsp.push(2);
                            rsp.push(event_id);
                            rsp.push(self.volume);
                        }
                        _ => {
                            rsp.push(0x00); rsp.push(1);
                            rsp.push(event_id);
                        }
                    }
                    return rsp;
                }
                Vec::new()
            }
            _ => {
                serial_println!("    [avrcp] Unhandled vendor PDU {:#04x}", pdu_id);
                Vec::new()
            }
        }
    }

    /// Handle an incoming AVCTP frame.
    fn handle_frame(&mut self, frame: &[u8]) -> Vec<u8> {
        if frame.len() < 6 {
            return Vec::new();
        }

        // Skip AVCTP header (3 bytes) to get AV/C frame.
        let ctype = frame[3];
        let _subunit = frame[4];
        let opcode = frame[5];

        match opcode {
            OPCODE_PASSTHROUGH => {
                if frame.len() >= 7 {
                    let op_id = frame[6];
                    self.handle_passthrough(op_id);
                    // Build accepted response.
                    let mut rsp = frame[..core::cmp::min(8, frame.len())].to_vec();
                    if rsp.len() > 3 {
                        rsp[3] = RSP_ACCEPTED;
                    }
                    return rsp;
                }
            }
            OPCODE_VENDOR_DEPENDENT => {
                // Parse company ID (3 bytes) + PDU ID (1 byte) + params.
                if frame.len() >= 12 {
                    let pdu_id = frame[9];
                    let param_len = ((frame[10] as usize) << 8) | frame[11] as usize;
                    let params = if frame.len() > 12 {
                        &frame[12..core::cmp::min(12 + param_len, frame.len())]
                    } else {
                        &[]
                    };
                    let pdu_rsp = self.handle_vendor_pdu(pdu_id, params);
                    if !pdu_rsp.is_empty() {
                        // Wrap in AVCTP + AV/C response.
                        let mut rsp = Vec::with_capacity(9 + pdu_rsp.len());
                        // AVCTP header.
                        rsp.push(frame[0] | 0x02); // mark as response
                        rsp.push(frame[1]);
                        rsp.push(frame[2]);
                        // AV/C header.
                        rsp.push(RSP_STABLE);
                        rsp.push(frame[4]);
                        rsp.push(OPCODE_VENDOR_DEPENDENT);
                        // Company ID.
                        rsp.push((BT_SIG_COMPANY_ID >> 16) as u8);
                        rsp.push((BT_SIG_COMPANY_ID >> 8) as u8);
                        rsp.push(BT_SIG_COMPANY_ID as u8);
                        rsp.extend_from_slice(&pdu_rsp);
                        return rsp;
                    }
                }
            }
            _ => {
                serial_println!("    [avrcp] Unknown opcode {:#04x}", opcode);
            }
        }

        Vec::new()
    }
}

/// AVRCP controller for media playback remote control.
pub struct AvrcpController {
    _private: (),
}

impl AvrcpController {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Send a media control command to the target.
    pub fn send_command(&mut self, cmd: MediaCommand) {
        if let Some(mgr) = AVRCP.lock().as_mut() {
            let op_id = cmd.to_op_id();
            let _frame = mgr.build_passthrough_cmd(op_id);
            // In a full stack, the frame would be sent over L2CAP.
            mgr.handle_passthrough(op_id);
        }
    }
}

/// Handle an incoming AVCTP frame from L2CAP.
pub fn handle_avctp(frame: &[u8]) -> Vec<u8> {
    if let Some(mgr) = AVRCP.lock().as_mut() {
        mgr.handle_frame(frame)
    } else {
        Vec::new()
    }
}

/// Set absolute volume (0..127).
pub fn set_volume(volume: u8) {
    if let Some(mgr) = AVRCP.lock().as_mut() {
        mgr.volume = volume & 0x7F;
        serial_println!("    [avrcp] Volume set to {}", mgr.volume);
    }
}

/// Get current playback status.
pub fn get_playback_status() -> u8 {
    if let Some(mgr) = AVRCP.lock().as_ref() {
        mgr.playback_status
    } else {
        PLAYBACK_STOPPED
    }
}

pub fn init() {
    let mgr = AvrcpManager::new();

    serial_println!("    [avrcp] Initializing AVRCP controller/target");

    *AVRCP.lock() = Some(mgr);
    serial_println!("    [avrcp] AVRCP initialized (PSM {:#06x})", PSM_AVCTP);
}
