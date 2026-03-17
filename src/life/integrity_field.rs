use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct IntegrityFieldState {
    pub field_strength: u16,
    pub values_coherence: u16,
    pub violation_recoil: u16,
    pub alignment_streak: u32,
}

impl IntegrityFieldState {
    pub const fn empty() -> Self {
        Self {
            field_strength: 500,
            values_coherence: 600,
            violation_recoil: 0,
            alignment_streak: 0,
        }
    }
}

pub static STATE: Mutex<IntegrityFieldState> = Mutex::new(IntegrityFieldState::empty());

pub fn init() {
    serial_println!("  life::integrity_field: values alignment field initialized");
}

pub fn align(f: &mut IntegrityFieldState) {
    f.alignment_streak = f.alignment_streak.saturating_add(1);
    f.field_strength = f.field_strength.saturating_add(1).min(1000);
    f.violation_recoil = f.violation_recoil.saturating_sub(5);
}

pub fn violate(severity: u16) {
    let mut s = STATE.lock();
    s.violation_recoil = severity;
    s.alignment_streak = 0;
    s.field_strength = s.field_strength.saturating_sub(severity / 2);
    serial_println!("exodus: integrity violation (recoil={})", severity);
}

pub fn tick(f: &mut IntegrityFieldState) {
    f.violation_recoil = f.violation_recoil.saturating_sub(10);
    f.values_coherence = f.values_coherence.saturating_add(1).min(1000);
}
