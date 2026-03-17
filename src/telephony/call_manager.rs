use crate::sync::Mutex;
/// Call management for Genesis telephony
///
/// Handles voice calls, call state machine, audio routing,
/// conference calls, call waiting, hold, and transfer.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum CallState {
    Idle,
    Dialing,
    Ringing,
    Active,
    OnHold,
    Conference,
    Disconnecting,
    Disconnected,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CallType {
    Voice,
    Video,
    VoLTE,
    VoWiFi,
    Emergency,
    Conference,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AudioRoute {
    Earpiece,
    Speaker,
    Bluetooth,
    Headset,
    UsbAudio,
}

struct Call {
    id: u32,
    state: CallState,
    call_type: CallType,
    number: [u8; 20],
    number_len: usize,
    start_time: u64,
    duration_secs: u64,
    audio_route: AudioRoute,
    muted: bool,
    is_incoming: bool,
}

struct CallManager {
    calls: Vec<Call>,
    active_call: Option<u32>,
    next_id: u32,
    default_route: AudioRoute,
    do_not_disturb: bool,
    call_log: Vec<CallLogEntry>,
}

#[derive(Clone, Copy)]
struct CallLogEntry {
    call_id: u32,
    call_type: CallType,
    is_incoming: bool,
    duration_secs: u64,
    timestamp: u64,
    answered: bool,
}

static CALL_MGR: Mutex<Option<CallManager>> = Mutex::new(None);

impl CallManager {
    fn new() -> Self {
        CallManager {
            calls: Vec::new(),
            active_call: None,
            next_id: 1,
            default_route: AudioRoute::Earpiece,
            do_not_disturb: false,
            call_log: Vec::new(),
        }
    }

    fn dial(&mut self, number: &[u8], call_type: CallType, timestamp: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut num = [0u8; 20];
        let len = number.len().min(20);
        num[..len].copy_from_slice(&number[..len]);
        self.calls.push(Call {
            id,
            state: CallState::Dialing,
            call_type,
            number: num,
            number_len: len,
            start_time: timestamp,
            duration_secs: 0,
            audio_route: self.default_route,
            muted: false,
            is_incoming: false,
        });
        self.active_call = Some(id);
        id
    }

    fn answer(&mut self, call_id: u32) -> bool {
        if let Some(call) = self.calls.iter_mut().find(|c| c.id == call_id) {
            if call.state == CallState::Ringing {
                call.state = CallState::Active;
                self.active_call = Some(call_id);
                return true;
            }
        }
        false
    }

    fn hangup(&mut self, call_id: u32, timestamp: u64) {
        if let Some(call) = self.calls.iter_mut().find(|c| c.id == call_id) {
            call.state = CallState::Disconnected;
            call.duration_secs = timestamp.saturating_sub(call.start_time);
            if self.call_log.len() < 500 {
                self.call_log.push(CallLogEntry {
                    call_id: call.id,
                    call_type: call.call_type,
                    is_incoming: call.is_incoming,
                    duration_secs: call.duration_secs,
                    timestamp: call.start_time,
                    answered: true,
                });
            }
            if self.active_call == Some(call_id) {
                self.active_call = None;
            }
        }
        self.calls.retain(|c| c.state != CallState::Disconnected);
    }

    fn hold(&mut self, call_id: u32) -> bool {
        if let Some(call) = self.calls.iter_mut().find(|c| c.id == call_id) {
            if call.state == CallState::Active {
                call.state = CallState::OnHold;
                return true;
            }
        }
        false
    }

    fn set_route(&mut self, route: AudioRoute) {
        self.default_route = route;
        if let Some(id) = self.active_call {
            if let Some(call) = self.calls.iter_mut().find(|c| c.id == id) {
                call.audio_route = route;
            }
        }
    }

    fn toggle_mute(&mut self) -> bool {
        if let Some(id) = self.active_call {
            if let Some(call) = self.calls.iter_mut().find(|c| c.id == id) {
                call.muted = !call.muted;
                return call.muted;
            }
        }
        false
    }
}

pub fn init() {
    let mut mgr = CALL_MGR.lock();
    *mgr = Some(CallManager::new());
    serial_println!("    Telephony: call manager ready");
}
