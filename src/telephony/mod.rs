pub mod ai_telephony;
/// Telephony framework for Genesis
///
/// Call management, SIM/eSIM, SMS/MMS, USSD,
/// IMS/VoLTE, emergency dialer, call screening,
/// dual-SIM, and AI-powered call features.
///
/// Original implementation for Hoags OS.
pub mod call_manager;
pub mod emergency;
pub mod ims;
pub mod screening;
pub mod sim;
pub mod sms;

use crate::{serial_print, serial_println};

pub fn init() {
    call_manager::init();
    sim::init();
    sms::init();
    ims::init();
    emergency::init();
    screening::init();
    ai_telephony::init();
    serial_println!("  Telephony initialized (calls, SIM, SMS, VoLTE, AI screening)");
}
