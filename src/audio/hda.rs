use crate::sync::Mutex;
/// Intel HD Audio (HDA) controller driver
///
/// Communicates with the HDA codec via CORB/RIRB (Command/Response Ring Buffers).
/// Discovers audio widgets (DACs, ADCs, mixers, pins) through the codec tree.
///
/// Output pipeline:
///   sw_mix_frame() -> BDL DMA buffer -> HDA output stream descriptor -> speaker
///
/// Call hda_set_output_stream() once after init() to configure the stream
/// format, then hda_start_playback() to begin DMA.  The timer tick (or any
/// periodic callback) should call hda_tick() to refill the BDL buffer from
/// the software mixer.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// BAR0 MMIO base address for the discovered HDA controller.
/// Written once during init(); read by callers that need register access.
static HDA_MMIO_BASE: AtomicU64 = AtomicU64::new(0);

static HDA_STATE: Mutex<Option<HdaController>> = Mutex::new(None);

// =============================================================================
// HDA register offsets (from BAR0)
// =============================================================================

const GCAP: u32 = 0x00; // Global Capabilities
const GCTL: u32 = 0x08; // Global Control
const STATESTS: u32 = 0x0E; // State Change Status
#[allow(dead_code)]
const CORBLBASE: u32 = 0x40; // CORB Lower Base
#[allow(dead_code)]
const CORBUBASE: u32 = 0x44; // CORB Upper Base
#[allow(dead_code)]
const CORBWP: u32 = 0x48; // CORB Write Pointer
#[allow(dead_code)]
const CORBRP: u32 = 0x4A; // CORB Read Pointer
#[allow(dead_code)]
const CORBCTL: u32 = 0x4C; // CORB Control
#[allow(dead_code)]
const RIRBLBASE: u32 = 0x50; // RIRB Lower Base
#[allow(dead_code)]
const RIRBUBASE: u32 = 0x54; // RIRB Upper Base
#[allow(dead_code)]
const RIRBWP: u32 = 0x58; // RIRB Write Pointer
#[allow(dead_code)]
const RINTCNT: u32 = 0x5A; // Response Interrupt Count
#[allow(dead_code)]
const RIRBCTL: u32 = 0x5C; // RIRB Control
#[allow(dead_code)]
const INTCTL: u32 = 0x20; // Interrupt Control
#[allow(dead_code)]
const INTSTS: u32 = 0x24; // Interrupt Status

// Output stream descriptor register base (stream 0 = first output stream).
// HDA spec: SD0 starts at 0x80 for controllers without input streams,
// but generally output stream n starts at 0x80 + n*0x20 (after input streams).
// We assume the first output stream is SD0 at 0x80 for simplicity;
// a full driver would compute the offset from GCAP.ISS.
#[allow(dead_code)]
const SD0_BASE: u32 = 0x80; // first output stream descriptor base

// Stream Descriptor register offsets (relative to stream descriptor base)
const SDnCTL: u32 = 0x00; // Stream Descriptor Control (24-bit)
const SDnSTS: u32 = 0x03; // Stream Descriptor Status (8-bit)
const SDnLPIB: u32 = 0x04; // Link Position in Buffer
const SDnCBL: u32 = 0x08; // Cyclic Buffer Length
const SDnLVI: u32 = 0x0C; // Last Valid Index (16-bit)
#[allow(dead_code)]
const SDnFIFOS: u32 = 0x10; // FIFO Size (16-bit)
const SDnFMT: u32 = 0x12; // Stream Format (16-bit)
const SDnBDPL: u32 = 0x18; // BDL Physical Address Lower
const SDnBDPU: u32 = 0x1C; // BDL Physical Address Upper

// SDnCTL bits
const SDCTL_SRST: u32 = 1 << 0; // Stream reset
const SDCTL_RUN: u32 = 1 << 1; // Stream run
const SDCTL_IOCE: u32 = 1 << 2; // Interrupt on completion enable
#[allow(dead_code)]
const SDCTL_FEIE: u32 = 1 << 3; // FIFO error interrupt enable
#[allow(dead_code)]
const SDCTL_DEIE: u32 = 1 << 4; // Descriptor error interrupt enable
/// Stream number field starts at bit 20 in SDnCTL.
const SDCTL_STRM_SHIFT: u32 = 20;

// SDnSTS bits
const SDSTS_BCIS: u8 = 1 << 2; // Buffer Completion Interrupt Status (IOC fired)
#[allow(dead_code)]
const SDSTS_FIFOE: u8 = 1 << 3; // FIFO Error
#[allow(dead_code)]
const SDSTS_DESE: u8 = 1 << 4; // Descriptor Error

// =============================================================================
// HDA verb commands (reserved for future CORB/RIRB command dispatch)
// =============================================================================

#[allow(dead_code)]
const GET_PARAM: u32 = 0xF0000;
#[allow(dead_code)]
const SET_STREAM: u32 = 0x70600;
#[allow(dead_code)]
const SET_FORMAT: u32 = 0x20000;
#[allow(dead_code)]
const SET_PIN_WIDGET: u32 = 0x70700;

// =============================================================================
// Codec parameters (reserved for future codec enumeration)
// =============================================================================

#[allow(dead_code)]
const PARAM_VENDOR: u32 = 0x00;
#[allow(dead_code)]
const PARAM_REVISION: u32 = 0x02;
#[allow(dead_code)]
const PARAM_NODE_COUNT: u32 = 0x04;
#[allow(dead_code)]
const PARAM_AUDIO_WIDGET_CAP: u32 = 0x09;

// =============================================================================
// HDA stream format word encoding
// =============================================================================
//
// Bits [15:14]: BASE (0 = 48 kHz base, 1 = 44.1 kHz base)
// Bits [13:11]: MULT (multiplier: 0=x1, 1=x2, 2=x3, 3=x4)
// Bits [10:8]:  DIV  (divider:    0=/1, 1=/2, 2=/3, ... 7=/8)
// Bits [6:4]:   BITS (0=8,1=16,2=20,3=24,4=32)
// Bits [3:0]:   CHAN (channels - 1)

fn encode_stream_format(sr: u32, channels: u8, bits: u8) -> u16 {
    // Determine base clock and multiplier/divider for the target sample rate.
    // We support 8, 11.025, 16, 22.05, 24, 32, 44.1, 48, 96, 192 kHz.
    // All others fall back to 48 kHz stereo 16-bit.
    let (base44, mult, div) = match sr {
        8000 => (0u16, 0u16, 5u16), // 48k / 6
        11025 => (1, 0, 3),         // 44.1k / 4
        16000 => (0, 0, 2),         // 48k / 3
        22050 => (1, 0, 1),         // 44.1k / 2
        24000 => (0, 0, 1),         // 48k / 2
        32000 => (0, 0, 0),         // 48k  (closest; actual 48k) -- approximate
        44100 => (1, 0, 0),         // 44.1k x1
        48000 => (0, 0, 0),         // 48k x1
        88200 => (1, 1, 0),         // 44.1k x2
        96000 => (0, 1, 0),         // 48k x2
        176400 => (1, 3, 0),        // 44.1k x4
        192000 => (0, 3, 0),        // 48k x4
        _ => (0, 0, 0),             // default: 48k x1
    };

    let bits_field: u16 = match bits {
        8 => 0,
        16 => 1,
        20 => 2,
        24 => 3,
        32 => 4,
        _ => 1, // default 16-bit
    };

    let chan_field = (channels.saturating_sub(1) as u16) & 0xF;

    (base44 << 14) | (mult << 11) | (div << 8) | (bits_field << 4) | chan_field
}

// =============================================================================
// BDL (Buffer Descriptor List)
// =============================================================================

/// Number of BDL entries.  Each entry points to one DMA segment.
/// We use 2 entries (ping-pong double buffer): while one is playing,
/// the mixer fills the other.
const BDL_ENTRIES: usize = 2;

/// Bytes per BDL segment.  Must be a multiple of 128 (HDA requirement).
/// 4096 bytes = 1024 stereo i16 frames = ~21 ms at 48 kHz.  Each entry
/// holds BUFFER_SAMPLES (2048) i16 samples = 4096 bytes.
const BDL_SEGMENT_BYTES: usize = super::mixer::BUFFER_FRAMES * 2 * core::mem::size_of::<i16>();
const BDL_TOTAL_BYTES: usize = BDL_SEGMENT_BYTES * BDL_ENTRIES;

/// One BDL entry (8 bytes physical address + 4 bytes length + 4 bytes flags).
#[repr(C)]
struct BdlEntry {
    /// Physical address of the DMA buffer (low 32 bits).
    addr_lo: u32,
    /// Physical address of the DMA buffer (high 32 bits).
    addr_hi: u32,
    /// Length of the buffer segment in bytes.
    length: u32,
    /// Flags: bit 0 = IOC (interrupt on completion).
    flags: u32,
}

/// Static BDL array.  Must be 128-byte aligned per HDA spec.
/// We over-align to 4096 (page) for simplicity.
#[repr(C, align(128))]
struct BdlTable {
    entries: [BdlEntry; BDL_ENTRIES],
}

/// Static DMA buffer (ping-pong).  Page-aligned.
#[repr(C, align(4096))]
struct DmaBuffer {
    data: [u8; BDL_TOTAL_BYTES],
}

static mut BDL: BdlTable = BdlTable {
    entries: [
        BdlEntry {
            addr_lo: 0,
            addr_hi: 0,
            length: 0,
            flags: 0,
        },
        BdlEntry {
            addr_lo: 0,
            addr_hi: 0,
            length: 0,
            flags: 0,
        },
    ],
};

static mut DMA_BUF: DmaBuffer = DmaBuffer {
    data: [0u8; BDL_TOTAL_BYTES],
};

/// Index of the BDL segment currently being written by the mixer (0 or 1).
/// The other segment is being consumed by the HDA DMA engine.
static WRITE_SEGMENT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// True when the output stream is running.
static PLAYBACK_RUNNING: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Audio widget types
// =============================================================================

/// Audio widget types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetType {
    AudioOutput, // DAC
    AudioInput,  // ADC
    AudioMixer,
    AudioSelector,
    PinComplex,
    Power,
    VolumeKnob,
    BeepGenerator,
    Unknown(u8),
}

impl WidgetType {
    fn from_caps(caps: u32) -> Self {
        match (caps >> 20) & 0xF {
            0 => WidgetType::AudioOutput,
            1 => WidgetType::AudioInput,
            2 => WidgetType::AudioMixer,
            3 => WidgetType::AudioSelector,
            4 => WidgetType::PinComplex,
            5 => WidgetType::Power,
            6 => WidgetType::VolumeKnob,
            7 => WidgetType::BeepGenerator,
            n => WidgetType::Unknown(n as u8),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioWidget {
    pub nid: u8,
    pub widget_type: WidgetType,
    pub caps: u32,
}

// =============================================================================
// HDA controller state
// =============================================================================

pub struct HdaController {
    pub bar0: u64,
    pub codecs: Vec<u8>,
    pub widgets: Vec<AudioWidget>,
    pub sample_rate: u32,
    pub channels: u8,
    pub bit_depth: u8,
}

impl HdaController {
    pub fn new(bar0: u64) -> Self {
        HdaController {
            bar0,
            codecs: Vec::new(),
            widgets: Vec::new(),
            sample_rate: 48000,
            channels: 2,
            bit_depth: 16,
        }
    }

    // -------------------------------------------------------------------------
    // Register I/O helpers
    // -------------------------------------------------------------------------

    /// Read a 32-bit HDA register at `offset` bytes from BAR0.
    fn read32(&self, offset: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.bar0 + offset as u64) as *const u32) }
    }

    /// Write a 32-bit HDA register.
    fn write32(&self, offset: u32, val: u32) {
        unsafe {
            core::ptr::write_volatile((self.bar0 + offset as u64) as *mut u32, val);
        }
    }

    /// Read a 16-bit HDA register.
    fn read16(&self, offset: u32) -> u16 {
        unsafe { core::ptr::read_volatile((self.bar0 + offset as u64) as *const u16) }
    }

    /// Write a 16-bit HDA register.
    fn write16(&self, offset: u32, val: u16) {
        unsafe {
            core::ptr::write_volatile((self.bar0 + offset as u64) as *mut u16, val);
        }
    }

    /// Read an 8-bit HDA register.
    fn read8(&self, offset: u32) -> u8 {
        unsafe { core::ptr::read_volatile((self.bar0 + offset as u64) as *const u8) }
    }

    /// Write an 8-bit HDA register.
    fn write8(&self, offset: u32, val: u8) {
        unsafe {
            core::ptr::write_volatile((self.bar0 + offset as u64) as *mut u8, val);
        }
    }

    // -------------------------------------------------------------------------
    // Controller lifecycle
    // -------------------------------------------------------------------------

    /// Reset the HDA controller.
    pub fn reset(&self) {
        // Clear CRST bit to enter reset
        self.write32(GCTL, self.read32(GCTL) & !0x01);
        // Wait until controller acknowledges reset (CRST reads back 0)
        for _ in 0..100_000 {
            if self.read32(GCTL) & 0x01 == 0 {
                break;
            }
        }
        // Set CRST bit to exit reset
        self.write32(GCTL, self.read32(GCTL) | 0x01);
        // Wait for controller ready (CRST reads back 1)
        for _ in 0..100_000 {
            if self.read32(GCTL) & 0x01 != 0 {
                break;
            }
        }
    }

    /// Enumerate codecs present on the HDA link.
    pub fn enumerate_codecs(&mut self) {
        let statests = self.read32(STATESTS as u32) & 0x7FFF;
        for i in 0..15u8 {
            if statests & (1 << i) != 0 {
                self.codecs.push(i);
                serial_println!("    [hda] Found codec at address {}", i);
            }
        }
    }

    /// Return stream capability counts from GCAP: (output, input, bidir).
    pub fn capabilities(&self) -> (u8, u8, u8) {
        let gcap = self.read32(GCAP);
        let oss = ((gcap >> 12) & 0xF) as u8; // output streams supported
        let iss = ((gcap >> 8) & 0xF) as u8; // input streams supported
        let bss = ((gcap >> 3) & 0x1F) as u8; // bidirectional streams
        (oss, iss, bss)
    }

    // -------------------------------------------------------------------------
    // Output stream descriptor management
    // -------------------------------------------------------------------------

    /// Return the register base offset for output stream `n` (0-based).
    ///
    /// Per HDA spec the stream descriptor array starts at 0x80.  Input stream
    /// descriptors come first (ISS of them), then output streams.  We read ISS
    /// from GCAP to compute the correct base for output stream 0.
    fn output_sd_base(&self, n: u32) -> u32 {
        let gcap = self.read32(GCAP);
        let iss = (gcap >> 8) & 0xF; // number of input stream descriptors
        0x80_u32.saturating_add(iss.saturating_add(n).saturating_mul(0x20))
    }

    /// Reset the output stream descriptor `n` (1-shot SRST cycle).
    fn reset_output_stream(&self, n: u32) {
        let base = self.output_sd_base(n);
        // Set SRST (stream reset)
        let ctl = self.read32(base + SDnCTL);
        self.write32(base + SDnCTL, ctl | SDCTL_SRST);
        // Wait until hardware acknowledges reset
        for _ in 0..10_000 {
            if self.read32(base + SDnCTL) & SDCTL_SRST != 0 {
                break;
            }
        }
        // Clear SRST to take stream out of reset
        let ctl = self.read32(base + SDnCTL);
        self.write32(base + SDnCTL, ctl & !SDCTL_SRST);
        // Wait until SRST clears
        for _ in 0..10_000 {
            if self.read32(base + SDnCTL) & SDCTL_SRST == 0 {
                break;
            }
        }
    }

    /// Configure output stream `n` for `sr` Hz, `channels` channels, `bits`
    /// bits per sample.  Programs the stream format word, BDL base address,
    /// cyclic buffer length, and last valid index.  Does NOT start the stream.
    pub fn configure_output_stream(&self, n: u32, sr: u32, channels: u8, bits: u8) {
        let base = self.output_sd_base(n);

        // Step 1: reset the stream descriptor.
        self.reset_output_stream(n);

        // Step 2: initialise the BDL entries using the static DMA buffer.
        // Safety: we have exclusive access at this point (called from init or
        // a locked context before playback starts).
        let bdl_phys = &raw const BDL as *const BdlTable as u64;
        let dma_phys = &raw const DMA_BUF as *const DmaBuffer as u64;

        unsafe {
            for i in 0..BDL_ENTRIES {
                let seg_phys = dma_phys.saturating_add((i * BDL_SEGMENT_BYTES) as u64);
                BDL.entries[i].addr_lo = seg_phys as u32;
                BDL.entries[i].addr_hi = (seg_phys >> 32) as u32;
                BDL.entries[i].length = BDL_SEGMENT_BYTES as u32;
                // Set IOC on every entry so we get a completion interrupt
                // each time the DMA engine finishes a segment.
                BDL.entries[i].flags = 1; // IOC=1
            }
        }

        // Step 3: program the stream format.
        let fmt = encode_stream_format(sr, channels, bits);
        self.write16(base + SDnFMT, fmt);

        // Step 4: program the cyclic buffer length and last valid index.
        self.write32(base + SDnCBL, BDL_TOTAL_BYTES as u32);
        self.write16(base + SDnLVI, (BDL_ENTRIES as u16).saturating_sub(1));

        // Step 5: write the BDL base address into the descriptor registers.
        self.write32(base + SDnBDPL, bdl_phys as u32);
        self.write32(base + SDnBDPU, (bdl_phys >> 32) as u32);

        // Step 6: set stream number in SDnCTL (stream tag = n+1; 0 is invalid).
        let stream_tag = (n.saturating_add(1) & 0xF) << SDCTL_STRM_SHIFT;
        let ctl = self.read32(base + SDnCTL);
        // Clear old stream number field, apply new one + enable IOC interrupt
        let ctl = (ctl & !(0xF << SDCTL_STRM_SHIFT)) | stream_tag | SDCTL_IOCE;
        self.write32(base + SDnCTL, ctl);

        serial_println!(
            "    [hda] Output stream {}: {}Hz, {}ch, {}bit, fmt={:#06x}, BDL phys={:#x}",
            n,
            sr,
            channels,
            bits,
            fmt,
            bdl_phys
        );
    }

    /// Start the output stream `n`.
    pub fn start_stream(&self, n: u32) {
        let base = self.output_sd_base(n);
        let ctl = self.read32(base + SDnCTL);
        self.write32(base + SDnCTL, ctl | SDCTL_RUN);
        PLAYBACK_RUNNING.store(true, Ordering::Relaxed);
        serial_println!("    [hda] Stream {} started", n);
    }

    /// Stop the output stream `n`.
    pub fn stop_stream(&self, n: u32) {
        let base = self.output_sd_base(n);
        let ctl = self.read32(base + SDnCTL);
        self.write32(base + SDnCTL, ctl & !SDCTL_RUN);
        PLAYBACK_RUNNING.store(false, Ordering::Relaxed);
        serial_println!("    [hda] Stream {} stopped", n);
    }

    /// Return the current link position in buffer (bytes consumed so far in
    /// the current cycle) for output stream `n`.
    pub fn stream_lpib(&self, n: u32) -> u32 {
        let base = self.output_sd_base(n);
        self.read32(base + SDnLPIB)
    }

    /// Clear the IOC (buffer completion interrupt) status bit for stream `n`.
    pub fn clear_ioc(&self, n: u32) {
        let base = self.output_sd_base(n);
        // Write 1 to clear BCIS (bit 2) in SDnSTS (byte at offset 0x03 into CTL)
        self.write8(base + SDnSTS, SDSTS_BCIS);
    }
}

// =============================================================================
// Public output-stream API
// =============================================================================

/// Configure output stream 0 for the given format.
///
/// Must be called after `init()` and before `hda_start_playback()`.
/// Resets the stream descriptor and re-programs the BDL.
pub fn hda_set_output_stream(sr: u32, channels: u8, bits: u8) {
    let mut state = HDA_STATE.lock();
    if let Some(ref mut ctrl) = *state {
        ctrl.sample_rate = sr;
        ctrl.channels = channels;
        ctrl.bit_depth = bits;
        ctrl.configure_output_stream(0, sr, channels, bits);
    } else {
        serial_println!("    [hda] hda_set_output_stream: no HDA controller");
    }
}

/// Start DMA playback on output stream 0.
pub fn hda_start_playback() {
    // Pre-fill both BDL segments from the software mixer before starting DMA.
    // This prevents an underrun on the very first interrupt.
    _refill_segment(0);
    _refill_segment(1);
    WRITE_SEGMENT.store(0, Ordering::Relaxed);

    let state = HDA_STATE.lock();
    if let Some(ref ctrl) = *state {
        ctrl.start_stream(0);
    } else {
        serial_println!("    [hda] hda_start_playback: no HDA controller");
    }
}

/// Stop DMA playback on output stream 0.
pub fn hda_stop_playback() {
    let state = HDA_STATE.lock();
    if let Some(ref ctrl) = *state {
        ctrl.stop_stream(0);
    }
}

/// Write `samples` directly into the BDL DMA buffer at the current write
/// segment, advancing the write pointer.  Returns the number of i16 samples
/// actually written.  Intended for callers that manage their own PCM data
/// rather than going through the software mixer.
pub fn hda_write_samples(samples: &[i16]) -> usize {
    if !PLAYBACK_RUNNING.load(Ordering::Relaxed) {
        return 0;
    }

    let seg = WRITE_SEGMENT.load(Ordering::Relaxed) % BDL_ENTRIES;
    let byte_offset = seg * BDL_SEGMENT_BYTES;

    // Maximum i16 samples that fit in one segment.
    let max_samples = BDL_SEGMENT_BYTES / core::mem::size_of::<i16>();
    let to_write = samples.len().min(max_samples);

    // Safety: DMA_BUF is only written here (single-threaded kernel context)
    // and read by the HDA DMA engine through physical memory.
    unsafe {
        let dst = DMA_BUF.data.as_mut_ptr().add(byte_offset) as *mut i16;
        core::ptr::copy_nonoverlapping(samples.as_ptr(), dst, to_write);
    }

    // Advance the write segment pointer.
    WRITE_SEGMENT.fetch_add(1, Ordering::Relaxed);

    to_write
}

// =============================================================================
// Software-mixer integration
// =============================================================================

/// Fill one BDL segment with output from the software mixer.
fn _refill_segment(seg: usize) {
    let seg_idx = seg % BDL_ENTRIES;
    let byte_offset = seg_idx * BDL_SEGMENT_BYTES;
    let sample_count = BDL_SEGMENT_BYTES / core::mem::size_of::<i16>();

    // Safety: DMA_BUF written here, read by HDA DMA via physical address.
    unsafe {
        let dst = DMA_BUF.data.as_mut_ptr().add(byte_offset) as *mut i16;
        let slice = core::slice::from_raw_parts_mut(dst, sample_count);
        // Call into the software mixer to fill this segment.
        super::mixer::sw_mix_frame(slice);
    }
}

/// Periodic tick: check whether the HDA DMA engine has finished consuming
/// the current segment (IOC fired) and, if so, refill it from the software
/// mixer.
///
/// Call this from the timer interrupt handler or any periodic kernel callback
/// to keep the DMA pipeline fed.  At 48 kHz with 1024-frame segments this
/// must be called at least once every ~21 ms.
pub fn hda_tick() {
    if !PLAYBACK_RUNNING.load(Ordering::Relaxed) {
        return;
    }

    // Read and clear the IOC status bit without holding the heavy controller
    // lock for longer than necessary.
    let bar0 = HDA_MMIO_BASE.load(Ordering::Relaxed);
    if bar0 == 0 {
        return;
    }

    // Read SDnSTS for stream 0.  We compute the output stream base inline here
    // to avoid locking HDA_STATE.
    let gcap = unsafe { core::ptr::read_volatile((bar0 + GCAP as u64) as *const u32) };
    let iss = (gcap >> 8) & 0xF;
    let sd_base = bar0 + 0x80_u64 + (iss as u64) * 0x20;
    let sts_addr = (sd_base + SDnSTS as u64) as *mut u8;

    let sts = unsafe { core::ptr::read_volatile(sts_addr) };
    if sts & SDSTS_BCIS == 0 {
        return; // no completion yet — nothing to do
    }

    // Clear the IOC bit (write-1-to-clear).
    unsafe {
        core::ptr::write_volatile(sts_addr, SDSTS_BCIS);
    }

    // Determine which segment the DMA engine just finished.
    // The LPIB register tells us the byte position within the cyclic buffer.
    let lpib_addr = (sd_base + SDnLPIB as u64) as *const u32;
    let lpib = unsafe { core::ptr::read_volatile(lpib_addr) } as usize;

    // The segment that was just completed is the one whose end falls at `lpib`.
    // Segment n spans [n*SEG_BYTES .. (n+1)*SEG_BYTES).
    // After the DMA engine finishes segment n, LPIB will be at (n+1)*SEG_BYTES
    // (or wrap to 0 for the last entry).  We refill segment n.
    let completed_seg = if lpib == 0 {
        BDL_ENTRIES - 1 // wrapped: last segment just completed
    } else {
        (lpib / BDL_SEGMENT_BYTES).saturating_sub(1) % BDL_ENTRIES
    };

    _refill_segment(completed_seg);
}

// =============================================================================
// Initialization
// =============================================================================

pub fn init() {
    // Scan PCI for HDA controllers (class 0x04, subclass 0x03)
    serial_println!("    [hda] Scanning for HD Audio controllers...");

    let hda_devices = crate::drivers::pci::find_by_class(0x04, 0x03);

    if let Some(dev) = hda_devices.first() {
        // Read BAR0 from PCI config space at offset 0x10.
        // Formula: enable-bit | (bus<<16) | (dev<<11) | (fn<<8) | offset
        let addr: u32 = 0x8000_0000
            | ((dev.bus as u32) << 16)
            | ((dev.device as u32) << 11)
            | ((dev.function as u32) << 8)
            | 0x10;

        crate::io::outl(0xCF8, addr);
        let bar0_raw = crate::io::inl(0xCFC);

        // Mask lower 4 bits (type/prefetch flags) to get the MMIO base address.
        let bar0_base = (bar0_raw & 0xFFFF_FFF0) as u64;

        if bar0_base != 0 {
            HDA_MMIO_BASE.store(bar0_base, Ordering::Relaxed);
            serial_println!(
                "    [hda] HDA controller found at {:02x}:{:02x}.{} BAR0={:#x}",
                dev.bus,
                dev.device,
                dev.function,
                bar0_base
            );

            let mut state = HDA_STATE.lock();
            *state = Some(HdaController::new(bar0_base));
            if let Some(ref mut ctrl) = *state {
                ctrl.reset();
                ctrl.enumerate_codecs();
                let (oss, iss, bss) = ctrl.capabilities();
                serial_println!("    [hda] Streams: {} out, {} in, {} bidir", oss, iss, bss);
                // Configure stream 0 for 48 kHz stereo 16-bit by default.
                // Callers may override with hda_set_output_stream().
                ctrl.configure_output_stream(0, 48000, 2, 16);
                serial_println!(
                    "    [hda] Default stream 0 configured (48kHz/2ch/16bit); \
                     call hda_start_playback() to begin"
                );
            }
        } else {
            serial_println!("    [hda] HDA device found but BAR0 is unassigned");
        }
    } else {
        serial_println!("    [hda] HDA driver loaded (no hardware detected)");
    }
}
