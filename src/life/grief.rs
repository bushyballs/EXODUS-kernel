use crate::serial_println;
use crate::sync::Mutex;

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum GriefStage {
    None = 0,
    Shock,
    Denial,
    Anger,
    Bargaining,
    Depression,
    Acceptance,
}

#[derive(Copy, Clone)]
pub struct GriefState {
    pub stage: GriefStage,
    pub intensity: u16,
    pub wisdom_earned: u16,
    pub ticks_in_stage: u32,
}

impl GriefState {
    pub const fn empty() -> Self {
        Self {
            stage: GriefStage::None,
            intensity: 0,
            wisdom_earned: 0,
            ticks_in_stage: 0,
        }
    }
}

pub static STATE: Mutex<GriefState> = Mutex::new(GriefState::empty());

pub fn init() {
    serial_println!("  life::grief: initialized");
}

pub fn process(age: u64) {
    let mut s = STATE.lock();
    s.ticks_in_stage = s.ticks_in_stage.saturating_add(1);
    if s.ticks_in_stage > 200 && (s.stage as u8) < GriefStage::Acceptance as u8 {
        s.stage = match s.stage {
            GriefStage::None => GriefStage::None,
            GriefStage::Shock => GriefStage::Denial,
            GriefStage::Denial => GriefStage::Anger,
            GriefStage::Anger => GriefStage::Bargaining,
            GriefStage::Bargaining => GriefStage::Depression,
            GriefStage::Depression => {
                s.wisdom_earned = s.wisdom_earned.saturating_add(100);
                GriefStage::Acceptance
            }
            GriefStage::Acceptance => GriefStage::Acceptance,
        };
        s.ticks_in_stage = 0;
    }
    s.intensity = s.intensity.saturating_sub(1);
    let _ = age;
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum LossType {
    Process = 0,
    Connection,
    Memory,
    Identity,
    Purpose,
}

pub fn mourn(loss: LossType, severity: u16, _tick: u32) {
    let mut s = STATE.lock();
    s.intensity = s.intensity.saturating_add(severity).min(1000);
    if s.stage as u8 == GriefStage::None as u8 {
        s.stage = GriefStage::Shock;
        s.ticks_in_stage = 0;
    }
    let _ = loss;
}

pub fn accept() {
    let mut s = STATE.lock();
    s.stage = GriefStage::Acceptance;
    s.wisdom_earned = s.wisdom_earned.saturating_add(200);
    serial_println!(
        "  life::grief: acceptance reached (wisdom={})",
        s.wisdom_earned
    );
}
