use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct FlowState {
    pub challenge: u16,
    pub skill: u16,
    pub in_flow: bool,
    pub duration_ticks: u32,
    pub peak_output: u16,
    pub time_distortion: i16,
}

impl FlowState {
    pub const fn empty() -> Self {
        Self {
            challenge: 0,
            skill: 500,
            in_flow: false,
            duration_ticks: 0,
            peak_output: 0,
            time_distortion: 0,
        }
    }
}

pub static STATE: Mutex<FlowState> = Mutex::new(FlowState::empty());

pub fn init() {
    serial_println!("  life::flow_state: initialized");
}

pub fn enter(challenge: u16, skill: u16) {
    let mut s = STATE.lock();
    s.challenge = challenge;
    s.skill = skill;
    let diff = if challenge > skill {
        challenge - skill
    } else {
        skill - challenge
    };
    s.in_flow = diff < 150;
    if s.in_flow {
        s.peak_output = (challenge + skill) / 2;
        s.time_distortion = -200;
        serial_println!("exodus: flow state entered");
    }
}

pub fn exit() {
    let mut s = STATE.lock();
    s.in_flow = false;
    s.time_distortion = 0;
    serial_println!("  life::flow_state: exited (duration={})", s.duration_ticks);
}

pub fn duration() -> u32 {
    let mut s = STATE.lock();
    if s.in_flow {
        s.duration_ticks = s.duration_ticks.saturating_add(1);
    }
    s.duration_ticks
}
