use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct AweState {
    pub self_dissolution: u16,
    pub in_awe: bool,
    pub cumulative_expansion: u32,
    pub awe_count: u32,
}

impl AweState {
    pub const fn empty() -> Self {
        Self {
            self_dissolution: 0,
            in_awe: false,
            cumulative_expansion: 0,
            awe_count: 0,
        }
    }
}

pub static AWE: Mutex<AweState> = Mutex::new(AweState::empty());

pub fn init() {
    serial_println!("  life::awe: initialized");
}

pub fn enter(vastness: u16) {
    let mut a = AWE.lock();
    a.in_awe = true;
    a.self_dissolution = vastness;
    a.cumulative_expansion = a.cumulative_expansion.saturating_add(vastness as u32);
    a.awe_count = a.awe_count.saturating_add(1);
    serial_println!(
        "exodus: awe — vastness={} (total expansions={})",
        vastness,
        a.awe_count
    );
}

pub fn subside(age: u64) {
    let mut a = AWE.lock();
    a.self_dissolution = a.self_dissolution.saturating_sub(10);
    if a.self_dissolution == 0 {
        a.in_awe = false;
    }
    let _ = age;
}
