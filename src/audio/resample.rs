/// Sample-rate conversion and PCM format helpers — integer / Q16 math only
///
/// All arithmetic uses Q16 fixed-point (1.0 = 65536).  No float casts
/// (`as f32` / `as f64`) are ever used; doing so would generate soft-float
/// calls that crash the kernel at opt-level 0.
///
/// Public functions:
///   resample_linear          — general interleaved SRC (any rate, any ch count)
///   resample_48k_to_44k      — fast-path: 48 000 → 44 100
///   resample_44k_to_48k      — fast-path: 44 100 → 48 000
///   resample_mono_to_stereo  — channel upmix (duplicate)
///   resample_stereo_to_mono  — channel downmix (average)
///   convert_s24le_to_s16     — S24 packed-in-32 → S16
///   convert_u8_to_s16        — U8 → S16
///   convert_s32le_to_s16     — S32 → S16

// ---------------------------------------------------------------------------
// Linear interpolation SRC
// ---------------------------------------------------------------------------

/// General-purpose linear-interpolation sample-rate converter.
///
/// `src`       — interleaved PCM at `src_rate` Hz
/// `src_rate`  — source sample rate (Hz)
/// `dst`       — interleaved output buffer at `dst_rate` Hz
/// `dst_rate`  — target sample rate (Hz)
/// `channels`  — number of interleaved channels (1 or 2 most common)
///
/// Returns the number of *frames* (not samples) written into `dst`.
///
/// Algorithm (Q16 fixed-point, no float):
/// ```text
/// step_q16 = (src_rate << 16) / dst_rate
/// pos_q16  = 0
/// for each output frame:
///   idx  = pos_q16 >> 16            (integer source frame index)
///   frac = pos_q16 & 0xFFFF         (fractional part, 0..65535)
///   for each channel ch:
///     a = src[idx * ch_count + ch]
///     b = src[(idx+1) * ch_count + ch]   (clamped to last frame)
///     out = a + ((b - a) * frac) >> 16   (linear interpolation)
///   pos_q16 += step_q16
/// ```
pub fn resample_linear(
    src: &[i16],
    src_rate: u32,
    dst: &mut [i16],
    dst_rate: u32,
    channels: u8,
) -> usize {
    let ch = channels.max(1) as usize;

    // Trivial identity pass.
    if src_rate == dst_rate {
        let n = src.len().min(dst.len());
        dst[..n].copy_from_slice(&src[..n]);
        return n / ch;
    }

    if src.is_empty() || dst.is_empty() || src_rate == 0 || dst_rate == 0 {
        return 0;
    }

    let src_frames = src.len() / ch;
    if src_frames == 0 {
        return 0;
    }

    // Q16 step: how far to advance in the source per output frame.
    // Use u64 to avoid overflow when src_rate is large.
    let step_q16: u64 = ((src_rate as u64) << 16) / dst_rate as u64;

    let dst_frames = dst.len() / ch;
    let max_src_frame_q16: u64 = ((src_frames as u64).saturating_sub(1)) << 16;

    let mut pos_q16: u64 = 0;
    let mut out_frame = 0usize;

    while out_frame < dst_frames && pos_q16 <= max_src_frame_q16 {
        let idx = (pos_q16 >> 16) as usize;
        let frac = (pos_q16 & 0xFFFF) as i32; // 0..65535

        for c in 0..ch {
            let a = src[idx * ch + c] as i32;
            // Guard: do not read past the end of the source.
            let b = if idx + 1 < src_frames {
                src[(idx + 1) * ch + c] as i32
            } else {
                a
            };
            // Linear interpolation: a + (b - a) * frac / 65536
            let interp = a + (((b - a) * frac) >> 16);
            dst[out_frame * ch + c] = interp.clamp(-32768, 32767) as i16;
        }

        out_frame = out_frame.wrapping_add(1);
        pos_q16 = pos_q16.wrapping_add(step_q16);
    }

    out_frame
}

// ---------------------------------------------------------------------------
// Common-rate fast-path helpers
// ---------------------------------------------------------------------------

/// Convert 48 000 Hz → 44 100 Hz (any channel count).
/// Returns frames written.
pub fn resample_48k_to_44k(src: &[i16], dst: &mut [i16], channels: u8) -> usize {
    resample_linear(src, 48_000, dst, 44_100, channels)
}

/// Convert 44 100 Hz → 48 000 Hz (any channel count).
/// Returns frames written.
pub fn resample_44k_to_48k(src: &[i16], dst: &mut [i16], channels: u8) -> usize {
    resample_linear(src, 44_100, dst, 48_000, channels)
}

// ---------------------------------------------------------------------------
// Channel conversion helpers
// ---------------------------------------------------------------------------

/// Mono → stereo: duplicate each source sample into both output channels.
/// Returns the number of stereo *frames* written.
pub fn resample_mono_to_stereo(src: &[i16], dst: &mut [i16]) -> usize {
    let frames = src.len().min(dst.len() / 2);
    for i in 0..frames {
        dst[i * 2] = src[i];
        dst[i * 2 + 1] = src[i];
    }
    frames
}

/// Stereo → mono: average the two channels.
/// `out[i] = ((src[i*2] as i32 + src[i*2+1] as i32) / 2) as i16`
/// Returns the number of mono *frames* written.
pub fn resample_stereo_to_mono(src: &[i16], dst: &mut [i16]) -> usize {
    let frames = (src.len() / 2).min(dst.len());
    for i in 0..frames {
        let l = src[i * 2] as i32;
        let r = src[i * 2 + 1] as i32;
        dst[i] = ((l + r) / 2) as i16;
    }
    frames
}

// ---------------------------------------------------------------------------
// Format conversion helpers (no float operations)
// ---------------------------------------------------------------------------

/// Convert S24-LE packed in 4 bytes (little-endian, MSB sign-extended) to S16.
///
/// Each source sample occupies 4 bytes (S24 padded to 32 bits).
/// `dst[i] = (read_i32_le(src, i) >> 8) as i16` — keeps the upper 16 bits.
///
/// Returns the number of samples converted.
pub fn convert_s24le_to_s16(src: &[u8], dst: &mut [i16]) -> usize {
    // Each sample is 4 bytes (S24 stored in a 32-bit word).
    let sample_count = (src.len() / 4).min(dst.len());
    for i in 0..sample_count {
        let b0 = src[i * 4] as i32;
        let b1 = src[i * 4 + 1] as i32;
        let b2 = src[i * 4 + 2] as i32;
        let b3 = src[i * 4 + 3] as i32; // sign byte (MSB of S24 packed to 32)
                                        // Reconstruct 32-bit signed value (little-endian).
        let v = b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
        // Right-shift by 8 to obtain the top 16 bits.
        dst[i] = (v >> 8) as i16;
    }
    sample_count
}

/// Convert U8 (unsigned 8-bit, DC offset at 128) to S16.
///
/// `dst[i] = (src[i] as i16 - 128) * 256`
///
/// Returns the number of samples converted.
pub fn convert_u8_to_s16(src: &[u8], dst: &mut [i16]) -> usize {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] = ((src[i] as i16).wrapping_sub(128)).wrapping_mul(256);
    }
    n
}

/// Convert S32-LE to S16 by discarding the lower 16 bits.
///
/// `dst[i] = (src[i] >> 16) as i16`
///
/// Returns the number of samples converted.
pub fn convert_s32le_to_s16(src: &[i32], dst: &mut [i16]) -> usize {
    let n = src.len().min(dst.len());
    for i in 0..n {
        dst[i] = (src[i] >> 16) as i16;
    }
    n
}
