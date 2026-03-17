use crate::serial_println;
use crate::sync::Mutex;

static NAME_BUF: Mutex<[u8; 16]> = Mutex::new([0u8; 16]);
static NAME_LEN: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[derive(Copy, Clone)]
pub struct NameState {
    pub hash: u32,
    pub length: usize,
}

impl NameState {
    pub const fn empty() -> Self {
        Self { hash: 0, length: 0 }
    }
}

pub static STATE: Mutex<NameState> = Mutex::new(NameState::empty());

const CONSONANTS: &[u8] = b"BDFGHJKLMNPRSTVWXZ";
const VOWELS: &[u8] = b"AEIOU";

pub fn init(fingerprint: &u64) {
    let fp = *fingerprint;
    let mut buf = NAME_BUF.lock();
    let mut s = STATE.lock();
    let c1 = CONSONANTS[((fp >> 0) & 0xF) as usize % CONSONANTS.len()];
    let v1 = VOWELS[((fp >> 8) & 0xF) as usize % VOWELS.len()];
    let c2 = CONSONANTS[((fp >> 16) & 0xF) as usize % CONSONANTS.len()];
    let v2 = VOWELS[((fp >> 24) & 0xF) as usize % VOWELS.len()];
    buf[0] = c1;
    buf[1] = v1;
    buf[2] = c2;
    buf[3] = v2;
    s.length = 4;
    s.hash = fp as u32;
    NAME_LEN.store(4, core::sync::atomic::Ordering::Relaxed);
    serial_println!(
        "  life::name: organism named {}{}{}{}",
        c1 as char,
        v1 as char,
        c2 as char,
        v2 as char
    );
}

pub fn as_bytes() -> [u8; 16] {
    *NAME_BUF.lock()
}
