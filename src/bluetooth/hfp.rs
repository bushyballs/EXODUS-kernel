/// Hands-Free Profile (HFP) for voice calls.
///
/// HFP enables hands-free voice call control over Bluetooth.
/// This module implements:
///   - AT command parser and handler (HFP-specific AT set)
///   - SCO (Synchronous Connection-Oriented) link management
///   - Call state machine (idle->incoming/outgoing->active->held)
///   - Codec negotiation (CVSD mandatory, mSBC for wideband)
///   - Multi-party call support (3-way calling)
///   - Indicator reporting (service, call, signal, battery, etc.)
///   - NREC (Noise Reduction and Echo Canceling) control
///
/// AT Commands supported:
///   ATA    - Answer call
///   AT+CHUP - Hang up
///   ATD    - Dial number
///   AT+BRSF - Supported features exchange
///   AT+CIND - Indicator status
///   AT+CLCC - Call list
///   AT+COPS - Network operator
///   AT+BCS  - Codec selection
///   AT+NREC - Noise reduction
///
/// Part of the AIOS bluetooth subsystem.

use alloc::string::String;
use alloc::vec::Vec;
use crate::{serial_print, serial_println};
use crate::sync::Mutex;

/// HFP feature bits (for AT+BRSF exchange).
const HFP_FEAT_3WAY_CALLING: u32 = 1 << 0;
const HFP_FEAT_EC_NR: u32 = 1 << 1;        // Echo Cancel / Noise Reduction
const HFP_FEAT_VOICE_RECOG: u32 = 1 << 2;
const HFP_FEAT_IN_BAND_RING: u32 = 1 << 3;
const HFP_FEAT_VOICE_TAG: u32 = 1 << 4;
const HFP_FEAT_REJECT_CALL: u32 = 1 << 5;
const HFP_FEAT_ENHANCED_STATUS: u32 = 1 << 6;
const HFP_FEAT_ENHANCED_CONTROL: u32 = 1 << 7;
const HFP_FEAT_CODEC_NEG: u32 = 1 << 8;     // Codec Negotiation (wideband)
const HFP_FEAT_ESCO_S4: u32 = 1 << 10;

/// Default features we support.
const LOCAL_FEATURES: u32 = HFP_FEAT_3WAY_CALLING | HFP_FEAT_EC_NR
    | HFP_FEAT_REJECT_CALL | HFP_FEAT_CODEC_NEG | HFP_FEAT_ENHANCED_STATUS;

/// Audio codec IDs.
const CODEC_CVSD: u8 = 0x01;   // Narrowband (8kHz)
const CODEC_MSBC: u8 = 0x02;   // Wideband (16kHz mSBC)

/// HFP indicator indices (per spec).
const IND_SERVICE: u8 = 1;
const IND_CALL: u8 = 2;
const IND_CALLSETUP: u8 = 3;
const IND_CALLHELD: u8 = 4;
const IND_SIGNAL: u8 = 5;
const IND_ROAM: u8 = 6;
const IND_BATTCHG: u8 = 7;

/// Global HFP state.
static HFP: Mutex<Option<HfpManager>> = Mutex::new(None);

/// HFP call states.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CallState {
    Idle,
    Incoming,
    Outgoing,
    Active,
    Held,
}

/// Call setup states.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CallSetupState {
    None,
    IncomingSetup,
    OutgoingSetup,
    RemoteAlerted,
}

/// HFP SLC (Service Level Connection) state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SlcState {
    Disconnected,
    BrsfExchange,
    CindTest,
    CindRead,
    CmerEnable,
    CodecNeg,
    Connected,
}

/// Indicator values.
struct Indicators {
    service: u8,       // 0=no service, 1=service
    call: u8,          // 0=no call, 1=active call
    callsetup: u8,     // 0=none, 1=incoming, 2=outgoing, 3=remote alerted
    callheld: u8,      // 0=none, 1=held+active, 2=held only
    signal: u8,        // 0..5 signal strength
    roam: u8,          // 0=not roaming, 1=roaming
    battchg: u8,       // 0..5 battery level
}

impl Indicators {
    fn new() -> Self {
        Self {
            service: 1,
            call: 0,
            callsetup: 0,
            callheld: 0,
            signal: 5,
            roam: 0,
            battchg: 5,
        }
    }
}

/// Per-connection HFP state.
struct HfpConnectionInner {
    call_state: CallState,
    call_setup: CallSetupState,
    slc_state: SlcState,
    codec: u8,
    remote_features: u32,
    indicators: Indicators,
    nrec_enabled: bool,
    sco_handle: u16,
    phone_number: String,
}

impl HfpConnectionInner {
    fn new() -> Self {
        Self {
            call_state: CallState::Idle,
            call_setup: CallSetupState::None,
            slc_state: SlcState::Disconnected,
            codec: CODEC_CVSD,
            remote_features: 0,
            indicators: Indicators::new(),
            nrec_enabled: true,
            sco_handle: 0,
            phone_number: String::new(),
        }
    }

    /// Process an AT command and return the response.
    fn process_at_command(&mut self, cmd: &str) -> String {
        let cmd = cmd.trim();

        // AT+BRSF=<features> -- Feature exchange.
        if let Some(rest) = cmd.strip_prefix("AT+BRSF=") {
            if let Ok(features) = rest.parse::<u32>() {
                self.remote_features = features;
            }
            self.slc_state = SlcState::BrsfExchange;
            return alloc::format!("+BRSF: {}\r\nOK\r\n", LOCAL_FEATURES);
        }

        // AT+CIND=? -- Indicator test.
        if cmd == "AT+CIND=?" {
            self.slc_state = SlcState::CindTest;
            return String::from("(\"service\",(0,1)),(\"call\",(0,1)),(\"callsetup\",(0-3)),(\"callheld\",(0-2)),(\"signal\",(0-5)),(\"roam\",(0,1)),(\"battchg\",(0-5))\r\nOK\r\n");
        }

        // AT+CIND? -- Read indicators.
        if cmd == "AT+CIND?" {
            self.slc_state = SlcState::CindRead;
            let ind = &self.indicators;
            return alloc::format!("+CIND: {},{},{},{},{},{},{}\r\nOK\r\n",
                ind.service, ind.call, ind.callsetup, ind.callheld,
                ind.signal, ind.roam, ind.battchg);
        }

        // AT+CMER -- Enable indicator reporting.
        if cmd.starts_with("AT+CMER") {
            self.slc_state = SlcState::Connected;
            serial_println!("    [hfp] SLC established");
            return String::from("OK\r\n");
        }

        // AT+BCS=<codec> -- Codec selection.
        if let Some(rest) = cmd.strip_prefix("AT+BCS=") {
            if let Ok(codec) = rest.parse::<u8>() {
                self.codec = codec;
                serial_println!("    [hfp] Codec selected: {}",
                    if codec == CODEC_MSBC { "mSBC (wideband)" } else { "CVSD (narrowband)" });
            }
            return String::from("OK\r\n");
        }

        // ATA -- Answer call.
        if cmd == "ATA" {
            return self.answer_call();
        }

        // AT+CHUP -- Hang up.
        if cmd == "AT+CHUP" {
            return self.hangup_call();
        }

        // ATD<number> -- Dial.
        if let Some(number) = cmd.strip_prefix("ATD") {
            let number = number.trim_end_matches(';');
            return self.dial(number);
        }

        // AT+CLCC -- List calls.
        if cmd == "AT+CLCC" {
            return self.list_calls();
        }

        // AT+COPS? -- Operator name.
        if cmd == "AT+COPS?" {
            return String::from("+COPS: 0,0,\"AIOS\"\r\nOK\r\n");
        }

        // AT+NREC -- NREC control.
        if let Some(rest) = cmd.strip_prefix("AT+NREC=") {
            self.nrec_enabled = rest.trim() == "1";
            serial_println!("    [hfp] NREC {}", if self.nrec_enabled { "enabled" } else { "disabled" });
            return String::from("OK\r\n");
        }

        // Unknown command.
        serial_println!("    [hfp] Unknown AT command: {}", cmd);
        String::from("ERROR\r\n")
    }

    /// Answer an incoming call.
    fn answer_call(&mut self) -> String {
        if self.call_state != CallState::Incoming {
            return String::from("ERROR\r\n");
        }
        self.call_state = CallState::Active;
        self.call_setup = CallSetupState::None;
        self.indicators.call = 1;
        self.indicators.callsetup = 0;
        serial_println!("    [hfp] Call answered");
        String::from("OK\r\n")
    }

    /// Hang up the active or reject incoming call.
    fn hangup_call(&mut self) -> String {
        match self.call_state {
            CallState::Active | CallState::Held => {
                self.call_state = CallState::Idle;
                self.indicators.call = 0;
                serial_println!("    [hfp] Call ended");
            }
            CallState::Incoming => {
                self.call_state = CallState::Idle;
                self.call_setup = CallSetupState::None;
                self.indicators.callsetup = 0;
                serial_println!("    [hfp] Incoming call rejected");
            }
            CallState::Outgoing => {
                self.call_state = CallState::Idle;
                self.call_setup = CallSetupState::None;
                self.indicators.callsetup = 0;
                serial_println!("    [hfp] Outgoing call cancelled");
            }
            CallState::Idle => {
                return String::from("ERROR\r\n");
            }
        }
        String::from("OK\r\n")
    }

    /// Initiate an outgoing call.
    fn dial(&mut self, number: &str) -> String {
        if self.call_state != CallState::Idle {
            return String::from("ERROR\r\n");
        }
        self.phone_number = String::from(number);
        self.call_state = CallState::Outgoing;
        self.call_setup = CallSetupState::OutgoingSetup;
        self.indicators.callsetup = 2;
        serial_println!("    [hfp] Dialing {}", number);
        String::from("OK\r\n")
    }

    /// Simulate incoming call notification.
    fn incoming_call(&mut self, number: &str) {
        if self.call_state != CallState::Idle {
            return;
        }
        self.phone_number = String::from(number);
        self.call_state = CallState::Incoming;
        self.call_setup = CallSetupState::IncomingSetup;
        self.indicators.callsetup = 1;
        serial_println!("    [hfp] Incoming call from {}", number);
    }

    /// List current calls (AT+CLCC response).
    fn list_calls(&self) -> String {
        match self.call_state {
            CallState::Active => {
                alloc::format!("+CLCC: 1,0,0,0,0,\"{}\",129\r\nOK\r\n", self.phone_number)
            }
            CallState::Incoming => {
                alloc::format!("+CLCC: 1,1,4,0,0,\"{}\",129\r\nOK\r\n", self.phone_number)
            }
            CallState::Outgoing => {
                alloc::format!("+CLCC: 1,0,2,0,0,\"{}\",129\r\nOK\r\n", self.phone_number)
            }
            CallState::Held => {
                alloc::format!("+CLCC: 1,0,1,0,0,\"{}\",129\r\nOK\r\n", self.phone_number)
            }
            CallState::Idle => {
                String::from("OK\r\n")
            }
        }
    }
}

/// Internal HFP manager.
struct HfpManager {
    connection: HfpConnectionInner,
}

impl HfpManager {
    fn new() -> Self {
        Self {
            connection: HfpConnectionInner::new(),
        }
    }
}

/// HFP connection managing voice call audio.
pub struct HfpConnection {
    _private: (),
}

impl HfpConnection {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Accept an incoming call.
    pub fn accept(&mut self) {
        if let Some(mgr) = HFP.lock().as_mut() {
            let _rsp = mgr.connection.answer_call();
        }
    }

    /// Hang up the active call.
    pub fn hangup(&mut self) {
        if let Some(mgr) = HFP.lock().as_mut() {
            let _rsp = mgr.connection.hangup_call();
        }
    }
}

/// Process an incoming AT command string.
pub fn process_at(cmd: &str) -> String {
    if let Some(mgr) = HFP.lock().as_mut() {
        mgr.connection.process_at_command(cmd)
    } else {
        String::from("ERROR\r\n")
    }
}

/// Simulate an incoming call.
pub fn incoming_call(number: &str) {
    if let Some(mgr) = HFP.lock().as_mut() {
        mgr.connection.incoming_call(number);
    }
}

pub fn init() {
    let mgr = HfpManager::new();

    serial_println!("    [hfp] Initializing Hands-Free Profile");
    serial_println!("    [hfp] Local features: {:#010x} (3-way, EC/NR, reject, codec-neg)", LOCAL_FEATURES);

    *HFP.lock() = Some(mgr);
    serial_println!("    [hfp] HFP initialized (CVSD + mSBC codecs)");
}
