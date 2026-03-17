//! Audio codec subsystem usage examples
//!
//! Demonstrates encoding, decoding, streaming, DSP, and device I/O.

#![no_std]
#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use super::types::*;
use super::error::*;
use super::codecs::*;
use super::stream::*;
use super::device::*;
use super::dsp::*;
use super::format::*;

/// Example 1: Basic AAC encoding
pub fn example_aac_encode() -> Result<()> {
    // Initialize audio subsystem
    init()?;

    // Create AAC encoder
    let config = AudioConfig {
        sample_rate: 48000,
        channels: 2,
        bit_depth: 16,
        buffer_size: 4096,
    };

    let mut encoder = get_encoder(CodecId::AAC)?;
    encoder.init(&config)?;

    // Generate test audio (1kHz sine wave)
    let sample_count = 1024;
    let mut samples = Vec::new();
    for i in 0..sample_count {
        let t = i as f32 / config.sample_rate as f32;
        let value = (2.0 * core::f32::consts::PI * 1000.0 * t).sin();
        samples.push((value * 32767.0) as i16);
        samples.push((value * 32767.0) as i16); // Stereo
    }

    // Create audio frame
    let frame = AudioFrame {
        data: samples.as_ptr() as *const u8,
        len: samples.len() * 2,
        sample_rate: config.sample_rate,
        channels: config.channels,
        format: SampleFormat::S16LE,
        timestamp_us: 0,
    };

    // Encode
    let mut output = Vec::new();
    output.resize(8192, 0);
    let encoded_size = encoder.encode(&frame, &mut output)?;

    println!("Encoded {} bytes to {} AAC bytes", samples.len() * 2, encoded_size);

    Ok(())
}

/// Example 2: MP3 decoding
pub fn example_mp3_decode(mp3_data: &[u8]) -> Result<Vec<i16>> {
    let config = AudioConfig {
        sample_rate: 44100,
        channels: 2,
        bit_depth: 16,
        buffer_size: 4096,
    };

    let mut decoder = get_decoder(CodecId::MP3)?;
    decoder.init(&config)?;

    let packet = AudioPacket {
        data: mp3_data.as_ptr(),
        len: mp3_data.len(),
        codec: CodecId::MP3,
        timestamp_us: 0,
        duration_us: 0,
        is_keyframe: true,
    };

    let mut output = Vec::new();
    output.resize(8192, 0);
    let decoded_size = decoder.decode(&packet, &mut output)?;

    // Convert to i16 samples
    let sample_count = decoded_size / 2;
    let samples = unsafe {
        core::slice::from_raw_parts(output.as_ptr() as *const i16, sample_count)
    };

    Ok(samples.to_vec())
}

/// Example 3: Opus low-latency streaming
pub fn example_opus_streaming() -> Result<()> {
    // Create Opus encoder stream
    let mut encoder_stream = StreamBuilder::new(CodecId::Opus)
        .sample_rate(48000)
        .channels(2)
        .bit_depth(16)
        .buffer_size(960) // 20ms at 48kHz
        .build_encoder()?;

    encoder_stream.init()?;

    // Create Opus decoder stream
    let mut decoder_stream = StreamBuilder::new(CodecId::Opus)
        .sample_rate(48000)
        .channels(2)
        .build_decoder()?;

    decoder_stream.init()?;

    // Generate 20ms of audio
    let frame_size = 960 * 2; // stereo
    let mut samples = Vec::new();
    samples.resize(frame_size, 0i16);

    for i in 0..960 {
        let t = i as f32 / 48000.0;
        let value = (2.0 * core::f32::consts::PI * 440.0 * t).sin();
        samples[i * 2] = (value * 32767.0) as i16;
        samples[i * 2 + 1] = (value * 32767.0) as i16;
    }

    // Encode frame
    let frame = AudioFrame {
        data: samples.as_ptr() as *const u8,
        len: samples.len() * 2,
        sample_rate: 48000,
        channels: 2,
        format: SampleFormat::S16LE,
        timestamp_us: 0,
    };

    let mut encoded = Vec::new();
    encoded.resize(1500, 0);
    let enc_size = encoder_stream.encode(&frame, &mut encoded)?;

    println!("Opus encoded {} bytes to {} bytes", frame.len, enc_size);

    // Decode back
    let packet = AudioPacket {
        data: encoded.as_ptr(),
        len: enc_size,
        codec: CodecId::Opus,
        timestamp_us: 0,
        duration_us: 20000, // 20ms
        is_keyframe: false,
    };

    let mut decoded = Vec::new();
    decoded.resize(frame_size * 2, 0);
    let dec_size = decoder_stream.decode(&packet, &mut decoded)?;

    println!("Opus decoded {} bytes to {} bytes", enc_size, dec_size);

    Ok(())
}

/// Example 4: FLAC lossless encoding
pub fn example_flac_lossless() -> Result<()> {
    let config = AudioConfig {
        sample_rate: 96000,
        channels: 2,
        bit_depth: 24,
        buffer_size: 4096,
    };

    let mut encoder = get_encoder(CodecId::FLAC)?;
    encoder.init(&config)?;

    // Generate high-quality test audio
    let sample_count = 4096;
    let mut samples = Vec::new();
    for i in 0..sample_count {
        let t = i as f32 / config.sample_rate as f32;
        let value = (2.0 * core::f32::consts::PI * 1000.0 * t).sin();
        // 24-bit samples (stored as i32)
        let sample_24 = (value * 8388607.0) as i32;
        samples.push(sample_24 as i16); // For this example, truncate to 16-bit
        samples.push(sample_24 as i16);
    }

    let frame = AudioFrame {
        data: samples.as_ptr() as *const u8,
        len: samples.len() * 2,
        sample_rate: config.sample_rate,
        channels: config.channels,
        format: SampleFormat::S16LE,
        timestamp_us: 0,
    };

    let mut output = Vec::new();
    output.resize(16384, 0);
    let encoded_size = encoder.encode(&frame, &mut output)?;

    let original_size = samples.len() * 2;
    let compression_ratio = original_size as f32 / encoded_size as f32;

    println!("FLAC compression: {} -> {} bytes (ratio: {:.2}:1)",
             original_size, encoded_size, compression_ratio);

    Ok(())
}

/// Example 5: Sample rate conversion
pub fn example_sample_rate_conversion() -> Result<()> {
    // Generate 1 second of 44.1kHz audio
    let src_rate = 44100u32;
    let dst_rate = 48000u32;
    let channels = 2;

    let mut input_samples = Vec::new();
    for i in 0..(src_rate as usize * channels) {
        let t = (i / channels) as f32 / src_rate as f32;
        let value = (2.0 * core::f32::consts::PI * 1000.0 * t).sin();
        input_samples.push(value);
    }

    // Convert to 48kHz
    let mut converter = SampleRateConverter::new(src_rate, dst_rate);
    let mut output_samples = Vec::new();
    output_samples.resize((dst_rate as usize * channels), 0.0);

    let converted_frames = converter.convert(&input_samples, &mut output_samples, channels as u8)?;

    println!("Converted {} frames @ {}Hz to {} frames @ {}Hz",
             input_samples.len() / channels, src_rate,
             converted_frames, dst_rate);

    Ok(())
}

/// Example 6: Multi-stream audio mixing
pub fn example_audio_mixing() -> Result<()> {
    let channels = 2;
    let frame_size = 1024;

    // Create 3 audio streams
    let mut stream1 = Vec::new();
    let mut stream2 = Vec::new();
    let mut stream3 = Vec::new();

    for i in 0..frame_size * channels {
        let t = (i / channels) as f32 / 48000.0;

        // 440Hz sine wave
        stream1.push((2.0 * core::f32::consts::PI * 440.0 * t).sin() * 0.3);

        // 880Hz sine wave
        stream2.push((2.0 * core::f32::consts::PI * 880.0 * t).sin() * 0.3);

        // 1320Hz sine wave
        stream3.push((2.0 * core::f32::consts::PI * 1320.0 * t).sin() * 0.3);
    }

    // Mix streams
    let mut mixer = AudioMixer::new(channels as u8);
    let mut mixed_output = Vec::new();
    mixed_output.resize(frame_size * channels, 0.0);

    let streams = [stream1.as_slice(), stream2.as_slice(), stream3.as_slice()];
    mixer.mix(&streams, &mut mixed_output)?;

    println!("Mixed {} streams into {} samples", streams.len(), mixed_output.len());

    Ok(())
}

/// Example 7: Channel remapping (stereo to 5.1)
pub fn example_channel_mapping() -> Result<()> {
    let frame_size = 1024;

    // Generate stereo audio
    let mut stereo_samples = Vec::new();
    for i in 0..frame_size {
        let t = i as f32 / 48000.0;
        let value = (2.0 * core::f32::consts::PI * 1000.0 * t).sin();
        stereo_samples.push(value);      // Left
        stereo_samples.push(value * 0.8); // Right
    }

    // Map to 5.1 surround
    let mapper = ChannelMapper::new(ChannelLayout::Stereo, ChannelLayout::Surround5_1);
    let mut surround_samples = Vec::new();
    surround_samples.resize(frame_size * 6, 0.0);

    let mapped_frames = mapper.map(&stereo_samples, &mut surround_samples)?;

    println!("Mapped {} stereo frames to {} 5.1 frames",
             stereo_samples.len() / 2, mapped_frames);

    Ok(())
}

/// Example 8: Audio playback pipeline
pub fn example_playback_pipeline() -> Result<()> {
    // Open playback device
    let config = AudioConfig {
        sample_rate: 48000,
        channels: 2,
        bit_depth: 16,
        buffer_size: 4096,
    };

    let device_handle = open_device(0, DeviceDirection::Playback, &config)?;

    // Create decoder stream
    let mut stream = StreamBuilder::new(CodecId::AAC)
        .sample_rate(config.sample_rate)
        .channels(config.channels)
        .build_decoder()?;

    stream.init()?;

    // Start playback
    start_device(device_handle)?;

    // Playback loop (simplified)
    for _ in 0..10 {
        // Decode audio packet
        // let packet = ...; // Get from file/network
        // let mut decoded = Vec::new();
        // decoded.resize(4096, 0);
        // let size = stream.decode(&packet, &mut decoded)?;

        // Write to device
        // write_samples(device_handle, &decoded[..size])?;
    }

    // Stop and close
    stop_device(device_handle)?;
    close_device(device_handle)?;

    Ok(())
}

/// Example 9: WAV file encoding
pub fn example_wav_file() -> Result<()> {
    // Create audio data
    let sample_rate = 44100;
    let channels = 2;
    let duration_secs = 1;

    let mut samples = Vec::new();
    for i in 0..(sample_rate * channels * duration_secs) {
        let t = (i / channels) as f32 / sample_rate as f32;
        let value = (2.0 * core::f32::consts::PI * 440.0 * t).sin();
        samples.push((value * 32767.0) as i16);
    }

    // Create WAV file
    let info = FormatInfo {
        codec: CodecId::PCM,
        sample_rate,
        channels: channels as u8,
        bit_depth: 16,
        duration_us: (duration_secs * 1000000) as u64,
        bitrate: sample_rate * channels * 16,
    };

    let mut muxer = WavMuxer::new(info.clone());
    let mut output = Vec::new();
    output.resize(65536, 0);

    // Write header
    let header_size = muxer.write_header(&mut output, &info)?;

    // Write audio data
    let sample_bytes = unsafe {
        core::slice::from_raw_parts(samples.as_ptr() as *const u8, samples.len() * 2)
    };

    let packet = AudioPacket {
        data: sample_bytes.as_ptr(),
        len: sample_bytes.len(),
        codec: CodecId::PCM,
        timestamp_us: 0,
        duration_us: (duration_secs * 1000000) as u64,
        is_keyframe: true,
    };

    let data_size = muxer.write_packet(&mut output[header_size..], &packet)?;

    // Finalize (update header with sizes)
    muxer.finalize(&mut output)?;

    println!("Created WAV file: {} bytes (header: {}, data: {})",
             header_size + data_size, header_size, data_size);

    Ok(())
}

/// Example 10: Parametric EQ with biquad filters
pub fn example_parametric_eq() -> Result<()> {
    let sample_rate = 48000.0;
    let samples_count = 4800; // 100ms

    // Generate input signal
    let mut samples = Vec::new();
    for i in 0..samples_count {
        let t = i as f32 / sample_rate;
        // Mix of frequencies: 200Hz, 1kHz, 5kHz
        let signal = (2.0 * core::f32::consts::PI * 200.0 * t).sin() * 0.3
                   + (2.0 * core::f32::consts::PI * 1000.0 * t).sin() * 0.3
                   + (2.0 * core::f32::consts::PI * 5000.0 * t).sin() * 0.3;
        samples.push(signal);
    }

    // Create 3-band EQ: bass boost, mid cut, treble boost
    let mut bass_filter = BiquadFilter::new();
    bass_filter.set_lowpass(sample_rate, 300.0, 0.707);

    let mut treble_filter = BiquadFilter::new();
    treble_filter.set_highpass(sample_rate, 3000.0, 0.707);

    // Apply EQ
    let mut eq_output = Vec::new();
    for &sample in &samples {
        let bass = bass_filter.process(sample) * 1.5;  // Boost bass
        let treble = treble_filter.process(sample) * 1.3; // Boost treble
        eq_output.push(bass + treble);
    }

    println!("Applied parametric EQ to {} samples", eq_output.len());

    Ok(())
}

/// Example 11: LDAC high-quality Bluetooth encoding
pub fn example_ldac_bluetooth() -> Result<()> {
    let config = AudioConfig {
        sample_rate: 96000, // Hi-Res audio
        channels: 2,
        bit_depth: 24,
        buffer_size: 128,
    };

    let mut encoder = get_encoder(CodecId::LDAC)?;
    encoder.init(&config)?;

    // Generate high-resolution audio
    let sample_count = 128;
    let mut samples = Vec::new();
    for i in 0..sample_count {
        let t = i as f32 / config.sample_rate as f32;
        let value = (2.0 * core::f32::consts::PI * 1000.0 * t).sin();
        samples.push((value * 32767.0) as i16);
        samples.push((value * 32767.0) as i16);
    }

    let frame = AudioFrame {
        data: samples.as_ptr() as *const u8,
        len: samples.len() * 2,
        sample_rate: config.sample_rate,
        channels: config.channels,
        format: SampleFormat::S16LE,
        timestamp_us: 0,
    };

    let mut output = Vec::new();
    output.resize(2048, 0);
    let encoded_size = encoder.encode(&frame, &mut output)?;

    println!("LDAC encoded {}kHz/{}ch to {} bytes (990kbps mode)",
             config.sample_rate / 1000, config.channels, encoded_size);

    Ok(())
}

/// Run all examples
pub fn run_all_examples() -> Result<()> {
    println!("=== Genesis OS Audio Codec Examples ===\n");

    println!("Example 1: AAC Encoding");
    example_aac_encode()?;

    println!("\nExample 3: Opus Streaming");
    example_opus_streaming()?;

    println!("\nExample 4: FLAC Lossless");
    example_flac_lossless()?;

    println!("\nExample 5: Sample Rate Conversion");
    example_sample_rate_conversion()?;

    println!("\nExample 6: Audio Mixing");
    example_audio_mixing()?;

    println!("\nExample 7: Channel Mapping");
    example_channel_mapping()?;

    println!("\nExample 9: WAV File Creation");
    example_wav_file()?;

    println!("\nExample 10: Parametric EQ");
    example_parametric_eq()?;

    println!("\nExample 11: LDAC Bluetooth");
    example_ldac_bluetooth()?;

    println!("\n=== All examples completed ===");

    Ok(())
}
