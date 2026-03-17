pub mod ai_connectivity;
pub mod cellular;
pub mod hotspot;
pub mod mesh;
/// Advanced connectivity for Genesis
///
/// NFC, UWB, cellular modem, tethering,
/// and VPN management.
///
/// Inspired by: Android Connectivity, iOS Network. All code is original.
pub mod nfc;
pub mod radio_toolkit;
pub mod tethering;
pub mod uwb;
pub mod vpn;
pub mod wifi_direct;

use crate::{serial_print, serial_println};

pub fn init() {
    nfc::init();
    uwb::init();
    cellular::init();
    tethering::init();
    vpn::init();
    ai_connectivity::init();
    radio_toolkit::init();
    wifi_direct::init();
    hotspot::init();
    mesh::init();
    serial_println!("  Advanced connectivity initialized (AI network, radio toolkit, WiFi Direct, hotspot, mesh)");
}
