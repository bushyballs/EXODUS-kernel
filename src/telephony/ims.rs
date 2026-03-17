/// IMS/VoLTE/VoWiFi for Genesis
///
/// IP Multimedia Subsystem support for voice over LTE,
/// voice over WiFi, video calling over LTE, and RCS.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum ImsState {
    Disconnected,
    Registering,
    Registered,
    InCall,
    Error,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ImsFeature {
    VoLTE,
    VoWiFi,
    ViLTE, // Video over LTE
    Rcs,
    VoNR, // Voice over 5G NR
}

struct ImsRegistration {
    state: ImsState,
    volte_enabled: bool,
    vowifi_enabled: bool,
    vilte_enabled: bool,
    rcs_enabled: bool,
    vonr_enabled: bool,
    registered_features: u8, // bitmask
    retry_count: u32,
    last_registration: u64,
}

struct ImsEngine {
    registration: ImsRegistration,
    call_count: u32,
    handover_count: u32, // LTE <-> WiFi
    quality_score: u32,  // 0-100
}

static IMS_ENGINE: Mutex<Option<ImsEngine>> = Mutex::new(None);

impl ImsEngine {
    fn new() -> Self {
        ImsEngine {
            registration: ImsRegistration {
                state: ImsState::Disconnected,
                volte_enabled: true,
                vowifi_enabled: true,
                vilte_enabled: false,
                rcs_enabled: true,
                vonr_enabled: false,
                registered_features: 0,
                retry_count: 0,
                last_registration: 0,
            },
            call_count: 0,
            handover_count: 0,
            quality_score: 0,
        }
    }

    fn register(&mut self, timestamp: u64) {
        self.registration.state = ImsState::Registering;
        self.registration.last_registration = timestamp;
        // Simulate registration
        self.registration.state = ImsState::Registered;
        let mut features = 0u8;
        if self.registration.volte_enabled {
            features |= 1;
        }
        if self.registration.vowifi_enabled {
            features |= 2;
        }
        if self.registration.vilte_enabled {
            features |= 4;
        }
        if self.registration.rcs_enabled {
            features |= 8;
        }
        if self.registration.vonr_enabled {
            features |= 16;
        }
        self.registration.registered_features = features;
    }

    fn is_feature_available(&self, feature: ImsFeature) -> bool {
        if self.registration.state != ImsState::Registered {
            return false;
        }
        match feature {
            ImsFeature::VoLTE => self.registration.registered_features & 1 != 0,
            ImsFeature::VoWiFi => self.registration.registered_features & 2 != 0,
            ImsFeature::ViLTE => self.registration.registered_features & 4 != 0,
            ImsFeature::Rcs => self.registration.registered_features & 8 != 0,
            ImsFeature::VoNR => self.registration.registered_features & 16 != 0,
        }
    }

    fn handover_to_wifi(&mut self) {
        if self.registration.vowifi_enabled {
            self.handover_count = self.handover_count.saturating_add(1);
        }
    }
}

pub fn init() {
    let mut engine = IMS_ENGINE.lock();
    let mut ims = ImsEngine::new();
    ims.register(0);
    *engine = Some(ims);
    serial_println!("    Telephony: IMS/VoLTE/VoWiFi registered");
}
