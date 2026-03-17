/// Conference calling for Genesis telephony
///
/// Multi-party audio/video conferencing, audio mixing,
/// participant management, mute/unmute controls, screen sharing,
/// recording, lobby/waiting room, and moderator controls.
///
/// Original implementation for Hoags OS.

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Participant and role types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum ParticipantRole {
    Host,
    CoHost,
    Presenter,
    Attendee,
    Listener,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ParticipantState {
    InLobby,
    Connecting,
    Connected,
    OnHold,
    Disconnected,
}

#[derive(Clone, Copy, PartialEq)]
pub enum MediaType {
    AudioOnly,
    AudioVideo,
    ScreenShare,
    ScreenShareWithAudio,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ConferenceState {
    Idle,
    Starting,
    Active,
    Paused,
    Ending,
    Ended,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RecordingState {
    Inactive,
    Recording,
    Paused,
    Stopped,
}

// ---------------------------------------------------------------------------
// Participant
// ---------------------------------------------------------------------------

struct Participant {
    id: u32,
    call_id: u64,
    role: ParticipantRole,
    state: ParticipantState,
    media: MediaType,
    audio_muted: bool,
    video_muted: bool,
    hand_raised: bool,
    audio_level_q16: i32,     // Q16 RMS audio level (0.0 to 1.0)
    join_time: u64,
    name: [u8; 64],
    name_len: usize,
    number: [u8; 20],
    number_len: usize,
}

// ---------------------------------------------------------------------------
// Audio mixer (Q16 fixed-point mixing)
// ---------------------------------------------------------------------------

struct AudioMixer {
    sample_rate: u32,
    frame_size: u32,
    max_active_speakers: u8,
    gain_q16: i32,            // Master gain, Q16 (1.0 = 65536)
    agc_enabled: bool,
    noise_gate_threshold_q16: i32,
    mix_buffer: Vec<i32>,     // Q16 mixed samples
    frames_mixed: u64,
}

impl AudioMixer {
    fn new(sample_rate: u32, frame_size: u32) -> Self {
        let buf_len = frame_size as usize;
        let mut mix_buffer = Vec::new();
        for _ in 0..buf_len {
            mix_buffer.push(0i32);
        }
        AudioMixer {
            sample_rate,
            frame_size,
            max_active_speakers: 3,
            gain_q16: 65536,  // 1.0
            agc_enabled: true,
            noise_gate_threshold_q16: 1310,  // ~0.02
            mix_buffer,
            frames_mixed: 0,
        }
    }

    /// Mix audio from multiple participants into the output buffer.
    /// Each input is a slice of Q16 samples. We sum and apply gain/clipping.
    fn mix_frame(&mut self, inputs: &[&[i32]], participant_gains: &[i32]) {
        // Clear mix buffer
        for sample in self.mix_buffer.iter_mut() {
            *sample = 0;
        }
        let frame_len = self.frame_size as usize;
        for (idx, input) in inputs.iter().enumerate() {
            let gain = if idx < participant_gains.len() {
                participant_gains[idx]
            } else {
                65536 // 1.0
            };
            let samples_to_mix = input.len().min(frame_len);
            for i in 0..samples_to_mix {
                let scaled = (input[i] as i64 * gain as i64) >> 16;
                self.mix_buffer[i] = self.mix_buffer[i].saturating_add(scaled as i32);
            }
        }
        // Apply master gain and clip
        for sample in self.mix_buffer.iter_mut() {
            let gained = (*sample as i64 * self.gain_q16 as i64) >> 16;
            // Clip to Q16 range: approximately -32768..32767 shifted by 16
            let max_val: i32 = 0x7FFF_0000;
            let min_val: i32 = -0x7FFF_0000;
            *sample = (gained as i32).max(min_val).min(max_val);
        }
        self.frames_mixed = self.frames_mixed.saturating_add(1);
    }

    /// Compute RMS level of a Q16 sample buffer
    fn compute_rms_q16(&self, samples: &[i32]) -> i32 {
        if samples.is_empty() {
            return 0;
        }
        let mut sum: i64 = 0;
        for &s in samples {
            let val = s as i64;
            sum += (val * val) >> 16; // keep in Q16 range
        }
        let mean = sum / samples.len() as i64;
        // Approximate sqrt via Newton's method (integer)
        if mean <= 0 {
            return 0;
        }
        let mut x = mean;
        for _ in 0..8 {
            x = (x + mean / x) / 2;
        }
        x as i32
    }
}

// ---------------------------------------------------------------------------
// Screen share session
// ---------------------------------------------------------------------------

struct ScreenShareSession {
    presenter_id: u32,
    active: bool,
    width: u16,
    height: u16,
    fps: u8,
    frames_sent: u64,
    annotation_enabled: bool,
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

struct Recording {
    state: RecordingState,
    start_time: u64,
    duration_secs: u64,
    file_size_kb: u64,
    sample_rate: u32,
    channels: u8,
    frames_recorded: u64,
    consent_all: bool,
}

// ---------------------------------------------------------------------------
// Lobby / waiting room
// ---------------------------------------------------------------------------

struct LobbySettings {
    enabled: bool,
    auto_admit_known: bool,
    max_wait_secs: u32,
    custom_message: [u8; 128],
    message_len: usize,
}

// ---------------------------------------------------------------------------
// Conference room
// ---------------------------------------------------------------------------

struct ConferenceRoom {
    id: u32,
    state: ConferenceState,
    participants: Vec<Participant>,
    mixer: AudioMixer,
    screen_share: ScreenShareSession,
    recording: Recording,
    lobby: LobbySettings,
    max_participants: u16,
    next_participant_id: u32,
    start_time: u64,
    pin_code: u32,
    mute_on_entry: bool,
    lock_room: bool,
}

struct ConferenceManager {
    rooms: Vec<ConferenceRoom>,
    next_room_id: u32,
    total_conferences: u32,
    max_concurrent_rooms: u8,
}

static CONF_MGR: Mutex<Option<ConferenceManager>> = Mutex::new(None);

impl ConferenceManager {
    fn new() -> Self {
        ConferenceManager {
            rooms: Vec::new(),
            next_room_id: 1,
            total_conferences: 0,
            max_concurrent_rooms: 4,
        }
    }

    /// Create a new conference room
    fn create_room(&mut self, pin: u32, max_participants: u16, timestamp: u64) -> u32 {
        if self.rooms.len() >= self.max_concurrent_rooms as usize {
            return 0;
        }
        let id = self.next_room_id;
        self.next_room_id = self.next_room_id.saturating_add(1);
        self.total_conferences = self.total_conferences.saturating_add(1);
        self.rooms.push(ConferenceRoom {
            id,
            state: ConferenceState::Starting,
            participants: Vec::new(),
            mixer: AudioMixer::new(16000, 320), // 16kHz, 20ms frames
            screen_share: ScreenShareSession {
                presenter_id: 0,
                active: false,
                width: 1920,
                height: 1080,
                fps: 15,
                frames_sent: 0,
                annotation_enabled: false,
            },
            recording: Recording {
                state: RecordingState::Inactive,
                start_time: 0,
                duration_secs: 0,
                file_size_kb: 0,
                sample_rate: 16000,
                channels: 1,
                frames_recorded: 0,
                consent_all: false,
            },
            lobby: LobbySettings {
                enabled: true,
                auto_admit_known: true,
                max_wait_secs: 300,
                custom_message: [0; 128],
                message_len: 0,
            },
            max_participants,
            next_participant_id: 1,
            start_time: timestamp,
            pin_code: pin,
            mute_on_entry: false,
            lock_room: false,
        });
        id
    }

    /// Add a participant to a conference room (goes to lobby if enabled)
    fn join_room(&mut self, room_id: u32, call_id: u64, name: &[u8],
                 number: &[u8], timestamp: u64) -> u32 {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            if room.lock_room {
                return 0;
            }
            if room.participants.len() >= room.max_participants as usize {
                return 0;
            }
            let pid = room.next_participant_id;
            room.next_participant_id = room.next_participant_id.saturating_add(1);
            let mut pname = [0u8; 64];
            let nlen = name.len().min(64);
            pname[..nlen].copy_from_slice(&name[..nlen]);
            let mut pnum = [0u8; 20];
            let numlen = number.len().min(20);
            pnum[..numlen].copy_from_slice(&number[..numlen]);
            let initial_state = if room.lobby.enabled {
                ParticipantState::InLobby
            } else {
                ParticipantState::Connected
            };
            let is_first = room.participants.is_empty();
            room.participants.push(Participant {
                id: pid,
                call_id,
                role: if is_first { ParticipantRole::Host } else { ParticipantRole::Attendee },
                state: initial_state,
                media: MediaType::AudioOnly,
                audio_muted: room.mute_on_entry,
                video_muted: true,
                hand_raised: false,
                audio_level_q16: 0,
                join_time: timestamp,
                name: pname,
                name_len: nlen,
                number: pnum,
                number_len: numlen,
            });
            if room.state == ConferenceState::Starting {
                room.state = ConferenceState::Active;
            }
            return pid;
        }
        0
    }

    /// Admit a participant from the lobby
    fn admit_from_lobby(&mut self, room_id: u32, participant_id: u32) -> bool {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            if let Some(p) = room.participants.iter_mut().find(|p| p.id == participant_id) {
                if p.state == ParticipantState::InLobby {
                    p.state = ParticipantState::Connected;
                    return true;
                }
            }
        }
        false
    }

    /// Toggle mute for a participant
    fn toggle_mute(&mut self, room_id: u32, participant_id: u32) -> bool {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            if let Some(p) = room.participants.iter_mut().find(|p| p.id == participant_id) {
                p.audio_muted = !p.audio_muted;
                return p.audio_muted;
            }
        }
        false
    }

    /// Mute all participants except the host
    fn mute_all(&mut self, room_id: u32) {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            for p in room.participants.iter_mut() {
                if p.role != ParticipantRole::Host {
                    p.audio_muted = true;
                }
            }
        }
    }

    /// Raise or lower a participant's hand
    fn toggle_hand(&mut self, room_id: u32, participant_id: u32) -> bool {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            if let Some(p) = room.participants.iter_mut().find(|p| p.id == participant_id) {
                p.hand_raised = !p.hand_raised;
                return p.hand_raised;
            }
        }
        false
    }

    /// Start screen sharing for a participant
    fn start_screen_share(&mut self, room_id: u32, participant_id: u32) -> bool {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            if room.screen_share.active {
                return false; // someone already sharing
            }
            if let Some(p) = room.participants.iter_mut().find(|p| p.id == participant_id) {
                p.media = MediaType::ScreenShareWithAudio;
                room.screen_share.active = true;
                room.screen_share.presenter_id = participant_id;
                room.screen_share.frames_sent = 0;
                return true;
            }
        }
        false
    }

    /// Stop screen sharing
    fn stop_screen_share(&mut self, room_id: u32) {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            if room.screen_share.active {
                let presenter = room.screen_share.presenter_id;
                if let Some(p) = room.participants.iter_mut().find(|p| p.id == presenter) {
                    p.media = MediaType::AudioOnly;
                }
                room.screen_share.active = false;
                room.screen_share.presenter_id = 0;
            }
        }
    }

    /// Start recording the conference
    fn start_recording(&mut self, room_id: u32, timestamp: u64) -> bool {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            if room.recording.state == RecordingState::Inactive
                || room.recording.state == RecordingState::Stopped {
                room.recording.state = RecordingState::Recording;
                room.recording.start_time = timestamp;
                room.recording.frames_recorded = 0;
                room.recording.file_size_kb = 0;
                return true;
            }
        }
        false
    }

    /// Stop recording
    fn stop_recording(&mut self, room_id: u32, timestamp: u64) {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            if room.recording.state == RecordingState::Recording
                || room.recording.state == RecordingState::Paused {
                room.recording.state = RecordingState::Stopped;
                room.recording.duration_secs = timestamp.saturating_sub(room.recording.start_time);
            }
        }
    }

    /// Remove a participant from a conference
    fn remove_participant(&mut self, room_id: u32, participant_id: u32) {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            // If the participant was sharing screen, stop it
            if room.screen_share.active && room.screen_share.presenter_id == participant_id {
                room.screen_share.active = false;
                room.screen_share.presenter_id = 0;
            }
            room.participants.retain(|p| p.id != participant_id);
            // End conference if empty
            if room.participants.is_empty() {
                room.state = ConferenceState::Ended;
            }
        }
    }

    /// End a conference room entirely
    fn end_conference(&mut self, room_id: u32, timestamp: u64) {
        if let Some(room) = self.rooms.iter_mut().find(|r| r.id == room_id) {
            room.state = ConferenceState::Ending;
            if room.recording.state == RecordingState::Recording {
                room.recording.state = RecordingState::Stopped;
                room.recording.duration_secs =
                    timestamp.saturating_sub(room.recording.start_time);
            }
            room.participants.clear();
            room.state = ConferenceState::Ended;
        }
        self.rooms.retain(|r| r.state != ConferenceState::Ended);
    }

    /// Get participant count for a room
    fn participant_count(&self, room_id: u32) -> usize {
        self.rooms.iter()
            .find(|r| r.id == room_id)
            .map(|r| r.participants.iter().filter(|p| p.state == ParticipantState::Connected).count())
            .unwrap_or(0)
    }

    /// Get count of participants waiting in lobby
    fn lobby_count(&self, room_id: u32) -> usize {
        self.rooms.iter()
            .find(|r| r.id == room_id)
            .map(|r| r.participants.iter().filter(|p| p.state == ParticipantState::InLobby).count())
            .unwrap_or(0)
    }
}

pub fn init() {
    let mut mgr = CONF_MGR.lock();
    *mgr = Some(ConferenceManager::new());
    serial_println!("    Telephony: conference calling (mixing, recording, lobby) ready");
}
