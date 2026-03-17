use crate::sync::Mutex;
/// Audio mixer — software mixing, volume control, and channel routing
///
/// Manages:
///   - Software mixer: 16 simultaneous streams, ring-buffer per stream
///   - Master volume
///   - Per-channel volume
///   - Mute state
///   - Channel routing (which output goes where)
///   - PCM format conversion (u8/i16/i24/i32 → i16, integer math only)
///   - Sample-rate conversion (linear interpolation, integer math only)
///   - Volume ramp (fade in/out, prevents clicks)
use crate::{serial_print, serial_println};

static MIXER: Mutex<AudioMixer> = Mutex::new(AudioMixer::new());

const MAX_CHANNELS: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct ChannelState {
    pub volume: u8, // 0-100
    pub muted: bool,
    pub balance: i8, // -100 (left) to +100 (right)
}

impl ChannelState {
    const fn default() -> Self {
        ChannelState {
            volume: 75,
            muted: false,
            balance: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Master,
    Pcm,    // applications
    System, // system sounds
    Notification,
    Voice, // voice calls
    Microphone,
}

pub struct AudioMixer {
    channels: [ChannelState; MAX_CHANNELS],
    master: ChannelState,
}

impl AudioMixer {
    const fn new() -> Self {
        AudioMixer {
            channels: [ChannelState::default(); MAX_CHANNELS],
            master: ChannelState {
                volume: 80,
                muted: false,
                balance: 0,
            },
        }
    }

    fn channel_index(ch: Channel) -> usize {
        match ch {
            Channel::Master => 0,
            Channel::Pcm => 1,
            Channel::System => 2,
            Channel::Notification => 3,
            Channel::Voice => 4,
            Channel::Microphone => 5,
        }
    }

    pub fn set_volume(&mut self, ch: Channel, volume: u8) {
        let vol = volume.min(100);
        if matches!(ch, Channel::Master) {
            self.master.volume = vol;
        } else {
            self.channels[Self::channel_index(ch)].volume = vol;
        }
    }

    pub fn get_volume(&self, ch: Channel) -> u8 {
        if matches!(ch, Channel::Master) {
            self.master.volume
        } else {
            self.channels[Self::channel_index(ch)].volume
        }
    }

    pub fn set_mute(&mut self, ch: Channel, muted: bool) {
        if matches!(ch, Channel::Master) {
            self.master.muted = muted;
        } else {
            self.channels[Self::channel_index(ch)].muted = muted;
        }
    }

    pub fn is_muted(&self, ch: Channel) -> bool {
        if matches!(ch, Channel::Master) {
            self.master.muted
        } else {
            self.master.muted || self.channels[Self::channel_index(ch)].muted
        }
    }

    /// Calculate effective volume for a channel (considering master)
    pub fn effective_volume(&self, ch: Channel) -> u8 {
        if self.is_muted(ch) {
            return 0;
        }
        let ch_vol = self.channels[Self::channel_index(ch)].volume as u16;
        let master_vol = self.master.volume as u16;
        ((ch_vol * master_vol) / 100) as u8
    }

    /// Apply volume to audio samples (16-bit signed)
    pub fn apply_volume(&self, ch: Channel, samples: &mut [i16]) {
        let vol = self.effective_volume(ch) as i32;
        for sample in samples.iter_mut() {
            *sample = ((*sample as i32 * vol) / 100) as i16;
        }
    }
}

pub fn init() {
    serial_println!(
        "    [mixer] Audio mixer initialized (master: {}%); sw-mixer: {} streams @ {} Hz, pcm-convert, resample",
        MIXER.lock().master.volume,
        SW_MIX_STREAMS,
        SAMPLE_RATE,
    );
}

pub fn set_volume(ch: Channel, vol: u8) {
    MIXER.lock().set_volume(ch, vol);
}

pub fn get_volume(ch: Channel) -> u8 {
    MIXER.lock().get_volume(ch)
}

pub fn set_mute(ch: Channel, muted: bool) {
    MIXER.lock().set_mute(ch, muted);
}

// =============================================================================
// Software mixer — up to 16 simultaneous audio streams
// =============================================================================

/// Number of simultaneous streams the software mixer supports.
const SW_MIX_STREAMS: usize = 16;
/// Number of stereo frames per mixing quantum (~21 ms at 48 kHz).
/// Public so the HDA driver can compute BDL segment sizes.
pub const BUFFER_FRAMES: usize = 1024;
/// Total samples per buffer quantum (frames × 2 channels).
const BUFFER_SAMPLES: usize = BUFFER_FRAMES * 2;
/// Hardware sample rate.
const SAMPLE_RATE: u32 = 48000;

/// One audio stream inside the software mixer.
pub struct AudioStream {
    /// Unique stream identifier (non-zero means active).
    pub id: u32,
    /// Ring-buffer holding interleaved i16 stereo samples.
    buf: [i16; BUFFER_SAMPLES],
    /// Next read position inside `buf`.
    pub read_pos: usize,
    /// Next write position inside `buf`.
    pub write_pos: usize,
    /// Per-stream volume (0 = silent, 255 = full).
    pub volume: u8,
    /// Whether this stream is contributing to the mix.
    pub active: bool,
    /// Volume ramp state: current gain in Q8 (0..255 scaled by 256).
    ramp_current_q8: i32,
    /// Volume ramp target in Q8.
    ramp_target_q8: i32,
    /// Ramp step per sample in Q8 (positive = fade in, negative = fade out).
    ramp_step_q8: i32,
}

impl AudioStream {
    const fn new() -> Self {
        AudioStream {
            id: 0,
            buf: [0i16; BUFFER_SAMPLES],
            read_pos: 0,
            write_pos: 0,
            volume: 255,
            active: false,
            ramp_current_q8: 0,
            ramp_target_q8: 255 * 256,
            ramp_step_q8: 0,
        }
    }

    /// Available samples to read from this stream's ring buffer.
    fn available(&self) -> usize {
        if self.write_pos >= self.read_pos {
            self.write_pos - self.read_pos
        } else {
            BUFFER_SAMPLES - self.read_pos + self.write_pos
        }
    }

    /// Free space for writing into this stream's ring buffer.
    fn free_space(&self) -> usize {
        BUFFER_SAMPLES - self.available() - 1
    }

    /// Read the next sample from the ring buffer, advancing read_pos.
    fn read_sample(&mut self) -> i16 {
        if self.read_pos == self.write_pos {
            return 0; // underrun — silence
        }
        let s = self.buf[self.read_pos];
        self.read_pos = (self.read_pos + 1) % BUFFER_SAMPLES;
        s
    }

    /// Begin a fade-in over `ramp_samples` samples.
    pub fn fade_in(&mut self, ramp_samples: usize) {
        self.ramp_current_q8 = 0;
        self.ramp_target_q8 = self.volume as i32 * 256;
        if ramp_samples == 0 {
            self.ramp_step_q8 = 0;
            self.ramp_current_q8 = self.ramp_target_q8;
        } else {
            self.ramp_step_q8 =
                (self.ramp_target_q8 - self.ramp_current_q8) / ramp_samples.max(1) as i32;
        }
    }

    /// Begin a fade-out over `ramp_samples` samples.
    pub fn fade_out(&mut self, ramp_samples: usize) {
        self.ramp_target_q8 = 0;
        if ramp_samples == 0 {
            self.ramp_step_q8 = 0;
            self.ramp_current_q8 = 0;
        } else {
            self.ramp_step_q8 = -((self.ramp_current_q8) / ramp_samples.max(1) as i32);
        }
    }
}

/// Software mixer: accumulates all active streams into a single output buffer.
pub struct SoftwareMixer {
    streams: [AudioStream; SW_MIX_STREAMS],
    /// i32 accumulation buffer (provides headroom before clipping).
    accum: [i32; BUFFER_SAMPLES],
    /// Global master volume (0 = silent, 255 = full).
    pub master_volume: u8,
    /// Next ID to assign (starts at 1).
    next_id: u32,
}

impl SoftwareMixer {
    const fn new() -> Self {
        const EMPTY: AudioStream = AudioStream::new();
        SoftwareMixer {
            streams: [EMPTY; SW_MIX_STREAMS],
            accum: [0i32; BUFFER_SAMPLES],
            master_volume: 200,
            next_id: 1,
        }
    }

    /// Mix one quantum of audio into `output` (must be BUFFER_SAMPLES long).
    /// Returns the number of samples written.
    pub fn mix_frame(&mut self, output: &mut [i16]) -> usize {
        let n = output.len().min(BUFFER_SAMPLES);

        // Zero the accumulation buffer.
        for s in self.accum[..n].iter_mut() {
            *s = 0;
        }

        // Sum all active streams.
        for stream in self.streams.iter_mut() {
            if !stream.active || stream.id == 0 {
                continue;
            }
            let avail = stream.available();
            if avail == 0 {
                continue;
            }
            let to_read = n.min(avail);
            for i in 0..to_read {
                let raw = stream.read_sample() as i32;

                // Apply volume ramp (Q8 fixed-point).
                let gain = stream.ramp_current_q8;
                let scaled = (raw * gain) >> 8;

                // Advance ramp.
                if stream.ramp_step_q8 != 0 {
                    stream.ramp_current_q8 += stream.ramp_step_q8;
                    // Clamp to [0, ramp_target_q8] depending on direction.
                    if stream.ramp_step_q8 > 0 && stream.ramp_current_q8 >= stream.ramp_target_q8 {
                        stream.ramp_current_q8 = stream.ramp_target_q8;
                        stream.ramp_step_q8 = 0;
                    } else if stream.ramp_step_q8 < 0 && stream.ramp_current_q8 <= 0 {
                        stream.ramp_current_q8 = 0;
                        stream.ramp_step_q8 = 0;
                        // Fade-out complete; mark inactive.
                        if stream.ramp_target_q8 == 0 {
                            stream.active = false;
                        }
                    }
                }

                // Per-stream volume (volume field acts as secondary gain).
                let final_sample = (scaled * stream.volume as i32) / 255;
                self.accum[i] = self.accum[i].saturating_add(final_sample);
            }
        }

        // Apply master volume and hard-clip to i16.
        for i in 0..n {
            let scaled = (self.accum[i] * self.master_volume as i32) / 255;
            output[i] = scaled.clamp(-32768, 32767) as i16;
        }

        n
    }

    /// Allocate a new stream slot and return its ID, or None if all slots are full.
    pub fn add_stream(&mut self) -> Option<u32> {
        for slot in self.streams.iter_mut() {
            if slot.id == 0 {
                let id = self.next_id;
                self.next_id = self.next_id.saturating_add(1);
                *slot = AudioStream::new();
                slot.id = id;
                slot.active = true;
                // Default: instant fade-in (no ramp).
                slot.ramp_current_q8 = 255 * 256;
                slot.ramp_target_q8 = 255 * 256;
                serial_println!("    [sw_mix] stream {} allocated", id);
                return Some(id);
            }
        }
        serial_println!("    [sw_mix] add_stream: no free slots");
        None
    }

    /// Release a stream by ID, fading out before deactivating.
    pub fn remove_stream(&mut self, id: u32) {
        for slot in self.streams.iter_mut() {
            if slot.id == id {
                // Hard stop: clear the ring buffer and mark the slot free.
                *slot = AudioStream::new();
                serial_println!("    [sw_mix] stream {} removed", id);
                return;
            }
        }
    }

    /// Write PCM i16 samples into a stream's ring buffer.
    /// Returns the number of samples actually written (may be less than requested
    /// if the ring buffer is full).
    pub fn write_stream(&mut self, id: u32, samples: &[i16]) -> usize {
        for slot in self.streams.iter_mut() {
            if slot.id != id {
                continue;
            }
            let free = slot.free_space();
            let to_write = samples.len().min(free);
            for i in 0..to_write {
                slot.buf[slot.write_pos] = samples[i];
                slot.write_pos = (slot.write_pos + 1) % BUFFER_SAMPLES;
            }
            return to_write;
        }
        0 // stream not found
    }

    /// Set per-stream volume without clicking (triggers a short ramp).
    pub fn set_stream_volume(&mut self, id: u32, volume: u8) {
        const RAMP: usize = 128; // ~2.7 ms at 48 kHz
        for slot in self.streams.iter_mut() {
            if slot.id == id {
                slot.volume = volume;
                let target = volume as i32 * 256;
                slot.ramp_target_q8 = target;
                if slot.ramp_current_q8 < target {
                    slot.ramp_step_q8 = (target - slot.ramp_current_q8) / RAMP as i32;
                } else {
                    slot.ramp_step_q8 = (target - slot.ramp_current_q8) / RAMP as i32;
                }
                return;
            }
        }
    }

    /// Number of active streams currently in the mixer.
    pub fn active_stream_count(&self) -> usize {
        self.streams
            .iter()
            .filter(|s| s.active && s.id != 0)
            .count()
    }
}

// =============================================================================
// PCM format conversion — integer math, no float
// =============================================================================

/// Convert unsigned 8-bit PCM samples (0..255) to signed 16-bit (-32768..32767).
pub fn u8_to_i16(input: &[u8], output: &mut [i16]) -> usize {
    let n = input.len().min(output.len());
    for i in 0..n {
        // u8 range [0, 255] → i16 range [-32768, 32767]
        // value = (sample as i16 - 128) * 256
        output[i] = ((input[i] as i16).wrapping_sub(128)).wrapping_mul(256);
    }
    n
}

/// Convert signed 16-bit PCM (native endian) to signed 16-bit — identity,
/// but copies so the output buffer signature is uniform.
pub fn i16_to_i16(input: &[i16], output: &mut [i16]) -> usize {
    let n = input.len().min(output.len());
    output[..n].copy_from_slice(&input[..n]);
    n
}

/// Convert signed 24-bit PCM (3 bytes little-endian per sample) to i16.
/// Each 24-bit sample is sign-extended then right-shifted by 8.
pub fn i24_to_i16(input: &[u8], output: &mut [i16]) -> usize {
    let n_samples = (input.len() / 3).min(output.len());
    for i in 0..n_samples {
        let b0 = input[i * 3] as i32;
        let b1 = input[i * 3 + 1] as i32;
        let b2 = input[i * 3 + 2] as i32;
        // Reconstruct signed 24-bit value (little-endian).
        let mut v = b0 | (b1 << 8) | (b2 << 16);
        // Sign-extend from bit 23.
        if v & 0x0080_0000 != 0 {
            v |= -0x0100_0000i32; // fill upper 8 bits with 1s
        }
        // Scale to i16: discard the lowest 8 bits.
        output[i] = (v >> 8) as i16;
    }
    n_samples
}

/// Convert signed 32-bit PCM (little-endian) to i16.
/// Discards the lower 16 bits (simple truncation, integer shift).
pub fn i32_to_i16(input: &[i32], output: &mut [i16]) -> usize {
    let n = input.len().min(output.len());
    for i in 0..n {
        output[i] = (input[i] >> 16) as i16;
    }
    n
}

// =============================================================================
// Sample-rate conversion — linear interpolation, integer Q16 math, no float
// =============================================================================

/// Convert `input` sampled at `src_rate` Hz to `output` at `dst_rate` Hz.
///
/// Uses integer linear interpolation (Q16 fractional position).
/// Returns the number of output samples written.
///
/// Both `input` and `output` are mono (single channel).  For stereo, call
/// twice — once per channel — or interleave before calling.
pub fn resample_mono(input: &[i16], src_rate: u32, output: &mut [i16], dst_rate: u32) -> usize {
    if src_rate == dst_rate {
        let n = input.len().min(output.len());
        output[..n].copy_from_slice(&input[..n]);
        return n;
    }
    if input.is_empty() || output.is_empty() || src_rate == 0 || dst_rate == 0 {
        return 0;
    }

    // step_q16 = (src_rate << 16) / dst_rate — how far to advance in `input`
    // per output sample (Q16 fixed-point).
    let step_q16: u64 = ((src_rate as u64) << 16) / dst_rate as u64;

    // pos_q16: current read position in `input`, Q16.
    let mut pos_q16: u64 = 0;
    let mut out_idx = 0usize;
    let max_pos_q16 = ((input.len() as u64).saturating_sub(1)) << 16;

    while out_idx < output.len() && pos_q16 <= max_pos_q16 {
        let idx = (pos_q16 >> 16) as usize;
        let frac = (pos_q16 & 0xFFFF) as i32; // 0..65535

        let s0 = input[idx] as i32;
        let s1 = if idx + 1 < input.len() {
            input[idx + 1] as i32
        } else {
            s0
        };

        // Interpolate: s0 + (s1 - s0) * frac / 65536
        let interp = s0 + (((s1 - s0) * frac) >> 16);
        output[out_idx] = interp.clamp(-32768, 32767) as i16;
        out_idx += 1;

        pos_q16 += step_q16;
    }

    out_idx
}

/// Resample stereo interleaved audio from `src_rate` to `dst_rate`.
/// Input and output are interleaved L/R pairs.
/// Returns the number of *samples* (not frames) written.
pub fn resample_stereo(input: &[i16], src_rate: u32, output: &mut [i16], dst_rate: u32) -> usize {
    if src_rate == dst_rate {
        let n = input.len().min(output.len());
        output[..n].copy_from_slice(&input[..n]);
        return n;
    }
    if input.len() < 2 || output.is_empty() || src_rate == 0 || dst_rate == 0 {
        return 0;
    }

    let in_frames = input.len() / 2;
    let out_frames = output.len() / 2;
    let step_q16: u64 = ((src_rate as u64) << 16) / dst_rate as u64;
    let mut pos_q16: u64 = 0;
    let mut out_frame = 0usize;
    let max_frame = in_frames.saturating_sub(1) as u64;

    while out_frame < out_frames && (pos_q16 >> 16) <= max_frame {
        let idx = (pos_q16 >> 16) as usize;
        let frac = (pos_q16 & 0xFFFF) as i32;

        // Left channel.
        let l0 = input[idx * 2] as i32;
        let l1 = if idx + 1 < in_frames {
            input[(idx + 1) * 2] as i32
        } else {
            l0
        };
        let l = l0 + (((l1 - l0) * frac) >> 16);

        // Right channel.
        let r0 = input[idx * 2 + 1] as i32;
        let r1 = if idx + 1 < in_frames {
            input[(idx + 1) * 2 + 1] as i32
        } else {
            r0
        };
        let r = r0 + (((r1 - r0) * frac) >> 16);

        output[out_frame * 2] = l.clamp(-32768, 32767) as i16;
        output[out_frame * 2 + 1] = r.clamp(-32768, 32767) as i16;
        out_frame += 1;
        pos_q16 += step_q16;
    }

    out_frame * 2
}

// =============================================================================
// Global software mixer singleton
// =============================================================================

static SW_MIXER: Mutex<SoftwareMixer> = Mutex::new(SoftwareMixer::new());

/// Mix one output quantum into `output`.
pub fn sw_mix_frame(output: &mut [i16]) -> usize {
    SW_MIXER.lock().mix_frame(output)
}

/// Allocate a new stream and return its ID.
pub fn sw_add_stream() -> Option<u32> {
    SW_MIXER.lock().add_stream()
}

/// Remove (free) a stream by ID.
pub fn sw_remove_stream(id: u32) {
    SW_MIXER.lock().remove_stream(id);
}

/// Write samples into a stream.  Returns bytes actually written.
pub fn sw_write_stream(id: u32, samples: &[i16]) -> usize {
    SW_MIXER.lock().write_stream(id, samples)
}

/// Set per-stream volume (0-255).
pub fn sw_set_stream_volume(id: u32, volume: u8) {
    SW_MIXER.lock().set_stream_volume(id, volume);
}

/// Set global master volume (0-255).
pub fn sw_set_master_volume(volume: u8) {
    SW_MIXER.lock().master_volume = volume;
}

/// Return number of active streams.
pub fn sw_active_streams() -> usize {
    SW_MIXER.lock().active_stream_count()
}
