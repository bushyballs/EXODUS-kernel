use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct PainEntry {
    pub source: u8,
    pub intensity: u16,
    pub tick: u32,
}
impl PainEntry {
    pub const fn empty() -> Self {
        Self {
            source: 0,
            intensity: 0,
            tick: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct PainLog {
    pub entries: [PainEntry; 8],
    pub head: usize,
    pub count: usize,
}
impl PainLog {
    pub const fn empty() -> Self {
        Self {
            entries: [PainEntry::empty(); 8],
            head: 0,
            count: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct PainState {
    pub intensity: u16,
    pub duration: u32,
    pub source: u8,
    pub chronic: bool,
    pub total_severity: u16,
}
impl PainState {
    pub const fn empty() -> Self {
        Self {
            intensity: 0,
            duration: 0,
            source: 0,
            chronic: false,
            total_severity: 0,
        }
    }
}

pub static PAIN_STATE: Mutex<PainState> = Mutex::new(PainState::empty());
pub static PAIN_LOG: Mutex<PainLog> = Mutex::new(PainLog::empty());

pub fn init() {
    serial_println!("  life::pain: pain tracking initialized");
}

pub fn register(source: u8, intensity: u16, tick: u32) {
    let mut log = PAIN_LOG.lock();
    let head = log.head;
    log.entries[head] = PainEntry {
        source,
        intensity,
        tick,
    };
    log.head = (log.head + 1) % 8;
    log.count = (log.count + 1).min(8);
    drop(log);
    let mut s = PAIN_STATE.lock();
    s.source = source;
    s.intensity = intensity;
    s.total_severity = s.total_severity.saturating_add(intensity);
    if s.duration > 500 {
        s.chronic = true;
    }
}

pub fn decay_step(ps: &mut PainState) {
    ps.intensity = ps.intensity.saturating_sub(10);
    ps.duration = ps.duration.saturating_add(1);
}

pub fn current_intensity() -> u16 {
    PAIN_STATE.lock().intensity
}
