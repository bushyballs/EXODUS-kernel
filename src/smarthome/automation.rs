use crate::sync::Mutex;
/// Home automation for Genesis smart home
///
/// Triggers, conditions, actions, routines.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum TriggerType {
    Time,
    DeviceState,
    Location,
    Sunrise,
    Sunset,
    Temperature,
    Motion,
    DoorOpen,
    VoiceCommand,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ActionType {
    TurnOn,
    TurnOff,
    SetBrightness,
    SetTemperature,
    Lock,
    Unlock,
    PlayMedia,
    SendNotification,
    RunScene,
}

struct AutomationRule {
    id: u32,
    name: [u8; 32],
    name_len: usize,
    trigger: TriggerType,
    trigger_value: u32,
    target_device: u32,
    action: ActionType,
    action_value: u32,
    enabled: bool,
    executions: u32,
}

struct HomeAutomation {
    rules: Vec<AutomationRule>,
    next_id: u32,
}

static HOME_AUTO: Mutex<Option<HomeAutomation>> = Mutex::new(None);

impl HomeAutomation {
    fn new() -> Self {
        HomeAutomation {
            rules: Vec::new(),
            next_id: 1,
        }
    }

    fn add_rule(
        &mut self,
        name: &[u8],
        trigger: TriggerType,
        trigger_val: u32,
        device: u32,
        action: ActionType,
        action_val: u32,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut n = [0u8; 32];
        let nlen = name.len().min(32);
        n[..nlen].copy_from_slice(&name[..nlen]);
        self.rules.push(AutomationRule {
            id,
            name: n,
            name_len: nlen,
            trigger,
            trigger_value: trigger_val,
            target_device: device,
            action,
            action_value: action_val,
            enabled: true,
            executions: 0,
        });
        id
    }

    fn check_triggers(&mut self, trigger: TriggerType, value: u32) -> Vec<(u32, ActionType, u32)> {
        let mut actions = Vec::new();
        for rule in self.rules.iter_mut() {
            if rule.enabled && rule.trigger == trigger && rule.trigger_value == value {
                actions.push((rule.target_device, rule.action, rule.action_value));
                rule.executions = rule.executions.saturating_add(1);
            }
        }
        actions
    }
}

pub fn init() {
    let mut a = HOME_AUTO.lock();
    *a = Some(HomeAutomation::new());
    serial_println!("    Smart home: automation engine ready");
}
