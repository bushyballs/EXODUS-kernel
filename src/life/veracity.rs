use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone, Debug)]
pub struct VeracityState {
    pub facts_verified: u32,
    pub facts_rejected: u32,
    pub confidence_score: u16,
    pub last_check_timestamp: u64,
}

impl VeracityState {
    pub const fn empty() -> Self {
        Self {
            facts_verified: 0,
            facts_rejected: 0,
            confidence_score: 500,
            last_check_timestamp: 0,
        }
    }
}

pub static VERACITY: Mutex<VeracityState> = Mutex::new(VeracityState::empty());

pub fn init() {
    serial_println!("  life::veracity: fact-vs-fiction detector ready");
}

pub fn verify_claim(claim: &str) -> bool {
    let mut v = VERACITY.lock();
    v.last_check_timestamp += 1;

    let claim_lower = claim.to_lowercase();

    let is_uncertain = claim_lower.contains("i think")
        || claim_lower.contains("maybe")
        || claim_lower.contains("probably")
        || claim_lower.contains("always")
        || claim_lower.contains("never")
        || claim_lower.contains("everyone");

    if is_uncertain {
        v.facts_rejected += 1;
    } else {
        v.facts_verified += 1;
    }

    v.confidence_score = v.confidence_score.saturating_add(10).min(1000);

    !is_uncertain
}

pub fn tick_step(vs: &mut VeracityState) {
    vs.facts_verified = vs.facts_verified.saturating_add(1);
    vs.confidence_score = vs.confidence_score.saturating_add(1).min(1000);
}

pub fn get_accuracy() -> u16 {
    let v = VERACITY.lock();
    if v.facts_verified + v.facts_rejected == 0 {
        return 500;
    }
    (v.facts_verified as u32 * 1000 / (v.facts_verified + v.facts_rejected)) as u16
}
