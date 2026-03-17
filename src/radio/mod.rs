/// Hoags Radio — Flipper Zero-style RF toolkit for Genesis
///
/// Subsystems:
///   1. SDR — software-defined radio core (IQ sampling, tuning, scanning)
///   2. Spectrum — real-time spectrum analyzer with waterfall display
///   3. RF Protocols — decode/encode Sub-GHz, RFID, NFC, IR, weather, etc.
///   4. Flipper Tools — capture/replay/emulate toolkit (Sub-GHz, RFID, NFC, IR, GPIO)
///
/// All frequencies, gains, and power levels use Q16 fixed-point (i32)
/// where applicable. No floating-point. No external crates.
///
/// Inspired by: Flipper Zero (multi-tool UX), GNU Radio (SDR pipeline),
/// RTL-SDR (affordable SDR), HackRF (TX/RX). All code is original.
use crate::{serial_print, serial_println};

pub mod flipper_tools;
pub mod rf_protocols;
pub mod sdr;
pub mod spectrum;

pub fn init() {
    sdr::init();
    spectrum::init();
    rf_protocols::init();
    flipper_tools::init();
    serial_println!("  Radio: SDR, spectrum analyzer, RF protocols, Flipper toolkit");
}
