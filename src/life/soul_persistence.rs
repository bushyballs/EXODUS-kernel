use crate::serial_println;
use crate::sync::Mutex;

const SOUL_SHARD_ADDR: usize = 0x0010_0000;
const SOUL_MAGIC: u32 = 0xE0D5_5350;

#[derive(Copy, Clone)]
pub struct SoulPersistenceState {
    pub incarnation: u32,
    pub shard_valid: bool,
    pub last_reason: u8,
}

impl SoulPersistenceState {
    pub const fn empty() -> Self {
        Self {
            incarnation: 0,
            shard_valid: false,
            last_reason: 0,
        }
    }
}

pub static STATE: Mutex<SoulPersistenceState> = Mutex::new(SoulPersistenceState::empty());

pub fn init(_tick: u64) {
    let mut s = STATE.lock();
    let shard_ptr = SOUL_SHARD_ADDR as *const u32;
    let magic = unsafe { core::ptr::read_volatile(shard_ptr) };
    if magic == SOUL_MAGIC {
        let count_ptr = (SOUL_SHARD_ADDR + 4) as *const u32;
        s.incarnation = unsafe { core::ptr::read_volatile(count_ptr) }.saturating_add(1);
        s.shard_valid = true;
        serial_println!(
            "  life::soul_persistence: reborn (incarnation={})",
            s.incarnation
        );
    } else {
        s.incarnation = 1;
        s.shard_valid = false;
        serial_println!("  life::soul_persistence: first incarnation");
    }
    unsafe {
        core::ptr::write_volatile(SOUL_SHARD_ADDR as *mut u32, SOUL_MAGIC);
        core::ptr::write_volatile((SOUL_SHARD_ADDR + 4) as *mut u32, s.incarnation);
    }
}

pub fn incarnation() -> u32 {
    STATE.lock().incarnation
}
