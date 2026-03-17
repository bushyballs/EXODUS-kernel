// video_codec/bench.rs - Performance benchmarking for video codecs

#![no_std]

use crate::video_codec::{
    CodecManager, CodecType, Frame, PixelFormat, EncoderConfig,
    Profile, Preset, RateControl, CodecError,
};

/// Benchmark results
pub struct BenchmarkResults {
    pub codec: CodecType,
    pub operation: Operation,
    pub resolution: (u32, u32),
    pub frames_processed: u32,
    pub total_cycles: u64,
    pub avg_cycles_per_frame: u64,
    pub fps: f32,
    pub hardware_used: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Operation {
    Decode,
    Encode,
}

/// Benchmarking suite
pub struct CodecBenchmark {
    codec_mgr: CodecManager,
}

impl CodecBenchmark {
    pub fn new() -> Self {
        Self {
            codec_mgr: CodecManager::new(),
        }
    }

    /// Benchmark H.264 decoding
    pub fn bench_h264_decode(&mut self, width: u32, height: u32, frame_count: u32) -> Result<BenchmarkResults, CodecError> {
        self.codec_mgr.init_decoder(CodecType::H264)?;

        let mut output_frame = Frame::new(width, height, PixelFormat::YUV420);
        let bitstream = [0u8; 64 * 1024];

        let start_cycles = Self::read_tsc();

        for _ in 0..frame_count {
            self.codec_mgr.decode(CodecType::H264, &bitstream, &mut output_frame)?;
        }

        let end_cycles = Self::read_tsc();
        let total_cycles = end_cycles - start_cycles;

        Ok(BenchmarkResults {
            codec: CodecType::H264,
            operation: Operation::Decode,
            resolution: (width, height),
            frames_processed: frame_count,
            total_cycles,
            avg_cycles_per_frame: total_cycles / frame_count as u64,
            fps: Self::calculate_fps(total_cycles, frame_count),
            hardware_used: self.codec_mgr.get_hw_capabilities().h264_decode,
        })
    }

    /// Benchmark H.265 decoding
    pub fn bench_h265_decode(&mut self, width: u32, height: u32, frame_count: u32) -> Result<BenchmarkResults, CodecError> {
        self.codec_mgr.init_decoder(CodecType::H265)?;

        let mut output_frame = Frame::new(width, height, PixelFormat::YUV420);
        let bitstream = [0u8; 64 * 1024];

        let start_cycles = Self::read_tsc();

        for _ in 0..frame_count {
            self.codec_mgr.decode(CodecType::H265, &bitstream, &mut output_frame)?;
        }

        let end_cycles = Self::read_tsc();
        let total_cycles = end_cycles - start_cycles;

        Ok(BenchmarkResults {
            codec: CodecType::H265,
            operation: Operation::Decode,
            resolution: (width, height),
            frames_processed: frame_count,
            total_cycles,
            avg_cycles_per_frame: total_cycles / frame_count as u64,
            fps: Self::calculate_fps(total_cycles, frame_count),
            hardware_used: self.codec_mgr.get_hw_capabilities().h265_decode,
        })
    }

    /// Benchmark VP9 decoding
    pub fn bench_vp9_decode(&mut self, width: u32, height: u32, frame_count: u32) -> Result<BenchmarkResults, CodecError> {
        self.codec_mgr.init_decoder(CodecType::VP9)?;

        let mut output_frame = Frame::new(width, height, PixelFormat::YUV420);
        let bitstream = [0u8; 64 * 1024];

        let start_cycles = Self::read_tsc();

        for _ in 0..frame_count {
            self.codec_mgr.decode(CodecType::VP9, &bitstream, &mut output_frame)?;
        }

        let end_cycles = Self::read_tsc();
        let total_cycles = end_cycles - start_cycles;

        Ok(BenchmarkResults {
            codec: CodecType::VP9,
            operation: Operation::Decode,
            resolution: (width, height),
            frames_processed: frame_count,
            total_cycles,
            avg_cycles_per_frame: total_cycles / frame_count as u64,
            fps: Self::calculate_fps(total_cycles, frame_count),
            hardware_used: self.codec_mgr.get_hw_capabilities().vp9_decode,
        })
    }

    /// Benchmark AV1 decoding
    pub fn bench_av1_decode(&mut self, width: u32, height: u32, frame_count: u32) -> Result<BenchmarkResults, CodecError> {
        self.codec_mgr.init_decoder(CodecType::AV1)?;

        let mut output_frame = Frame::new(width, height, PixelFormat::YUV420);
        let bitstream = [0u8; 64 * 1024];

        let start_cycles = Self::read_tsc();

        for _ in 0..frame_count {
            self.codec_mgr.decode(CodecType::AV1, &bitstream, &mut output_frame)?;
        }

        let end_cycles = Self::read_tsc();
        let total_cycles = end_cycles - start_cycles;

        Ok(BenchmarkResults {
            codec: CodecType::AV1,
            operation: Operation::Decode,
            resolution: (width, height),
            frames_processed: frame_count,
            total_cycles,
            avg_cycles_per_frame: total_cycles / frame_count as u64,
            fps: Self::calculate_fps(total_cycles, frame_count),
            hardware_used: self.codec_mgr.get_hw_capabilities().av1_decode,
        })
    }

    /// Benchmark H.264 encoding
    pub fn bench_h264_encode(&mut self, width: u32, height: u32, frame_count: u32) -> Result<BenchmarkResults, CodecError> {
        let config = EncoderConfig {
            width,
            height,
            framerate: 30,
            bitrate: 5_000_000,
            gop_size: 30,
            max_b_frames: 2,
            profile: Profile::High,
            preset: Preset::Medium,
            rate_control: RateControl::CBR,
            use_hardware: true,
        };

        self.codec_mgr.init_encoder(CodecType::H264, config)?;

        let input_frame = Frame::new(width, height, PixelFormat::YUV420);
        let mut output_buffer = [0u8; 256 * 1024];

        let start_cycles = Self::read_tsc();

        for _ in 0..frame_count {
            self.codec_mgr.encode(CodecType::H264, &input_frame, &mut output_buffer)?;
        }

        let end_cycles = Self::read_tsc();
        let total_cycles = end_cycles - start_cycles;

        Ok(BenchmarkResults {
            codec: CodecType::H264,
            operation: Operation::Encode,
            resolution: (width, height),
            frames_processed: frame_count,
            total_cycles,
            avg_cycles_per_frame: total_cycles / frame_count as u64,
            fps: Self::calculate_fps(total_cycles, frame_count),
            hardware_used: self.codec_mgr.get_hw_capabilities().h264_encode,
        })
    }

    /// Benchmark H.265 encoding
    pub fn bench_h265_encode(&mut self, width: u32, height: u32, frame_count: u32) -> Result<BenchmarkResults, CodecError> {
        let config = EncoderConfig {
            width,
            height,
            framerate: 30,
            bitrate: 5_000_000,
            gop_size: 30,
            max_b_frames: 2,
            profile: Profile::Main,
            preset: Preset::Medium,
            rate_control: RateControl::CBR,
            use_hardware: true,
        };

        self.codec_mgr.init_encoder(CodecType::H265, config)?;

        let input_frame = Frame::new(width, height, PixelFormat::YUV420);
        let mut output_buffer = [0u8; 256 * 1024];

        let start_cycles = Self::read_tsc();

        for _ in 0..frame_count {
            self.codec_mgr.encode(CodecType::H265, &input_frame, &mut output_buffer)?;
        }

        let end_cycles = Self::read_tsc();
        let total_cycles = end_cycles - start_cycles;

        Ok(BenchmarkResults {
            codec: CodecType::H265,
            operation: Operation::Encode,
            resolution: (width, height),
            frames_processed: frame_count,
            total_cycles,
            avg_cycles_per_frame: total_cycles / frame_count as u64,
            fps: Self::calculate_fps(total_cycles, frame_count),
            hardware_used: self.codec_mgr.get_hw_capabilities().h265_encode,
        })
    }

    /// Run comprehensive benchmark suite
    pub fn run_full_benchmark(&mut self) -> [BenchmarkResults; 12] {
        let resolutions = [
            (1280, 720),   // 720p
            (1920, 1080),  // 1080p
            (3840, 2160),  // 4K
        ];

        let frame_count = 100;
        let mut results = [BenchmarkResults::default(); 12];
        let mut idx = 0;

        for (width, height) in resolutions {
            // Decode benchmarks
            if let Ok(result) = self.bench_h264_decode(width, height, frame_count) {
                results[idx] = result;
                idx += 1;
            }

            if let Ok(result) = self.bench_h265_decode(width, height, frame_count) {
                results[idx] = result;
                idx += 1;
            }

            if let Ok(result) = self.bench_vp9_decode(width, height, frame_count) {
                results[idx] = result;
                idx += 1;
            }

            if let Ok(result) = self.bench_av1_decode(width, height, frame_count) {
                results[idx] = result;
                idx += 1;
            }
        }

        results
    }

    /// Read Time Stamp Counter (x86_64)
    fn read_tsc() -> u64 {
        unsafe {
            let mut low: u32;
            let mut high: u32;

            core::arch::asm!(
                "rdtsc",
                out("eax") low,
                out("edx") high,
                options(nomem, nostack)
            );

            ((high as u64) << 32) | (low as u64)
        }
    }

    /// Calculate FPS based on CPU cycles
    fn calculate_fps(cycles: u64, frame_count: u32) -> f32 {
        // Assume 3 GHz CPU
        const CPU_FREQ: u64 = 3_000_000_000;

        let seconds = cycles as f32 / CPU_FREQ as f32;
        frame_count as f32 / seconds
    }
}

impl BenchmarkResults {
    fn default() -> Self {
        Self {
            codec: CodecType::H264,
            operation: Operation::Decode,
            resolution: (0, 0),
            frames_processed: 0,
            total_cycles: 0,
            avg_cycles_per_frame: 0,
            fps: 0.0,
            hardware_used: false,
        }
    }

    /// Print benchmark results
    pub fn print(&self) {
        vga_print(b"\n=== Benchmark Results ===\n");

        // Codec
        vga_print(b"Codec: ");
        match self.codec {
            CodecType::H264 => vga_print(b"H.264"),
            CodecType::H265 => vga_print(b"H.265"),
            CodecType::VP9 => vga_print(b"VP9"),
            CodecType::AV1 => vga_print(b"AV1"),
        }
        vga_print(b"\n");

        // Operation
        vga_print(b"Operation: ");
        match self.operation {
            Operation::Decode => vga_print(b"Decode"),
            Operation::Encode => vga_print(b"Encode"),
        }
        vga_print(b"\n");

        // Resolution
        vga_print(b"Resolution: ");
        vga_print_num(self.resolution.0 as u64);
        vga_print(b"x");
        vga_print_num(self.resolution.1 as u64);
        vga_print(b"\n");

        // Hardware
        vga_print(b"Hardware: ");
        if self.hardware_used {
            vga_print(b"Yes");
        } else {
            vga_print(b"No");
        }
        vga_print(b"\n");

        // Performance
        vga_print(b"Frames: ");
        vga_print_num(self.frames_processed as u64);
        vga_print(b"\n");

        vga_print(b"Avg Cycles/Frame: ");
        vga_print_num(self.avg_cycles_per_frame);
        vga_print(b"\n");

        vga_print(b"FPS: ");
        vga_print_num(self.fps as u64);
        vga_print(b"\n");
    }
}

// VGA output helpers (would be imported from main kernel)
fn vga_print(s: &[u8]) {
    // Implementation in main kernel
}

fn vga_print_num(n: u64) {
    // Implementation in main kernel
}
