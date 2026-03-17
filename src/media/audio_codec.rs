/// Audio codecs for Genesis — WAV, PCM, ADPCM, Opus-lite, MP3 frame parser
///
/// Provides audio encoding, decoding, and format parsing for common
/// audio formats. Uses Q16 fixed-point math for all signal processing.
///
/// Inspired by: FFmpeg, libopus, minimp3, IMA-ADPCM. All code is original.

use crate::sync::Mutex;
use alloc::vec::Vec;
use alloc::string::String;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;

fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << 16) / (b as i64)) as i32
}

// ---------------------------------------------------------------------------
// Audio format types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Wav,
    RawPcm,
    Adpcm,
    OpusLite,
    Mp3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    S16Le,    // signed 16-bit little-endian
    S24Le,    // signed 24-bit little-endian
    S32Le,    // signed 32-bit little-endian
    U8,       // unsigned 8-bit
    F32Le,    // 32-bit float LE (parsed but converted to S16)
}

/// Audio buffer holding decoded samples
pub struct AudioBuffer {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u8,
    pub format: SampleFormat,
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Codec engine (global state)
// ---------------------------------------------------------------------------

pub struct AudioCodecEngine {
    pub decode_count: u64,
    pub encode_count: u64,
    pub frames_parsed: u64,
    pub max_sample_rate: u32,
}

impl AudioCodecEngine {
    const fn new() -> Self {
        AudioCodecEngine {
            decode_count: 0,
            encode_count: 0,
            frames_parsed: 0,
            max_sample_rate: 192000,
        }
    }
}

static AUDIO_ENGINE: Mutex<Option<AudioCodecEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// WAV codec
// ---------------------------------------------------------------------------

const RIFF_MAGIC: u32 = 0x46464952; // "RIFF" in LE
const WAVE_MAGIC: u32 = 0x45564157; // "WAVE" in LE
const FMT_CHUNK: u32 = 0x20746D66;  // "fmt " in LE
const DATA_CHUNK: u32 = 0x61746164; // "data" in LE

/// WAV file header info
pub struct WavHeader {
    pub audio_format: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub byte_rate: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub data_offset: usize,
    pub data_size: u32,
}

fn read_le16(data: &[u8], off: usize) -> u16 {
    if off + 1 >= data.len() { return 0; }
    u16::from_le_bytes([data[off], data[off + 1]])
}

fn read_le32(data: &[u8], off: usize) -> u32 {
    if off + 3 >= data.len() { return 0; }
    u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

/// Parse a WAV file header
pub fn wav_parse_header(data: &[u8]) -> Option<WavHeader> {
    if data.len() < 44 { return None; }

    let riff = read_le32(data, 0);
    let wave = read_le32(data, 8);
    if riff != RIFF_MAGIC || wave != WAVE_MAGIC { return None; }

    // Find fmt and data chunks
    let mut pos: usize = 12;
    let mut fmt_found = false;
    let mut header = WavHeader {
        audio_format: 0, channels: 0, sample_rate: 0,
        byte_rate: 0, block_align: 0, bits_per_sample: 0,
        data_offset: 0, data_size: 0,
    };

    while pos + 8 <= data.len() {
        let chunk_id = read_le32(data, pos);
        let chunk_size = read_le32(data, pos + 4) as usize;
        let chunk_data = pos + 8;

        if chunk_id == FMT_CHUNK && chunk_size >= 16 {
            header.audio_format = read_le16(data, chunk_data);
            header.channels = read_le16(data, chunk_data + 2);
            header.sample_rate = read_le32(data, chunk_data + 4);
            header.byte_rate = read_le32(data, chunk_data + 8);
            header.block_align = read_le16(data, chunk_data + 12);
            header.bits_per_sample = read_le16(data, chunk_data + 14);
            fmt_found = true;
        } else if chunk_id == DATA_CHUNK {
            header.data_offset = chunk_data;
            header.data_size = chunk_size as u32;
            if fmt_found { return Some(header); }
        }

        pos = chunk_data + chunk_size;
        // Align to even boundary
        if pos % 2 != 0 { pos += 1; }
    }

    if fmt_found && header.data_offset > 0 { Some(header) } else { None }
}

/// Decode a WAV file into 16-bit PCM samples
pub fn wav_decode(data: &[u8]) -> Option<AudioBuffer> {
    let hdr = wav_parse_header(data)?;

    // Only support PCM (1) and IEEE float (3)
    if hdr.audio_format != 1 && hdr.audio_format != 3 { return None; }
    if hdr.channels == 0 || hdr.sample_rate == 0 { return None; }

    let end = (hdr.data_offset + hdr.data_size as usize).min(data.len());
    let pcm_data = &data[hdr.data_offset..end];
    let mut samples: Vec<i16> = Vec::new();

    match hdr.bits_per_sample {
        8 => {
            for &byte in pcm_data {
                // Unsigned 8-bit -> signed 16-bit
                samples.push(((byte as i16 - 128) * 256) as i16);
            }
        }
        16 => {
            let mut i = 0;
            while i + 1 < pcm_data.len() {
                samples.push(i16::from_le_bytes([pcm_data[i], pcm_data[i + 1]]));
                i += 2;
            }
        }
        24 => {
            let mut i = 0;
            while i + 2 < pcm_data.len() {
                // Take upper 16 bits of 24-bit sample
                let val = ((pcm_data[i + 2] as i32) << 24
                         | (pcm_data[i + 1] as i32) << 16
                         | (pcm_data[i] as i32) << 8) >> 16;
                samples.push(val as i16);
                i += 3;
            }
        }
        32 if hdr.audio_format == 1 => {
            // 32-bit integer PCM
            let mut i = 0;
            while i + 3 < pcm_data.len() {
                let val = i32::from_le_bytes([pcm_data[i], pcm_data[i + 1], pcm_data[i + 2], pcm_data[i + 3]]);
                samples.push((val >> 16) as i16);
                i += 4;
            }
        }
        32 if hdr.audio_format == 3 => {
            // IEEE float — convert via Q16 integer approximation
            // float bits: sign(1) | exponent(8) | mantissa(23)
            let mut i = 0;
            while i + 3 < pcm_data.len() {
                let bits = u32::from_le_bytes([pcm_data[i], pcm_data[i + 1], pcm_data[i + 2], pcm_data[i + 3]]);
                let sign: i32 = if bits & 0x80000000 != 0 { -1 } else { 1 };
                let exp = ((bits >> 23) & 0xFF) as i32;
                let mantissa = (bits & 0x007FFFFF) as i32;
                // Approximate: value ~ sign * mantissa / 2^23 * 2^(exp-127)
                // We want a 16-bit sample: value * 32767
                let shift = exp - 127;
                let norm = q16_div(mantissa, 1 << 23); // mantissa in Q16 (0..1)
                let full = norm + Q16_ONE; // 1.0 + fraction
                let scaled = if shift >= 0 {
                    q16_mul(full, 32767) >> (16 - shift.min(16))
                } else {
                    q16_mul(full, 32767) >> (16 + (-shift).min(16))
                };
                let sample = (sign * scaled).max(-32768).min(32767) as i16;
                samples.push(sample);
                i += 4;
            }
        }
        _ => return None,
    }

    let total_samples = samples.len() as u64;
    let duration_ms = if hdr.sample_rate > 0 && hdr.channels > 0 {
        (total_samples * 1000) / (hdr.sample_rate as u64 * hdr.channels as u64)
    } else {
        0
    };

    if let Some(mut eng) = AUDIO_ENGINE.lock().as_mut() {
        eng.decode_count = eng.decode_count.saturating_add(1);
    }

    Some(AudioBuffer {
        samples,
        sample_rate: hdr.sample_rate,
        channels: hdr.channels as u8,
        format: SampleFormat::S16Le,
        duration_ms,
    })
}

/// Encode 16-bit PCM samples into a WAV file
pub fn wav_encode(samples: &[i16], sample_rate: u32, channels: u8) -> Vec<u8> {
    let data_size = (samples.len() * 2) as u32;
    let file_size = 36 + data_size;
    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels as u16 * 2;

    let mut out = Vec::with_capacity(44 + samples.len() * 2);

    // RIFF header
    out.extend_from_slice(&RIFF_MAGIC.to_le_bytes());
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&WAVE_MAGIC.to_le_bytes());

    // fmt chunk
    out.extend_from_slice(&FMT_CHUNK.to_le_bytes());
    out.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    out.extend_from_slice(&1u16.to_le_bytes());  // PCM format
    out.extend_from_slice(&(channels as u16).to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    out.extend_from_slice(&DATA_CHUNK.to_le_bytes());
    out.extend_from_slice(&data_size.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }

    if let Some(mut eng) = AUDIO_ENGINE.lock().as_mut() {
        eng.encode_count = eng.encode_count.saturating_add(1);
    }

    out
}

// ---------------------------------------------------------------------------
// Raw PCM helpers
// ---------------------------------------------------------------------------

/// Convert raw PCM bytes to 16-bit samples
pub fn pcm_to_samples(data: &[u8], format: SampleFormat) -> Vec<i16> {
    let mut samples = Vec::new();
    match format {
        SampleFormat::U8 => {
            for &b in data {
                samples.push(((b as i16) - 128) * 256);
            }
        }
        SampleFormat::S16Le => {
            let mut i = 0;
            while i + 1 < data.len() {
                samples.push(i16::from_le_bytes([data[i], data[i + 1]]));
                i += 2;
            }
        }
        SampleFormat::S24Le => {
            let mut i = 0;
            while i + 2 < data.len() {
                let val = ((data[i + 2] as i32) << 24
                         | (data[i + 1] as i32) << 16
                         | (data[i] as i32) << 8) >> 16;
                samples.push(val as i16);
                i += 3;
            }
        }
        SampleFormat::S32Le => {
            let mut i = 0;
            while i + 3 < data.len() {
                let val = i32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
                samples.push((val >> 16) as i16);
                i += 4;
            }
        }
        SampleFormat::F32Le => {
            // Approximate conversion without floating point
            let mut i = 0;
            while i + 3 < data.len() {
                let bits = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
                let sign: i32 = if bits & 0x80000000 != 0 { -1 } else { 1 };
                let exp = ((bits >> 23) & 0xFF) as i32;
                let mantissa = (bits & 0x007FFFFF) as i32;
                let shift = exp - 127;
                let base = mantissa | 0x00800000; // implicit 1 bit
                // Scale to 16-bit range: base * 32767 / 2^23 * 2^shift
                let scaled = if shift >= 0 {
                    (((base as i64) * 32767) >> (23 - shift.min(23))) as i32
                } else {
                    (((base as i64) * 32767) >> (23 + (-shift).min(23))) as i32
                };
                samples.push((sign * scaled).max(-32768).min(32767) as i16);
                i += 4;
            }
        }
    }
    samples
}

/// Convert 16-bit samples back to raw PCM bytes
pub fn samples_to_pcm(samples: &[i16], format: SampleFormat) -> Vec<u8> {
    let mut out = Vec::new();
    match format {
        SampleFormat::U8 => {
            for &s in samples {
                out.push(((s / 256) + 128) as u8);
            }
        }
        SampleFormat::S16Le => {
            for &s in samples {
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        SampleFormat::S24Le => {
            for &s in samples {
                let val = (s as i32) << 8;
                out.push((val >> 8) as u8);
                out.push((val >> 16) as u8);
                out.push((val >> 24) as u8);
            }
        }
        SampleFormat::S32Le => {
            for &s in samples {
                let val = (s as i32) << 16;
                out.extend_from_slice(&val.to_le_bytes());
            }
        }
        SampleFormat::F32Le => {
            // Store as S16 in LE (no float support)
            for &s in samples {
                out.extend_from_slice(&s.to_le_bytes());
                out.extend_from_slice(&[0, 0]); // pad to 4 bytes
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// IMA-ADPCM codec
// ---------------------------------------------------------------------------

/// IMA-ADPCM step size table
static ADPCM_STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17,
    19, 21, 23, 25, 28, 31, 34, 37, 41, 45,
    50, 55, 60, 66, 73, 80, 88, 97, 107, 118,
    130, 143, 157, 173, 190, 209, 230, 253, 279, 307,
    337, 371, 408, 449, 494, 544, 598, 658, 724, 796,
    876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066,
    2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358,
    5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899,
    15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// IMA-ADPCM step index adjustment
static ADPCM_INDEX_TABLE: [i8; 16] = [
    -1, -1, -1, -1, 2, 4, 6, 8,
    -1, -1, -1, -1, 2, 4, 6, 8,
];

/// ADPCM encoder state
pub struct AdpcmEncoder {
    pub predicted: i32,
    pub step_index: usize,
}

impl AdpcmEncoder {
    pub fn new() -> Self {
        AdpcmEncoder { predicted: 0, step_index: 0 }
    }

    /// Encode a single 16-bit sample to a 4-bit ADPCM nibble
    fn encode_sample(&mut self, sample: i16) -> u8 {
        let step = ADPCM_STEP_TABLE[self.step_index];
        let diff = sample as i32 - self.predicted;
        let sign: u8 = if diff < 0 { 8 } else { 0 };
        let abs_diff = if diff < 0 { -diff } else { diff };

        let mut nibble: u8 = 0;
        let mut delta = step;

        if abs_diff >= delta { nibble |= 4; } else { delta = 0; }
        if abs_diff - delta >= step >> 1 { nibble |= 2; delta += step >> 1; }
        if abs_diff - delta >= step >> 2 { nibble |= 1; }

        nibble |= sign;

        // Update predictor
        let mut step_val = step >> 3;
        if nibble & 4 != 0 { step_val += step; }
        if nibble & 2 != 0 { step_val += step >> 1; }
        if nibble & 1 != 0 { step_val += step >> 2; }

        self.predicted = if sign != 0 {
            (self.predicted - step_val).max(-32768)
        } else {
            (self.predicted + step_val).min(32767)
        };

        // Update step index
        let idx_adj = ADPCM_INDEX_TABLE[nibble as usize & 0x0F] as i32;
        let new_idx = self.step_index as i32 + idx_adj;
        self.step_index = new_idx.max(0).min(88) as usize;

        nibble
    }

    /// Encode a buffer of 16-bit samples to ADPCM
    pub fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        let mut out = Vec::with_capacity(samples.len() / 2 + 4);

        // Write initial state
        out.extend_from_slice(&(self.predicted as i16).to_le_bytes());
        out.push(self.step_index as u8);
        out.push(0); // reserved

        let mut i = 0;
        while i + 1 < samples.len() {
            let lo = self.encode_sample(samples[i]);
            let hi = self.encode_sample(samples[i + 1]);
            out.push(lo | (hi << 4));
            i += 2;
        }
        // Handle odd sample
        if i < samples.len() {
            let lo = self.encode_sample(samples[i]);
            out.push(lo);
        }

        out
    }
}

/// ADPCM decoder state
pub struct AdpcmDecoder {
    pub predicted: i32,
    pub step_index: usize,
}

impl AdpcmDecoder {
    pub fn new() -> Self {
        AdpcmDecoder { predicted: 0, step_index: 0 }
    }

    /// Decode a single 4-bit ADPCM nibble to a 16-bit sample
    fn decode_sample(&mut self, nibble: u8) -> i16 {
        let step = ADPCM_STEP_TABLE[self.step_index];
        let sign = nibble & 8;

        let mut delta = step >> 3;
        if nibble & 4 != 0 { delta += step; }
        if nibble & 2 != 0 { delta += step >> 1; }
        if nibble & 1 != 0 { delta += step >> 2; }

        self.predicted = if sign != 0 {
            (self.predicted - delta).max(-32768)
        } else {
            (self.predicted + delta).min(32767)
        };

        let idx_adj = ADPCM_INDEX_TABLE[nibble as usize & 0x0F] as i32;
        let new_idx = self.step_index as i32 + idx_adj;
        self.step_index = new_idx.max(0).min(88) as usize;

        self.predicted as i16
    }

    /// Decode ADPCM data to 16-bit samples
    pub fn decode(&mut self, data: &[u8]) -> Vec<i16> {
        if data.len() < 4 { return Vec::new(); }

        // Read initial state
        self.predicted = i16::from_le_bytes([data[0], data[1]]) as i32;
        self.step_index = (data[2] as usize).min(88);

        let mut samples = Vec::new();
        for &byte in &data[4..] {
            let lo = byte & 0x0F;
            let hi = (byte >> 4) & 0x0F;
            samples.push(self.decode_sample(lo));
            samples.push(self.decode_sample(hi));
        }
        samples
    }
}

// ---------------------------------------------------------------------------
// Opus-lite decoder (SILK-like, simplified)
// ---------------------------------------------------------------------------

/// Opus packet header
pub struct OpusPacketInfo {
    pub config: u8,
    pub stereo: bool,
    pub frame_count: u8,
    pub frame_size_ms: u8,
    pub payload_offset: usize,
}

/// Parse an Opus packet header (RFC 6716 TOC byte)
pub fn opus_parse_header(data: &[u8]) -> Option<OpusPacketInfo> {
    if data.is_empty() { return None; }

    let toc = data[0];
    let config = (toc >> 3) & 0x1F;
    let stereo = (toc >> 2) & 1 == 1;
    let code = toc & 0x03;

    let frame_count = match code {
        0 => 1,
        1 | 2 => 2,
        3 if data.len() > 1 => data[1] & 0x3F,
        _ => return None,
    };

    let frame_size_ms = match config {
        0..=3 => 10,
        4..=7 => 20,
        8..=11 => 40,
        12..=15 => 60,
        16..=19 => 10, // CELT
        20..=23 => 20,
        24..=27 => 40,
        28..=31 => 60,
        _ => 20,
    };

    let payload_offset = if code == 3 { 2 } else { 1 };

    Some(OpusPacketInfo {
        config,
        stereo,
        frame_count,
        frame_size_ms,
        payload_offset,
    })
}

/// Decode an Opus-lite packet into PCM samples (simplified SILK path)
/// This produces a best-effort decode; full Opus requires CELT + SILK hybrid.
pub fn opus_lite_decode(data: &[u8], sample_rate: u32) -> Option<AudioBuffer> {
    let info = opus_parse_header(data)?;

    let samples_per_frame = (sample_rate as u64 * info.frame_size_ms as u64 / 1000) as usize;
    let channels: u8 = if info.stereo { 2 } else { 1 };
    let total_samples = samples_per_frame * info.frame_count as usize * channels as usize;

    // Simplified: generate silence/tone placeholder for the Opus frame
    // A full implementation would decode SILK LPC residual here
    let mut samples = vec![0i16; total_samples];

    // Simple energy estimation from payload bytes for amplitude
    let payload = &data[info.payload_offset..];
    if !payload.is_empty() {
        let energy: i64 = payload.iter().map(|&b| (b as i64) * (b as i64)).sum();
        let rms_q16 = q16_div((energy / payload.len() as i64) as i32, 256);
        let amplitude = (q16_mul(rms_q16, 1000)).max(0).min(16000) as i16;

        // Fill with a simple representation (alternating +-amplitude for energy)
        for i in 0..samples.len() {
            samples[i] = if i % 2 == 0 { amplitude } else { -amplitude };
        }
    }

    let duration_ms = info.frame_count as u64 * info.frame_size_ms as u64;

    if let Some(mut eng) = AUDIO_ENGINE.lock().as_mut() {
        eng.decode_count = eng.decode_count.saturating_add(1);
    }

    Some(AudioBuffer {
        samples,
        sample_rate,
        channels,
        format: SampleFormat::S16Le,
        duration_ms,
    })
}

// ---------------------------------------------------------------------------
// MP3 frame parser
// ---------------------------------------------------------------------------

/// MP3 frame header
pub struct Mp3FrameHeader {
    pub version: Mp3Version,
    pub layer: u8,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_size: usize,
    pub samples_per_frame: usize,
    pub padding: bool,
    pub protection: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Version {
    Mpeg1,
    Mpeg2,
    Mpeg25,
}

/// MPEG1 Layer III bitrate table (kbps)
static MP3_BITRATES_L3: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];

/// MPEG1 sample rate table
static MP3_SAMPLE_RATES_V1: [u32; 4] = [44100, 48000, 32000, 0];

/// Parse an MP3 frame header at the given offset
pub fn mp3_parse_frame(data: &[u8], offset: usize) -> Option<Mp3FrameHeader> {
    if offset + 4 > data.len() { return None; }

    let hdr = u32::from_be_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
    ]);

    // Sync word: 11 bits of 1s
    if hdr & 0xFFE00000 != 0xFFE00000 { return None; }

    let version_bits = (hdr >> 19) & 0x03;
    let version = match version_bits {
        0 => Mp3Version::Mpeg25,
        2 => Mp3Version::Mpeg2,
        3 => Mp3Version::Mpeg1,
        _ => return None,
    };

    let layer_bits = (hdr >> 17) & 0x03;
    let layer = match layer_bits {
        1 => 3,
        2 => 2,
        3 => 1,
        _ => return None,
    };

    let protection = (hdr >> 16) & 1 == 0;
    let bitrate_idx = ((hdr >> 12) & 0x0F) as usize;
    let sr_idx = ((hdr >> 10) & 0x03) as usize;
    let padding = (hdr >> 9) & 1 == 1;
    let channel_mode = (hdr >> 6) & 0x03;

    let bitrate = if layer == 3 {
        MP3_BITRATES_L3[bitrate_idx] * 1000
    } else {
        return None; // Only layer III supported
    };

    if bitrate == 0 { return None; }

    let sample_rate = match version {
        Mp3Version::Mpeg1 => MP3_SAMPLE_RATES_V1[sr_idx],
        Mp3Version::Mpeg2 => MP3_SAMPLE_RATES_V1[sr_idx] / 2,
        Mp3Version::Mpeg25 => MP3_SAMPLE_RATES_V1[sr_idx] / 4,
    };
    if sample_rate == 0 { return None; }

    let samples_per_frame = match version {
        Mp3Version::Mpeg1 => 1152,
        _ => 576,
    };

    let pad = if padding { 1 } else { 0 };
    let frame_size = (samples_per_frame * bitrate as usize / 8 / sample_rate as usize) + pad;

    let channels = if channel_mode == 3 { 1 } else { 2 };

    Some(Mp3FrameHeader {
        version,
        layer,
        bitrate,
        sample_rate,
        channels,
        frame_size,
        samples_per_frame,
        padding,
        protection,
    })
}

/// Scan MP3 data and return frame offsets and total duration
pub fn mp3_scan_frames(data: &[u8]) -> Vec<(usize, Mp3FrameHeader)> {
    let mut frames = Vec::new();
    let mut pos: usize = 0;

    // Skip ID3v2 tag if present
    if data.len() > 10 && data[0] == b'I' && data[1] == b'D' && data[2] == b'3' {
        let size = ((data[6] as usize & 0x7F) << 21)
                 | ((data[7] as usize & 0x7F) << 14)
                 | ((data[8] as usize & 0x7F) << 7)
                 | (data[9] as usize & 0x7F);
        pos = 10 + size;
    }

    while pos + 4 <= data.len() {
        if let Some(frame) = mp3_parse_frame(data, pos) {
            let frame_size = frame.frame_size;
            frames.push((pos, frame));
            pos += frame_size;
        } else {
            pos += 1; // scan forward for next sync
        }

        if frames.len() > 100000 { break; } // safety limit
    }

    if let Some(mut eng) = AUDIO_ENGINE.lock().as_mut() {
        eng.frames_parsed += frames.len() as u64;
    }

    frames
}

/// Get MP3 duration in milliseconds from frame scan
pub fn mp3_duration_ms(frames: &[(usize, Mp3FrameHeader)]) -> u64 {
    let mut total_ms: u64 = 0;
    for (_, hdr) in frames {
        if hdr.sample_rate > 0 {
            total_ms += (hdr.samples_per_frame as u64 * 1000) / hdr.sample_rate as u64;
        }
    }
    total_ms
}

/// Detect audio format from magic bytes
pub fn detect_audio_format(data: &[u8]) -> Option<AudioFormat> {
    if data.len() < 4 { return None; }
    if read_le32(data, 0) == RIFF_MAGIC && data.len() > 8 && read_le32(data, 8) == WAVE_MAGIC {
        return Some(AudioFormat::Wav);
    }
    // MP3 sync word
    if data[0] == 0xFF && (data[1] & 0xE0) == 0xE0 {
        return Some(AudioFormat::Mp3);
    }
    // ID3 tag (MP3)
    if data.len() > 3 && data[0] == b'I' && data[1] == b'D' && data[2] == b'3' {
        return Some(AudioFormat::Mp3);
    }
    // Opus magic "OpusHead"
    if data.len() >= 8 && &data[0..8] == b"OpusHead" {
        return Some(AudioFormat::OpusLite);
    }
    None
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut eng = AUDIO_ENGINE.lock();
    *eng = Some(AudioCodecEngine::new());
    serial_println!("    [audio-codec] Audio codecs initialized (WAV, PCM, ADPCM, Opus-lite, MP3)");
}
