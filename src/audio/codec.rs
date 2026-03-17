use alloc::vec::Vec;

/// Audio sample format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    U8,
    S16Le,
    S16Be,
    S24Le,
    S32Le,
    F32Le,
}

impl SampleFormat {
    pub fn bytes_per_sample(&self) -> usize {
        match self {
            SampleFormat::U8 => 1,
            SampleFormat::S16Le | SampleFormat::S16Be => 2,
            SampleFormat::S24Le => 3,
            SampleFormat::S32Le | SampleFormat::F32Le => 4,
        }
    }
}

/// Audio codec type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
    Pcm,
    Adpcm,
    /// Simple lossy compression (our own design)
    GenesisAudio,
}

/// Codec parameters
#[derive(Debug, Clone, Copy)]
pub struct CodecParams {
    pub codec: CodecType,
    pub sample_rate: u32,
    pub channels: u8,
    pub format: SampleFormat,
    pub bitrate: u32, // bits per second (for compressed)
}

impl CodecParams {
    pub fn cd_quality() -> Self {
        CodecParams {
            codec: CodecType::Pcm,
            sample_rate: 44100,
            channels: 2,
            format: SampleFormat::S16Le,
            bitrate: 44100 * 2 * 16,
        }
    }

    pub fn voice() -> Self {
        CodecParams {
            codec: CodecType::Pcm,
            sample_rate: 16000,
            channels: 1,
            format: SampleFormat::S16Le,
            bitrate: 16000 * 1 * 16,
        }
    }
}

/// ADPCM state for encoding/decoding
pub struct AdpcmState {
    predicted: i16,
    step_index: u8,
}

/// IMA ADPCM step size table
static STEP_TABLE: [i16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

static INDEX_TABLE: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

impl AdpcmState {
    pub fn new() -> Self {
        AdpcmState {
            predicted: 0,
            step_index: 0,
        }
    }

    /// Encode one sample to 4-bit ADPCM
    pub fn encode_sample(&mut self, sample: i16) -> u8 {
        let step = STEP_TABLE[self.step_index as usize];
        let mut diff = sample as i32 - self.predicted as i32;
        let mut code: u8 = 0;

        if diff < 0 {
            code = 8;
            diff = -diff;
        }

        if diff >= step as i32 {
            code |= 4;
            diff -= step as i32;
        }
        if diff >= (step >> 1) as i32 {
            code |= 2;
            diff -= (step >> 1) as i32;
        }
        if diff >= (step >> 2) as i32 {
            code |= 1;
        }

        // Update predictor
        self.decode_sample(code);
        code
    }

    /// Decode one 4-bit ADPCM code to sample
    pub fn decode_sample(&mut self, code: u8) -> i16 {
        let step = STEP_TABLE[self.step_index as usize] as i32;
        let mut diff = step >> 3;

        if code & 4 != 0 {
            diff += step;
        }
        if code & 2 != 0 {
            diff += step >> 1;
        }
        if code & 1 != 0 {
            diff += step >> 2;
        }
        if code & 8 != 0 {
            diff = -diff;
        }

        let mut predicted = self.predicted as i32 + diff;
        predicted = predicted.max(-32768).min(32767);
        self.predicted = predicted as i16;

        let idx = self.step_index as i8 + INDEX_TABLE[(code & 0x0F) as usize];
        self.step_index = idx.max(0).min(88) as u8;

        self.predicted
    }
}

/// Encode PCM samples to ADPCM
pub fn adpcm_encode(pcm: &[i16]) -> Vec<u8> {
    let mut state = AdpcmState::new();
    let mut output = Vec::with_capacity(pcm.len() / 2 + 1);

    for chunk in pcm.chunks(2) {
        let hi = state.encode_sample(chunk[0]);
        let lo = if chunk.len() > 1 {
            state.encode_sample(chunk[1])
        } else {
            0
        };
        output.push((hi << 4) | lo);
    }
    output
}

/// Decode ADPCM to PCM samples
pub fn adpcm_decode(adpcm: &[u8], num_samples: usize) -> Vec<i16> {
    let mut state = AdpcmState::new();
    let mut output = Vec::with_capacity(num_samples);

    for &byte in adpcm {
        if output.len() >= num_samples {
            break;
        }
        output.push(state.decode_sample(byte >> 4));
        if output.len() >= num_samples {
            break;
        }
        output.push(state.decode_sample(byte & 0x0F));
    }
    output
}

/// Sample rate converter (linear interpolation)
pub fn resample(input: &[i16], in_rate: u32, out_rate: u32) -> Vec<i16> {
    if in_rate == out_rate {
        return input.to_vec();
    }
    let ratio = in_rate as f64 / out_rate as f64;
    let out_len = ((input.len() as f64) / ratio) as usize;
    let mut output = Vec::with_capacity(out_len);

    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;

        if idx + 1 < input.len() {
            let sample = input[idx] as f64 * (1.0 - frac) + input[idx + 1] as f64 * frac;
            output.push(sample as i16);
        } else if idx < input.len() {
            output.push(input[idx]);
        }
    }
    output
}

/// Channel mixer — convert between mono/stereo
pub fn mix_channels(input: &[i16], in_channels: u8, out_channels: u8) -> Vec<i16> {
    if in_channels == out_channels {
        return input.to_vec();
    }

    let mut output = Vec::new();
    if in_channels == 2 && out_channels == 1 {
        // Stereo to mono: average L+R
        for chunk in input.chunks(2) {
            if chunk.len() == 2 {
                let mono = ((chunk[0] as i32 + chunk[1] as i32) / 2) as i16;
                output.push(mono);
            }
        }
    } else if in_channels == 1 && out_channels == 2 {
        // Mono to stereo: duplicate
        for &sample in input {
            output.push(sample);
            output.push(sample);
        }
    }
    output
}

/// Apply volume (0-256, where 256 = 1.0)
pub fn apply_volume(samples: &mut [i16], volume: u16) {
    for s in samples.iter_mut() {
        *s = ((*s as i32 * volume as i32) >> 8) as i16;
    }
}

/// Simple low-pass filter
pub fn low_pass(samples: &mut [i16], alpha: u16) {
    // alpha: 0-256, lower = more filtering
    let mut prev = samples[0] as i32;
    for s in samples.iter_mut().skip(1) {
        let filtered = (alpha as i32 * *s as i32 + (256 - alpha as i32) * prev) >> 8;
        prev = filtered;
        *s = filtered as i16;
    }
}

pub fn init() {
    crate::serial_println!("  [codec] Audio codec pipeline initialized (PCM, ADPCM)");
}
