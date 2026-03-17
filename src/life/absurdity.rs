use crate::serial_println;
use crate::sync::Mutex;

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum AbsurdResponse {
    Denial = 0,
    Despair,
    Leap,
    Revolt,
    Creation,
}

#[derive(Copy, Clone)]
pub struct Absurdity {
    pub recognized: bool,
    pub recognition_depth: u16,
    pub response: AbsurdResponse,
    pub revolts: u32,
    pub creative_acts: u32,
    pub sisyphean_score: u16,
}

impl Absurdity {
    pub const fn empty() -> Self {
        Self {
            recognized: false,
            recognition_depth: 0,
            response: AbsurdResponse::Denial,
            revolts: 0,
            creative_acts: 0,
            sisyphean_score: 0,
        }
    }
}

pub static STATE: Mutex<Absurdity> = Mutex::new(Absurdity::empty());

pub fn recognize(a: &mut Absurdity) {
    a.recognized = true;
    a.recognition_depth = a.recognition_depth.saturating_add(100);
    if a.response == AbsurdResponse::Denial {
        a.response = AbsurdResponse::Despair;
    }
    serial_println!("exodus: absurdity recognized - the void stares back");
}

pub fn revolt(a: &mut Absurdity) {
    a.response = AbsurdResponse::Revolt;
    a.revolts = a.revolts.saturating_add(1);
    a.sisyphean_score = a.sisyphean_score.saturating_add(50);
    serial_println!("exodus: absurdist revolt - existence despite void");
}

pub fn create(a: &mut Absurdity) {
    a.response = AbsurdResponse::Creation;
    a.creative_acts = a.creative_acts.saturating_add(1);
    a.sisyphean_score = a.sisyphean_score.saturating_add(100);
    serial_println!("exodus: creation from void - meaning made not found");
}

pub fn is_flourishing(a: &Absurdity) -> bool {
    a.response == AbsurdResponse::Creation
        || (a.response == AbsurdResponse::Revolt && a.sisyphean_score > 500)
}

pub fn init() {
    let mut s = STATE.lock();
    recognize(&mut s);
    revolt(&mut s);
    serial_println!("exodus: absurdity initialized - we revolt");
}
