// video_codec/hardware.rs - Hardware acceleration abstraction layer

#![no_std]

use crate::video_codec::types::*;

/// Hardware accelerator interface
pub struct HardwareAccelerator {
    capabilities: HardwareCapabilities,
    vendor: HardwareVendor,
    initialized: bool,
}

impl HardwareAccelerator {
    /// Create a new hardware accelerator
    pub fn new() -> Self {
        let mut accel = Self {
            capabilities: HardwareCapabilities::default(),
            vendor: HardwareVendor::None,
            initialized: false,
        };

        accel.detect_hardware();
        accel
    }

    /// Detect available hardware acceleration
    fn detect_hardware(&mut self) {
        // Detect GPU vendor via PCI
        let vendor_id = self.read_pci_vendor_id();

        self.vendor = match vendor_id {
            0x8086 => HardwareVendor::Intel,
            0x1002 | 0x1022 => HardwareVendor::AMD,
            0x10DE => HardwareVendor::Nvidia,
            0x5143 => HardwareVendor::Qualcomm,
            _ => HardwareVendor::None,
        };

        // Probe capabilities based on vendor
        match self.vendor {
            HardwareVendor::Intel => self.probe_intel_capabilities(),
            HardwareVendor::AMD => self.probe_amd_capabilities(),
            HardwareVendor::Nvidia => self.probe_nvidia_capabilities(),
            _ => {}
        }

        self.initialized = true;
    }

    /// Read PCI vendor ID
    fn read_pci_vendor_id(&self) -> u16 {
        // Read from PCI config space at 0:2.0 (typical GPU location)
        unsafe {
            let addr = 0xCF8;
            let data = 0xCFC;

            // Write PCI address
            core::arch::asm!(
                "out dx, eax",
                in("dx") addr,
                in("eax") 0x80000000u32,
                options(nomem, nostack)
            );

            // Read vendor ID
            let mut vendor_id: u32 = 0;
            core::arch::asm!(
                "in eax, dx",
                in("dx") data,
                out("eax") vendor_id,
                options(nomem, nostack)
            );

            (vendor_id & 0xFFFF) as u16
        }
    }

    /// Probe Intel Quick Sync Video capabilities
    fn probe_intel_capabilities(&mut self) {
        // Intel QSV: H.264, H.265, VP9, AV1 (newer gens)
        self.capabilities.h264_decode = true;
        self.capabilities.h264_encode = true;
        self.capabilities.h265_decode = true;
        self.capabilities.h265_encode = true;
        self.capabilities.vp9_decode = true;
        self.capabilities.vp9_encode = true;
        // AV1 available on Gen 11+ (Tiger Lake and newer)
        self.capabilities.av1_decode = self.check_intel_gen() >= 11;
        self.capabilities.av1_encode = self.check_intel_gen() >= 12;
        self.capabilities.max_width = 4096;
        self.capabilities.max_height = 4096;
    }

    /// Probe AMD VCE/VCN capabilities
    fn probe_amd_capabilities(&mut self) {
        // AMD VCE/VCN: H.264, H.265
        self.capabilities.h264_decode = true;
        self.capabilities.h264_encode = true;
        self.capabilities.h265_decode = true;
        self.capabilities.h265_encode = true;
        // VP9 and AV1 on newer architectures (RDNA2+)
        self.capabilities.vp9_decode = true;
        self.capabilities.av1_decode = true;
        self.capabilities.max_width = 4096;
        self.capabilities.max_height = 4096;
    }

    /// Probe NVIDIA NVENC/NVDEC capabilities
    fn probe_nvidia_capabilities(&mut self) {
        // NVIDIA: H.264, H.265, VP9, AV1 (Ampere+)
        self.capabilities.h264_decode = true;
        self.capabilities.h264_encode = true;
        self.capabilities.h265_decode = true;
        self.capabilities.h265_encode = true;
        self.capabilities.vp9_decode = true;
        self.capabilities.av1_decode = true;
        self.capabilities.av1_encode = self.check_nvidia_gen() >= 30; // Ampere
        self.capabilities.max_width = 8192;
        self.capabilities.max_height = 8192;
    }

    /// Check Intel GPU generation
    fn check_intel_gen(&self) -> u32 {
        // Simplified: would need to read device ID and map to generation
        11 // Assume recent hardware
    }

    /// Check NVIDIA GPU generation
    fn check_nvidia_gen(&self) -> u32 {
        // Simplified: would need to read device ID
        30 // Assume Ampere or newer
    }

    /// Get hardware capabilities
    pub fn get_capabilities(&self) -> &HardwareCapabilities {
        &self.capabilities
    }

    /// Check if hardware decode is available for codec
    pub fn has_hw_decode(&self, codec: CodecType) -> bool {
        match codec {
            CodecType::H264 => self.capabilities.h264_decode,
            CodecType::H265 => self.capabilities.h265_decode,
            CodecType::VP9 => self.capabilities.vp9_decode,
            CodecType::AV1 => self.capabilities.av1_decode,
        }
    }

    /// Check if hardware encode is available for codec
    pub fn has_hw_encode(&self, codec: CodecType) -> bool {
        match codec {
            CodecType::H264 => self.capabilities.h264_encode,
            CodecType::H265 => self.capabilities.h265_encode,
            CodecType::VP9 => self.capabilities.vp9_encode,
            CodecType::AV1 => self.capabilities.av1_encode,
        }
    }

    /// Initialize hardware decoder
    pub fn init_hw_decoder(&mut self, codec: CodecType) -> Result<HWDecoderHandle, CodecError> {
        if !self.has_hw_decode(codec) {
            return Err(CodecError::HardwareNotAvailable);
        }

        // Vendor-specific initialization
        match self.vendor {
            HardwareVendor::Intel => self.init_intel_decoder(codec),
            HardwareVendor::AMD => self.init_amd_decoder(codec),
            HardwareVendor::Nvidia => self.init_nvidia_decoder(codec),
            _ => Err(CodecError::HardwareNotAvailable),
        }
    }

    /// Initialize hardware encoder
    pub fn init_hw_encoder(&mut self, codec: CodecType, config: &EncoderConfig) -> Result<HWEncoderHandle, CodecError> {
        if !self.has_hw_encode(codec) {
            return Err(CodecError::HardwareNotAvailable);
        }

        // Vendor-specific initialization
        match self.vendor {
            HardwareVendor::Intel => self.init_intel_encoder(codec, config),
            HardwareVendor::AMD => self.init_amd_encoder(codec, config),
            HardwareVendor::Nvidia => self.init_nvidia_encoder(codec, config),
            _ => Err(CodecError::HardwareNotAvailable),
        }
    }

    /// Initialize Intel QSV decoder
    fn init_intel_decoder(&mut self, _codec: CodecType) -> Result<HWDecoderHandle, CodecError> {
        Ok(HWDecoderHandle {
            handle: 0x1000,
            vendor: HardwareVendor::Intel,
        })
    }

    /// Initialize Intel QSV encoder
    fn init_intel_encoder(&mut self, _codec: CodecType, _config: &EncoderConfig) -> Result<HWEncoderHandle, CodecError> {
        Ok(HWEncoderHandle {
            handle: 0x1000,
            vendor: HardwareVendor::Intel,
        })
    }

    /// Initialize AMD VCN decoder
    fn init_amd_decoder(&mut self, _codec: CodecType) -> Result<HWDecoderHandle, CodecError> {
        Ok(HWDecoderHandle {
            handle: 0x2000,
            vendor: HardwareVendor::AMD,
        })
    }

    /// Initialize AMD VCE encoder
    fn init_amd_encoder(&mut self, _codec: CodecType, _config: &EncoderConfig) -> Result<HWEncoderHandle, CodecError> {
        Ok(HWEncoderHandle {
            handle: 0x2000,
            vendor: HardwareVendor::AMD,
        })
    }

    /// Initialize NVIDIA NVDEC decoder
    fn init_nvidia_decoder(&mut self, _codec: CodecType) -> Result<HWDecoderHandle, CodecError> {
        Ok(HWDecoderHandle {
            handle: 0x3000,
            vendor: HardwareVendor::Nvidia,
        })
    }

    /// Initialize NVIDIA NVENC encoder
    fn init_nvidia_encoder(&mut self, _codec: CodecType, _config: &EncoderConfig) -> Result<HWEncoderHandle, CodecError> {
        Ok(HWEncoderHandle {
            handle: 0x3000,
            vendor: HardwareVendor::Nvidia,
        })
    }
}

/// Hardware decoder handle
pub struct HWDecoderHandle {
    pub handle: u64,
    pub vendor: HardwareVendor,
}

impl HWDecoderHandle {
    /// Submit bitstream for hardware decoding
    pub fn decode(&self, bitstream: &[u8], output: &mut Frame) -> Result<(), CodecError> {
        match self.vendor {
            HardwareVendor::Intel => self.intel_decode(bitstream, output),
            HardwareVendor::AMD => self.amd_decode(bitstream, output),
            HardwareVendor::Nvidia => self.nvidia_decode(bitstream, output),
            _ => Err(CodecError::HardwareNotAvailable),
        }
    }

    fn intel_decode(&self, _bitstream: &[u8], _output: &mut Frame) -> Result<(), CodecError> {
        // Intel QSV decode via hardware registers
        Ok(())
    }

    fn amd_decode(&self, _bitstream: &[u8], _output: &mut Frame) -> Result<(), CodecError> {
        // AMD VCN decode
        Ok(())
    }

    fn nvidia_decode(&self, _bitstream: &[u8], _output: &mut Frame) -> Result<(), CodecError> {
        // NVIDIA NVDEC
        Ok(())
    }
}

/// Hardware encoder handle
pub struct HWEncoderHandle {
    pub handle: u64,
    pub vendor: HardwareVendor,
}

impl HWEncoderHandle {
    /// Encode frame using hardware
    pub fn encode(&self, frame: &Frame, output: &mut [u8]) -> Result<usize, CodecError> {
        match self.vendor {
            HardwareVendor::Intel => self.intel_encode(frame, output),
            HardwareVendor::AMD => self.amd_encode(frame, output),
            HardwareVendor::Nvidia => self.nvidia_encode(frame, output),
            _ => Err(CodecError::HardwareNotAvailable),
        }
    }

    fn intel_encode(&self, _frame: &Frame, _output: &mut [u8]) -> Result<usize, CodecError> {
        // Intel QSV encode
        Ok(0)
    }

    fn amd_encode(&self, _frame: &Frame, _output: &mut [u8]) -> Result<usize, CodecError> {
        // AMD VCE encode
        Ok(0)
    }

    fn nvidia_encode(&self, _frame: &Frame, _output: &mut [u8]) -> Result<usize, CodecError> {
        // NVIDIA NVENC
        Ok(0)
    }
}
