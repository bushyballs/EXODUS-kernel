// video_codec/examples.rs - Usage examples for the video codec framework

#![no_std]

use crate::video_codec::{
    CodecManager, CodecType, Frame, PixelFormat, EncoderConfig,
    Profile, Preset, RateControl, CodecError,
};

/// Example 1: Basic H.264 decoding
pub fn example_h264_decode() -> Result<(), CodecError> {
    // Initialize codec manager
    let mut codec_mgr = CodecManager::new();

    // Initialize H.264 decoder
    codec_mgr.init_decoder(CodecType::H264)?;

    // Prepare output frame
    let mut output_frame = Frame::new(1920, 1080, PixelFormat::YUV420);

    // Simulated bitstream data (in real use, this comes from file/network)
    let bitstream_data = [0u8; 4096];

    // Decode frame
    codec_mgr.decode(CodecType::H264, &bitstream_data, &mut output_frame)?;

    // Frame is now decoded in output_frame
    // Can be displayed via SurfaceFlinger or processed further

    Ok(())
}

/// Example 2: H.265 encoding with custom configuration
pub fn example_h265_encode() -> Result<(), CodecError> {
    let mut codec_mgr = CodecManager::new();

    // Configure encoder for high quality 4K video
    let config = EncoderConfig {
        width: 3840,
        height: 2160,
        framerate: 60,
        bitrate: 20_000_000,  // 20 Mbps
        gop_size: 60,         // 1 second GOP
        max_b_frames: 4,
        profile: Profile::Main,
        preset: Preset::Slow,  // Higher quality, slower encoding
        rate_control: RateControl::VBR,
        use_hardware: true,
    };

    // Initialize encoder
    codec_mgr.init_encoder(CodecType::H265, config)?;

    // Create input frame
    let input_frame = Frame::new(3840, 2160, PixelFormat::YUV420);
    // Fill frame with actual video data...

    // Encode frame
    let mut output_buffer = [0u8; 1024 * 1024];  // 1MB buffer
    let bytes_written = codec_mgr.encode(CodecType::H265, &input_frame, &mut output_buffer)?;

    // Encoded data is in output_buffer[0..bytes_written]
    // Can be written to file or streamed over network

    Ok(())
}

/// Example 3: VP9 decoding with hardware acceleration check
pub fn example_vp9_decode_with_hw_check() -> Result<(), CodecError> {
    let mut codec_mgr = CodecManager::new();

    // Check hardware capabilities
    let hw_caps = codec_mgr.get_hw_capabilities();

    if hw_caps.vp9_decode {
        println!("Using hardware VP9 decoder");
    } else {
        println!("Using software VP9 decoder");
    }

    // Initialize decoder
    codec_mgr.init_decoder(CodecType::VP9)?;

    // Decode frames
    let mut output_frame = Frame::new(1920, 1080, PixelFormat::YUV420);
    let bitstream_data = [0u8; 8192];

    codec_mgr.decode(CodecType::VP9, &bitstream_data, &mut output_frame)?;

    Ok(())
}

/// Example 4: AV1 encoding for modern streaming
pub fn example_av1_streaming() -> Result<(), CodecError> {
    let mut codec_mgr = CodecManager::new();

    // Check if hardware AV1 encoding is available
    let hw_caps = codec_mgr.get_hw_capabilities();

    let config = EncoderConfig {
        width: 1920,
        height: 1080,
        framerate: 30,
        bitrate: 3_000_000,   // 3 Mbps (AV1 is very efficient)
        gop_size: 30,
        max_b_frames: 7,      // AV1 supports more B-frames
        profile: Profile::Main,
        preset: if hw_caps.av1_encode {
            Preset::Fast        // Hardware can handle faster presets
        } else {
            Preset::Medium      // Software needs balanced preset
        },
        rate_control: RateControl::AVBR,
        use_hardware: hw_caps.av1_encode,
    };

    codec_mgr.init_encoder(CodecType::AV1, config)?;

    // Encode multiple frames
    for frame_num in 0..300 {  // 10 seconds @ 30fps
        let input_frame = Frame::new(1920, 1080, PixelFormat::YUV420);
        // Fill with actual frame data...

        let mut output_buffer = [0u8; 512 * 1024];
        let bytes_written = codec_mgr.encode(CodecType::AV1, &input_frame, &mut output_buffer)?;

        // Stream encoded data
        // send_to_network(&output_buffer[0..bytes_written]);
    }

    Ok(())
}

/// Example 5: Multi-codec transcoding
pub fn example_transcoding() -> Result<(), CodecError> {
    let mut codec_mgr = CodecManager::new();

    // Initialize H.264 decoder (input)
    codec_mgr.init_decoder(CodecType::H264)?;

    // Initialize H.265 encoder (output)
    let encoder_config = EncoderConfig {
        width: 1920,
        height: 1080,
        framerate: 30,
        bitrate: 4_000_000,
        gop_size: 30,
        max_b_frames: 2,
        profile: Profile::Main,
        preset: Preset::Medium,
        rate_control: RateControl::CBR,
        use_hardware: true,
    };
    codec_mgr.init_encoder(CodecType::H265, encoder_config)?;

    // Transcode loop
    let h264_bitstream = [0u8; 8192];
    let mut decoded_frame = Frame::new(1920, 1080, PixelFormat::YUV420);
    let mut h265_output = [0u8; 8192];

    // Decode H.264
    codec_mgr.decode(CodecType::H264, &h264_bitstream, &mut decoded_frame)?;

    // Encode as H.265
    let bytes_written = codec_mgr.encode(CodecType::H265, &decoded_frame, &mut h265_output)?;

    println!("Transcoded frame: {} bytes", bytes_written);

    Ok(())
}

/// Example 6: Low-latency real-time encoding
pub fn example_realtime_encoding() -> Result<(), CodecError> {
    let mut codec_mgr = CodecManager::new();

    // Configure for minimal latency
    let config = EncoderConfig {
        width: 1280,
        height: 720,
        framerate: 60,
        bitrate: 5_000_000,
        gop_size: 60,
        max_b_frames: 0,      // No B-frames for lowest latency
        profile: Profile::Baseline,  // Simpler profile
        preset: Preset::UltraFast,
        rate_control: RateControl::CBR,
        use_hardware: true,   // Hardware is essential for real-time
    };

    codec_mgr.init_encoder(CodecType::H264, config)?;

    // Real-time encoding loop
    loop {
        // Capture frame from camera/screen
        let input_frame = Frame::new(1280, 720, PixelFormat::YUV420);
        // capture_frame(&mut input_frame);

        let mut output_buffer = [0u8; 256 * 1024];
        let bytes_written = codec_mgr.encode(CodecType::H264, &input_frame, &mut output_buffer)?;

        // Stream immediately
        // send_to_network(&output_buffer[0..bytes_written]);

        // Break on some condition
        // if should_stop() { break; }
    }

    Ok(())
}

/// Example 7: Adaptive bitrate streaming (ABR)
pub fn example_adaptive_bitrate() -> Result<(), CodecError> {
    let mut codec_mgr = CodecManager::new();

    // Create multiple encoders for different quality levels
    let quality_levels = [
        (640, 360, 500_000),     // 360p @ 500 kbps
        (1280, 720, 2_000_000),  // 720p @ 2 Mbps
        (1920, 1080, 5_000_000), // 1080p @ 5 Mbps
        (3840, 2160, 15_000_000),// 4K @ 15 Mbps
    ];

    // Initialize encoder for highest quality
    let (width, height, bitrate) = quality_levels[3];
    let config = EncoderConfig {
        width,
        height,
        framerate: 30,
        bitrate,
        gop_size: 30,
        max_b_frames: 2,
        profile: Profile::High,
        preset: Preset::Fast,
        rate_control: RateControl::VBR,
        use_hardware: true,
    };

    codec_mgr.init_encoder(CodecType::H264, config)?;

    // Encode at selected quality based on network conditions
    let input_frame = Frame::new(width, height, PixelFormat::YUV420);
    let mut output_buffer = [0u8; 1024 * 1024];

    let bytes_written = codec_mgr.encode(CodecType::H264, &input_frame, &mut output_buffer)?;

    // Could dynamically switch quality levels based on:
    // - Available bandwidth
    // - Buffer occupancy
    // - Device capabilities

    Ok(())
}

/// Example 8: Decode and display pipeline
pub fn example_decode_display() -> Result<(), CodecError> {
    let mut codec_mgr = CodecManager::new();

    // Initialize decoder
    codec_mgr.init_decoder(CodecType::H265)?;

    // Create display frame
    let mut display_frame = Frame::new(1920, 1080, PixelFormat::YUV420);

    // Simulated video playback loop
    for frame_num in 0..900 {  // 30 seconds @ 30fps
        // Read encoded frame from file
        let bitstream_data = [0u8; 16384];
        // read_frame_from_file(&mut bitstream_data);

        // Decode
        codec_mgr.decode(CodecType::H265, &bitstream_data, &mut display_frame)?;

        // Display via SurfaceFlinger
        // surface_flinger.queue_buffer(&display_frame);

        // Timing control for playback
        // sleep_until_next_frame();
    }

    Ok(())
}

/// Example 9: Hardware capability detection and fallback
pub fn example_hw_detection() -> Result<(), CodecError> {
    let codec_mgr = CodecManager::new();
    let hw_caps = codec_mgr.get_hw_capabilities();

    println!("Video Codec Hardware Capabilities:");
    println!("===================================");
    println!("Vendor: {:?}", hw_caps.vendor);
    println!("H.264 Decode: {}", hw_caps.h264_decode);
    println!("H.264 Encode: {}", hw_caps.h264_encode);
    println!("H.265 Decode: {}", hw_caps.h265_decode);
    println!("H.265 Encode: {}", hw_caps.h265_encode);
    println!("VP9 Decode: {}", hw_caps.vp9_decode);
    println!("VP9 Encode: {}", hw_caps.vp9_encode);
    println!("AV1 Decode: {}", hw_caps.av1_decode);
    println!("AV1 Encode: {}", hw_caps.av1_encode);
    println!("Max Resolution: {}x{}", hw_caps.max_width, hw_caps.max_height);

    // Select best codec based on hardware
    let preferred_codec = if hw_caps.av1_encode {
        CodecType::AV1      // Best compression
    } else if hw_caps.h265_encode {
        CodecType::H265     // Good compression
    } else {
        CodecType::H264     // Widely compatible
    };

    println!("Recommended codec: {:?}", preferred_codec);

    Ok(())
}

/// Example 10: Buffer management for efficient decoding
pub fn example_buffer_management() -> Result<(), CodecError> {
    use crate::video_codec::buffer::BufferPool;

    let mut codec_mgr = CodecManager::new();
    let mut buffer_pool = BufferPool::new();

    codec_mgr.init_decoder(CodecType::VP9)?;

    // Pre-allocate buffers
    let buffer1 = buffer_pool.allocate(1920, 1080, PixelFormat::YUV420)?;
    let buffer2 = buffer_pool.allocate(1920, 1080, PixelFormat::YUV420)?;

    // Decode into buffers alternately
    let bitstream1 = [0u8; 8192];
    codec_mgr.decode(CodecType::VP9, &bitstream1, buffer1.frame_mut())?;

    let bitstream2 = [0u8; 8192];
    codec_mgr.decode(CodecType::VP9, &bitstream2, buffer2.frame_mut())?;

    // Buffers can be reused
    buffer_pool.release(0);
    buffer_pool.release(1);

    Ok(())
}

// Note: In no_std environment, would need custom print implementation
// using VGA buffer or serial output. These examples use conceptual println!
// which would be replaced with actual output mechanism.
