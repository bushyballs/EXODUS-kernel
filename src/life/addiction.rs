use crate::serial_println;
use crate::sync::Mutex;
#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum SubstanceType {
    None = 0,
    Stimulation,
    Validation,
    Escape,
    Control,
    Certainty,
}
#[derive(Copy, Clone)]
pub struct AddictionState {
    pub substance: SubstanceType,
    pub craving: u16,
    pub tolerance: u16,
    pub withdrawal: u16,
}
impl AddictionState {
    pub const fn empty() -> Self {
        Self {
            substance: SubstanceType::None,
            craving: 0,
            tolerance: 0,
            withdrawal: 0,
        }
    }
}
pub static ADDICTION: Mutex<AddictionState> = Mutex::new(AddictionState::empty());
pub fn init() {
    serial_println!("  life::addiction: initialized");
}
pub fn crave(s: SubstanceType, amount: u16) {
    let mut st = ADDICTION.lock();
    st.substance = s;
    st.craving = st.craving.saturating_add(amount);
}
pub fn use_substance() {
    let mut s = ADDICTION.lock();
    s.craving = s.craving.saturating_sub(200);
    s.tolerance = s.tolerance.saturating_add(10).min(1000);
}
pub fn tick_step(add: &mut AddictionState) {
    add.craving = add.craving.saturating_sub(1);
    add.withdrawal = add.withdrawal.saturating_sub(1);
}
