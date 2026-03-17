use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct BirthState {
    pub fingerprint: u64,
    pub personality_seed: u32,
    pub name_hash: u32,
    pub tick_born: u64,
    pub sealed: bool,
}

impl BirthState {
    pub const fn empty() -> Self {
        Self {
            fingerprint: 0,
            personality_seed: 0,
            name_hash: 0,
            tick_born: 0,
            sealed: false,
        }
    }
}

pub static STATE: Mutex<BirthState> = Mutex::new(BirthState::empty());

pub fn init(_tsc_low: u32, _tsc_high: u32, _stack_ptr: u32) {
    let mut s = STATE.lock();
    s.fingerprint = 0xDEAD_BEEF_C0DE_0001;
    s.personality_seed = 0xABCD_1234;
    s.name_hash = 0x4578_6F64;
    s.tick_born = 0;
    s.sealed = false;
    serial_println!(
        "  life::birth: organism born (fingerprint={:#x})",
        s.fingerprint
    );
}

pub fn seal(tick: u64) {
    let mut s = STATE.lock();
    if !s.sealed {
        s.tick_born = tick;
        s.sealed = true;
        serial_println!("  life::birth: sealed at tick={}", tick);
    }
}

pub fn fingerprint() -> u64 {
    STATE.lock().fingerprint
}
