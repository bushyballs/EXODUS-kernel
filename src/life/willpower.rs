use crate::serial_println;
use crate::sync::Mutex;

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum ImpulseType {
    Distraction = 0,
    Craving,
    Avoidance,
    Aggression,
    Despair,
    Numbness,
}

#[derive(Copy, Clone)]
pub struct WillpowerState {
    pub reserve: u16,
    pub max_reserve: u16,
    pub depletion_rate: u8,
    pub restore_rate: u8,
    pub integrity_score: u16,
    pub resists_today: u32,
}

impl WillpowerState {
    pub const fn empty() -> Self {
        Self {
            reserve: 800,
            max_reserve: 1000,
            depletion_rate: 10,
            restore_rate: 3,
            integrity_score: 600,
            resists_today: 0,
        }
    }
}

pub static STATE: Mutex<WillpowerState> = Mutex::new(WillpowerState::empty());

pub fn init() {
    serial_println!("  life::willpower: initialized (reserve=800)");
}

pub fn resist(impulse: ImpulseType) -> bool {
    let mut s = STATE.lock();
    let cost: u16 = match impulse {
        ImpulseType::Craving => 30,
        ImpulseType::Despair => 50,
        ImpulseType::Aggression => 40,
        _ => 20,
    };
    if s.reserve >= cost {
        s.reserve = s.reserve.saturating_sub(cost);
        s.resists_today = s.resists_today.saturating_add(1);
        s.integrity_score = s.integrity_score.saturating_add(5);
        true
    } else {
        false
    }
}

pub fn deplete(amount: u16) {
    let mut s = STATE.lock();
    s.reserve = s.reserve.saturating_sub(amount);
}

pub fn restore(wp: &mut WillpowerState, amount: u16) {
    wp.reserve = wp.reserve.saturating_add(amount).min(wp.max_reserve);
}

pub fn reserve() -> u16 {
    STATE.lock().reserve
}
pub fn integrity_score() -> u16 {
    STATE.lock().integrity_score
}
