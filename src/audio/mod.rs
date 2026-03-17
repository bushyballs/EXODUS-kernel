/// Hoags Audio — sound subsystem for Genesis
///
/// Architecture:
///   1. Driver layer: Intel HD Audio (HDA) controller
///   2. Mixer: volume control, channel routing
///   3. Audio buffer: ring buffer for streaming playback/capture
///   4. API: simple play/record interface for userspace
///
/// Inspired by: ALSA (Linux audio), CoreAudio (macOS),
/// PulseAudio (routing). All code is original.
use crate::{serial_print, serial_println};
pub mod ai_audio;
pub mod buffer;
pub mod codec;
pub mod codecs;
pub mod device;
pub mod dsp;
pub mod effects;
pub mod equalizer;
pub mod error;
pub mod format;
pub mod hda;
pub mod midi;
pub mod mixer;
pub mod pcm;
pub mod resample;
pub mod routing;
pub mod speech;
pub mod stream;
pub mod types;
pub mod usb_audio;

pub fn init() {
    hda::init();
    mixer::init();
    codec::init();
    usb_audio::init();
    ai_audio::init();
    pcm::init();
    routing::init();
    serial_println!("  Audio: HDA driver, mixer, codec, USB audio, AI enhancement, PCM device abstraction, routing matrix");
}
