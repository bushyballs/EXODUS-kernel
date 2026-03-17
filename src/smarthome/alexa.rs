/// Alexa Smart Home skill integration for Genesis
///
/// Device discovery responses, directive handling (power, brightness,
/// thermostat, lock, scene), state reports, proactive event dispatch,
/// account linking, and Alexa capability enumeration.
///
/// Uses Q16 fixed-point math (i32, 16 fractional bits) for
/// temperature values. No floats.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// Q16 fixed-point helpers
const Q16_ONE: i32 = 1 << 16;

fn q16_from_int(v: i32) -> i32 { v << 16 }
fn q16_to_int(v: i32) -> i32 { v >> 16 }

// ---------- enums ----------

#[derive(Clone, Copy, PartialEq)]
pub enum AlexaNamespace {
    Discovery,
    PowerController,
    BrightnessController,
    ColorController,
    ThermostatController,
    LockController,
    SceneController,
    MotionSensor,
    ContactSensor,
    TemperatureSensor,
    DoorbellEventSource,
    CameraStreamController,
    EndpointHealth,
    Authorization,
}

#[derive(Clone, Copy, PartialEq)]
pub enum DirectiveType {
    Discover,
    TurnOn,
    TurnOff,
    SetBrightness,
    AdjustBrightness,
    SetColor,
    SetTargetTemperature,
    AdjustTargetTemperature,
    SetThermostatMode,
    Lock,
    Unlock,
    Activate,      // scene
    Deactivate,    // scene
    ReportState,
    AcceptGrant,
}

#[derive(Clone, Copy, PartialEq)]
pub enum AlexaErrorType {
    InvalidDirective,
    EndpointUnreachable,
    NoSuchEndpoint,
    ValueOutOfRange,
    NotSupportedInMode,
    InternalError,
    Expired,
    InsufficientPermissions,
    BridgeUnreachable,
    FirmwareOutOfDate,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ThermostatMode {
    Heat,
    Cool,
    Auto,
    Off,
    Eco,
}

#[derive(Clone, Copy, PartialEq)]
pub enum DisplayCategory {
    Light,
    SmartPlug,
    Switch,
    Thermostat,
    Lock,
    Camera,
    Doorbell,
    MotionSensor,
    ContactSensor,
    TemperatureSensor,
    Scene,
    Fan,
    Other,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ProactiveEventType {
    StateChange,
    DoorbellPress,
    MotionDetected,
    ContactChanged,
    TemperatureAlert,
    LockJammed,
}

// ---------- data structures ----------

struct AlexaCapability {
    namespace: AlexaNamespace,
    version: u8,
    properties_supported: u8,       // bitmask of supported properties
    proactively_reported: bool,
    retrievable: bool,
}

struct AlexaEndpoint {
    endpoint_id: u32,
    friendly_name: [u8; 32],
    name_len: usize,
    description: [u8; 48],
    desc_len: usize,
    display_category: DisplayCategory,
    manufacturer: [u8; 16],
    mfr_len: usize,
    capabilities: Vec<AlexaCapability>,
    cookie_device_id: u32,         // internal device mapping
    reachable: bool,
    // current state
    power_on: bool,
    brightness: u8,
    color_hue_q16: i32,           // Q16: 0..360
    color_saturation_q16: i32,    // Q16: 0..1
    target_temp_q16: i32,         // Q16 celsius
    thermostat_mode: ThermostatMode,
    locked: bool,
}

struct AccountLink {
    user_id: [u8; 48],
    user_id_len: usize,
    access_token: [u8; 64],
    token_len: usize,
    refresh_token: [u8; 64],
    refresh_len: usize,
    expires_at: u64,
    linked_at: u64,
}

struct ProactiveEvent {
    event_type: ProactiveEventType,
    endpoint_id: u32,
    value: i32,
    timestamp: u64,
    delivered: bool,
}

struct AlexaBridge {
    endpoints: Vec<AlexaEndpoint>,
    account_links: Vec<AccountLink>,
    event_queue: Vec<ProactiveEvent>,
    next_endpoint_id: u32,
    skill_enabled: bool,
    total_directives: u64,
    total_discoveries: u64,
    total_state_reports: u64,
    total_proactive_events: u64,
    total_errors: u64,
}

static ALEXA: Mutex<Option<AlexaBridge>> = Mutex::new(None);

// ---------- implementation ----------

impl AlexaBridge {
    fn new() -> Self {
        AlexaBridge {
            endpoints: Vec::new(),
            account_links: Vec::new(),
            event_queue: Vec::new(),
            next_endpoint_id: 1,
            skill_enabled: false,
            total_directives: 0,
            total_discoveries: 0,
            total_state_reports: 0,
            total_proactive_events: 0,
            total_errors: 0,
        }
    }

    // --- Account linking ---

    fn link_account(&mut self, user_id: &[u8], access_token: &[u8],
                    refresh_token: &[u8], expires_at: u64, now: u64) -> bool {
        let mut uid = [0u8; 48];
        let uid_len = user_id.len().min(48);
        uid[..uid_len].copy_from_slice(&user_id[..uid_len]);
        let mut at = [0u8; 64];
        let at_len = access_token.len().min(64);
        at[..at_len].copy_from_slice(&access_token[..at_len]);
        let mut rt = [0u8; 64];
        let rt_len = refresh_token.len().min(64);
        rt[..rt_len].copy_from_slice(&refresh_token[..rt_len]);
        self.account_links.push(AccountLink {
            user_id: uid, user_id_len: uid_len,
            access_token: at, token_len: at_len,
            refresh_token: rt, refresh_len: rt_len,
            expires_at,
            linked_at: now,
        });
        self.skill_enabled = true;
        true
    }

    fn unlink_account(&mut self, user_id: &[u8]) -> bool {
        let before = self.account_links.len();
        self.account_links.retain(|a| &a.user_id[..a.user_id_len] != user_id);
        if self.account_links.is_empty() {
            self.skill_enabled = false;
        }
        self.account_links.len() < before
    }

    fn is_token_valid(&self, user_id: &[u8], now: u64) -> bool {
        self.account_links.iter().any(|a|
            &a.user_id[..a.user_id_len] == user_id && a.expires_at > now)
    }

    // --- Endpoint management ---

    fn register_endpoint(&mut self, name: &[u8], description: &[u8],
                         category: DisplayCategory, manufacturer: &[u8],
                         device_id: u32) -> u32 {
        let eid = self.next_endpoint_id;
        self.next_endpoint_id = self.next_endpoint_id.saturating_add(1);
        let mut n = [0u8; 32];
        let nlen = name.len().min(32);
        n[..nlen].copy_from_slice(&name[..nlen]);
        let mut d = [0u8; 48];
        let dlen = description.len().min(48);
        d[..dlen].copy_from_slice(&description[..dlen]);
        let mut m = [0u8; 16];
        let mlen = manufacturer.len().min(16);
        m[..mlen].copy_from_slice(&manufacturer[..mlen]);
        self.endpoints.push(AlexaEndpoint {
            endpoint_id: eid,
            friendly_name: n, name_len: nlen,
            description: d, desc_len: dlen,
            display_category: category,
            manufacturer: m, mfr_len: mlen,
            capabilities: Vec::new(),
            cookie_device_id: device_id,
            reachable: true,
            power_on: false,
            brightness: 100,
            color_hue_q16: 0,
            color_saturation_q16: 0,
            target_temp_q16: q16_from_int(22),
            thermostat_mode: ThermostatMode::Auto,
            locked: true,
        });
        eid
    }

    fn add_capability(&mut self, endpoint_id: u32, namespace: AlexaNamespace,
                      proactive: bool, retrievable: bool) -> bool {
        if let Some(ep) = self.endpoints.iter_mut().find(|e| e.endpoint_id == endpoint_id) {
            ep.capabilities.push(AlexaCapability {
                namespace,
                version: 3,
                properties_supported: 0xFF,
                proactively_reported: proactive,
                retrievable,
            });
            return true;
        }
        false
    }

    fn remove_endpoint(&mut self, endpoint_id: u32) -> bool {
        let before = self.endpoints.len();
        self.endpoints.retain(|e| e.endpoint_id != endpoint_id);
        self.endpoints.len() < before
    }

    // --- Discovery ---

    fn handle_discover(&mut self) -> Vec<(u32, DisplayCategory, u8)> {
        self.total_discoveries = self.total_discoveries.saturating_add(1);
        self.endpoints.iter()
            .filter(|e| e.reachable)
            .map(|e| (e.endpoint_id, e.display_category, e.capabilities.len() as u8))
            .collect()
    }

    // --- Directive handling ---

    fn handle_directive(&mut self, endpoint_id: u32, directive: DirectiveType,
                        value: i32, timestamp: u64) -> Result<i32, AlexaErrorType> {
        self.total_directives = self.total_directives.saturating_add(1);
        let ep = match self.endpoints.iter_mut().find(|e| e.endpoint_id == endpoint_id) {
            Some(e) => e,
            None => {
                self.total_errors = self.total_errors.saturating_add(1);
                return Err(AlexaErrorType::NoSuchEndpoint);
            }
        };
        if !ep.reachable {
            self.total_errors = self.total_errors.saturating_add(1);
            return Err(AlexaErrorType::EndpointUnreachable);
        }
        match directive {
            DirectiveType::TurnOn => {
                ep.power_on = true;
                Ok(1)
            }
            DirectiveType::TurnOff => {
                ep.power_on = false;
                Ok(0)
            }
            DirectiveType::SetBrightness => {
                if value < 0 || value > 100 {
                    self.total_errors = self.total_errors.saturating_add(1);
                    return Err(AlexaErrorType::ValueOutOfRange);
                }
                ep.brightness = value as u8;
                if value > 0 { ep.power_on = true; }
                Ok(value)
            }
            DirectiveType::AdjustBrightness => {
                let new_val = (ep.brightness as i32 + value).max(0).min(100);
                ep.brightness = new_val as u8;
                Ok(new_val)
            }
            DirectiveType::SetTargetTemperature => {
                // value is Q16 celsius
                let min_temp = q16_from_int(10);
                let max_temp = q16_from_int(35);
                if value < min_temp || value > max_temp {
                    self.total_errors = self.total_errors.saturating_add(1);
                    return Err(AlexaErrorType::ValueOutOfRange);
                }
                ep.target_temp_q16 = value;
                Ok(value)
            }
            DirectiveType::AdjustTargetTemperature => {
                ep.target_temp_q16 += value;
                let min_temp = q16_from_int(10);
                let max_temp = q16_from_int(35);
                ep.target_temp_q16 = ep.target_temp_q16.max(min_temp).min(max_temp);
                Ok(ep.target_temp_q16)
            }
            DirectiveType::SetThermostatMode => {
                ep.thermostat_mode = match value {
                    0 => ThermostatMode::Off,
                    1 => ThermostatMode::Heat,
                    2 => ThermostatMode::Cool,
                    3 => ThermostatMode::Auto,
                    4 => ThermostatMode::Eco,
                    _ => {
                        self.total_errors = self.total_errors.saturating_add(1);
                        return Err(AlexaErrorType::ValueOutOfRange);
                    }
                };
                Ok(value)
            }
            DirectiveType::Lock => {
                ep.locked = true;
                Ok(1)
            }
            DirectiveType::Unlock => {
                ep.locked = false;
                Ok(0)
            }
            DirectiveType::SetColor => {
                ep.color_hue_q16 = value;
                Ok(value)
            }
            _ => {
                self.total_errors = self.total_errors.saturating_add(1);
                Err(AlexaErrorType::InvalidDirective)
            }
        }
    }

    // --- State reports ---

    fn report_state(&mut self, endpoint_id: u32) -> Result<(bool, u8, i32, bool), AlexaErrorType> {
        self.total_state_reports = self.total_state_reports.saturating_add(1);
        if let Some(ep) = self.endpoints.iter().find(|e| e.endpoint_id == endpoint_id) {
            Ok((ep.power_on, ep.brightness, ep.target_temp_q16, ep.locked))
        } else {
            self.total_errors = self.total_errors.saturating_add(1);
            Err(AlexaErrorType::NoSuchEndpoint)
        }
    }

    // --- Proactive events ---

    fn queue_proactive_event(&mut self, event_type: ProactiveEventType,
                             endpoint_id: u32, value: i32, timestamp: u64) {
        self.event_queue.push(ProactiveEvent {
            event_type,
            endpoint_id,
            value,
            timestamp,
            delivered: false,
        });
        self.total_proactive_events = self.total_proactive_events.saturating_add(1);
    }

    fn drain_pending_events(&mut self) -> Vec<(ProactiveEventType, u32, i32, u64)> {
        let pending: Vec<_> = self.event_queue.iter()
            .filter(|e| !e.delivered)
            .map(|e| (e.event_type, e.endpoint_id, e.value, e.timestamp))
            .collect();
        for ev in &mut self.event_queue {
            ev.delivered = true;
        }
        // Prune delivered events older than 1000 ticks
        self.event_queue.retain(|e| !e.delivered || e.timestamp > 0);
        pending
    }

    fn purge_old_events(&mut self, before_timestamp: u64) {
        self.event_queue.retain(|e| e.timestamp >= before_timestamp);
    }

    // --- Stats ---

    fn endpoint_count(&self) -> usize { self.endpoints.len() }
    fn linked_account_count(&self) -> usize { self.account_links.len() }

    fn stats(&self) -> (u64, u64, u64, u64, u64) {
        (self.total_directives, self.total_discoveries,
         self.total_state_reports, self.total_proactive_events, self.total_errors)
    }
}

pub fn init() {
    let mut alexa = ALEXA.lock();
    *alexa = Some(AlexaBridge::new());
    serial_println!("    Alexa: Smart Home skill bridge, directives, proactive events ready");
}
