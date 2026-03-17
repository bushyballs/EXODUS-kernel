use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum AnomalyAction {
    Log = 0,
    Alert = 1,
    Kill = 2,
}

impl AnomalyAction {
    pub const fn empty() -> Self {
        AnomalyAction::Log
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PolicyConfig {
    pub log_threshold: u16,
    pub alert_threshold: u16,
    pub kill_threshold: u16,
}

impl PolicyConfig {
    pub const fn empty() -> Self {
        Self {
            log_threshold: 0,
            alert_threshold: 0,
            kill_threshold: 0,
        }
    }

    pub const fn default_kernel() -> Self {
        Self {
            log_threshold: 96,
            alert_threshold: 160,
            kill_threshold: 224,
        }
    }
}

pub static POLICY: Mutex<[PolicyConfig; 1]> = Mutex::new([PolicyConfig::default_kernel(); 1]);

pub fn set_policy(config: PolicyConfig) {
    let mut policy = POLICY.lock();
    policy[0] = config;
}

pub fn enforce(pid: u32, score: u16) -> AnomalyAction {
    let cfg = POLICY.lock()[0];
    let action = if score >= cfg.kill_threshold {
        AnomalyAction::Kill
    } else if score >= cfg.alert_threshold {
        AnomalyAction::Alert
    } else {
        AnomalyAction::Log
    };

    match action {
        AnomalyAction::Kill => {
            serial_println!("[ml/policy] ACTION=KILL pid={} score_q8_8={}", pid, score);
        }
        AnomalyAction::Alert => {
            serial_println!("[ml/policy] ACTION=ALERT pid={} score_q8_8={}", pid, score);
        }
        AnomalyAction::Log => {
            if score >= cfg.log_threshold {
                serial_println!("[ml/policy] ACTION=LOG pid={} score_q8_8={}", pid, score);
            } else {
                serial_println!("[ml/policy] pid={} normal score_q8_8={}", pid, score);
            }
        }
    }

    action
}
