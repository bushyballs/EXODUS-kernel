/// PCM device abstraction — ALSA-compatible naming and ring-buffer model
///
/// Each PCM stream owns a static ring buffer of PCM_PERIODS × PCM_PERIOD_FRAMES
/// stereo i16 frames (8 KiB per stream).  Callers write interleaved samples via
/// `pcm_write`; the HDA interrupt fetches completed periods via `pcm_pull_period`.
///
/// Rules upheld throughout:
///   - No float casts (no `as f32` / `as f64`)
///   - No heap (no Vec / Box / String)
///   - saturating arithmetic for counters, wrapping_add for ring indices
///   - Early returns instead of panics
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of concurrent PCM streams.
pub const MAX_PCM_DEVICES: usize = 4;

/// Frames per period (one interrupt quantum).  512 frames × 2 ch × 2 B = 2 KiB.
pub const PCM_PERIOD_FRAMES: usize = 512;

/// Number of periods in each stream's ring buffer.
pub const PCM_PERIODS: usize = 4;

/// Interleaved samples per period (frames × 2 channels).
const PERIOD_SAMPLES: usize = PCM_PERIOD_FRAMES * 2;

// ---------------------------------------------------------------------------
// Public enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmDirection {
    Playback,
    Capture,
}

/// Supported PCM sample formats.
/// `Float32Le` is accepted by the API but converted to S16 internally —
/// no float arithmetic is performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmFormat {
    S16Le,
    S24Le,
    S32Le,
    U8,
    Float32Le,
}

/// Life-cycle state of a PCM stream, mirroring ALSA states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmState {
    Open,
    Prepared,
    Running,
    Paused,
    Suspended,
    Disconnected,
}

// ---------------------------------------------------------------------------
// Hardware parameter block
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct PcmHwParams {
    pub format: PcmFormat,
    /// Sample rate in Hz (8 000 … 192 000).
    pub rate: u32,
    /// Channel count: 1 = mono, 2 = stereo, up to 8.
    pub channels: u8,
    /// Frames per interrupt (advisory; driver may override with PCM_PERIOD_FRAMES).
    pub period_frames: u32,
    /// Total periods in the ring (advisory; driver uses PCM_PERIODS).
    pub periods: u32,
}

impl PcmHwParams {
    pub const fn default_stereo_48k() -> Self {
        PcmHwParams {
            format: PcmFormat::S16Le,
            rate: 48_000,
            channels: 2,
            period_frames: PCM_PERIOD_FRAMES as u32,
            periods: PCM_PERIODS as u32,
        }
    }
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

/// One PCM stream with an embedded ring buffer.
///
/// Ring layout: `ring[period_index][sample_index]` where each period holds
/// `PCM_PERIOD_FRAMES * 2` interleaved stereo i16 samples.
pub struct PcmStream {
    pub id: u32,
    pub direction: PcmDirection,
    pub hw_params: PcmHwParams,
    pub state: PcmState,

    /// Ring buffer: [period][frame*channels].
    pub ring: [[i16; PERIOD_SAMPLES]; PCM_PERIODS],

    /// Index of the next period the *writer* will fill.
    pub write_period: usize,
    /// Index of the next period the *reader* (HDA DMA) will consume.
    pub read_period: usize,
    /// How many samples have been written into the current write period so far.
    write_sample_pos: usize,

    pub frames_written: u64,
    pub frames_read: u64,
    pub underruns: u32,
    pub overruns: u32,
}

impl PcmStream {
    const fn new_empty() -> Self {
        PcmStream {
            id: 0,
            direction: PcmDirection::Playback,
            hw_params: PcmHwParams::default_stereo_48k(),
            state: PcmState::Open,
            ring: [[0i16; PERIOD_SAMPLES]; PCM_PERIODS],
            write_period: 0,
            read_period: 0,
            write_sample_pos: 0,
            frames_written: 0,
            frames_read: 0,
            underruns: 0,
            overruns: 0,
        }
    }

    /// Number of fully-filled periods waiting to be read.
    fn periods_available(&self) -> usize {
        if self.write_period >= self.read_period {
            self.write_period - self.read_period
        } else {
            PCM_PERIODS - self.read_period + self.write_period
        }
    }

    /// Number of periods the writer can still fill before overrunning.
    fn periods_free(&self) -> usize {
        // Keep one slot reserved so full != empty.
        PCM_PERIODS
            .saturating_sub(1)
            .saturating_sub(self.periods_available())
    }

    /// Write interleaved i16 samples into the ring.  Returns frames written.
    /// Callers supply *samples* (not frames); we convert: frames = samples / ch.
    fn write_samples(&mut self, samples: &[i16]) -> usize {
        if samples.is_empty() {
            return 0;
        }
        let ch = self.hw_params.channels.max(1) as usize;
        let mut written = 0usize;

        for &s in samples {
            // If current write period is full, advance to next.
            if self.write_sample_pos >= PERIOD_SAMPLES {
                if self.periods_free() == 0 {
                    // Overrun: the reader hasn't consumed periods fast enough.
                    self.overruns = self.overruns.saturating_add(1);
                    break;
                }
                self.write_period = (self.write_period + 1) % PCM_PERIODS;
                self.write_sample_pos = 0;
            }
            self.ring[self.write_period][self.write_sample_pos] = s;
            self.write_sample_pos = self.write_sample_pos.wrapping_add(1);
            written = written.wrapping_add(1);
        }

        // frames_written tracks *complete* frames only.
        let frames = written / ch.max(1);
        self.frames_written = self.frames_written.saturating_add(frames as u64);
        frames
    }

    /// Read up to `out.len()` samples from the ring.  Returns samples read.
    fn read_samples(&mut self, out: &mut [i16]) -> usize {
        let ch = self.hw_params.channels.max(1) as usize;
        let mut read = 0usize;

        // Walk complete periods only.
        while read < out.len() && self.periods_available() > 0 {
            let remaining_in_out = out.len() - read;
            let remaining_in_period = PERIOD_SAMPLES; // always a full period
            let to_copy = remaining_in_out.min(remaining_in_period);
            let period = self.read_period;
            out[read..read + to_copy].copy_from_slice(&self.ring[period][..to_copy]);
            read = read.wrapping_add(to_copy);
            self.read_period = (self.read_period + 1) % PCM_PERIODS;
            let frames = to_copy / ch.max(1);
            self.frames_read = self.frames_read.saturating_add(frames as u64);
        }
        read
    }

    /// Fill `out` with exactly one period from the ring, or silence on underrun.
    /// Returns `true` if real audio was delivered, `false` on underrun (silence).
    fn pull_period(&mut self, out: &mut [i16; PERIOD_SAMPLES]) -> bool {
        if self.state != PcmState::Running {
            for s in out.iter_mut() {
                *s = 0;
            }
            return false;
        }
        if self.periods_available() == 0 {
            self.underruns = self.underruns.saturating_add(1);
            for s in out.iter_mut() {
                *s = 0;
            }
            return false;
        }
        let period = self.read_period;
        out.copy_from_slice(&self.ring[period]);
        self.read_period = (self.read_period + 1) % PCM_PERIODS;
        let ch = self.hw_params.channels.max(1) as usize;
        self.frames_read = self
            .frames_read
            .saturating_add((PERIOD_SAMPLES / ch.max(1)) as u64);
        true
    }

    /// Number of frames available to read (playback) or space to write (capture).
    fn avail_frames(&self) -> usize {
        let ch = self.hw_params.channels.max(1) as usize;
        match self.direction {
            PcmDirection::Playback => self.periods_available() * PCM_PERIOD_FRAMES,
            PcmDirection::Capture => self.periods_free() * PCM_PERIOD_FRAMES * ch,
        }
    }
}

// ---------------------------------------------------------------------------
// Global stream table
// ---------------------------------------------------------------------------

/// Wraps [Option<PcmStream>; MAX_PCM_DEVICES] but needs a const initializer.
struct PcmTable {
    slots: [Option<PcmStream>; MAX_PCM_DEVICES],
    next_id: u32,
}

impl PcmTable {
    const fn new() -> Self {
        PcmTable {
            slots: [None, None, None, None],
            next_id: 1,
        }
    }

    fn find_mut(&mut self, id: u32) -> Option<&mut PcmStream> {
        for slot in self.slots.iter_mut() {
            if let Some(ref mut s) = slot {
                if s.id == id {
                    return Some(s);
                }
            }
        }
        None
    }
}

static PCM_STREAMS: Mutex<PcmTable> = Mutex::new(PcmTable::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Open a new PCM stream.  Returns the stream id on success, `None` if all
/// slots are occupied.
pub fn pcm_open(direction: PcmDirection, params: PcmHwParams) -> Option<u32> {
    let mut tbl = PCM_STREAMS.lock();
    let id = tbl.next_id;
    for i in 0..MAX_PCM_DEVICES {
        if tbl.slots[i].is_none() {
            tbl.next_id = tbl.next_id.saturating_add(1);
            let mut s = PcmStream::new_empty();
            s.id = id;
            s.direction = direction;
            s.hw_params = params;
            s.state = PcmState::Prepared;
            tbl.slots[i] = Some(s);
            serial_println!(
                "    [pcm] stream {} opened ({:?})",
                id,
                match direction {
                    PcmDirection::Playback => "playback",
                    PcmDirection::Capture => "capture",
                }
            );
            return Some(id);
        }
    }
    serial_println!("    [pcm] pcm_open: no free slots");
    None
}

/// Close and release a PCM stream.
pub fn pcm_close(id: u32) {
    let mut tbl = PCM_STREAMS.lock();
    for slot in tbl.slots.iter_mut() {
        if let Some(ref s) = slot {
            if s.id == id {
                *slot = None;
                serial_println!("    [pcm] stream {} closed", id);
                return;
            }
        }
    }
}

/// Write interleaved i16 samples into a playback stream.
/// Returns the number of *frames* written.
pub fn pcm_write(id: u32, samples: &[i16]) -> usize {
    let mut tbl = PCM_STREAMS.lock();
    if let Some(s) = tbl.find_mut(id) {
        if s.direction != PcmDirection::Playback {
            return 0;
        }
        s.write_samples(samples)
    } else {
        0
    }
}

/// Read interleaved i16 samples from a capture stream.
/// Returns the number of samples read.
pub fn pcm_read(id: u32, out: &mut [i16]) -> usize {
    let mut tbl = PCM_STREAMS.lock();
    if let Some(s) = tbl.find_mut(id) {
        if s.direction != PcmDirection::Capture {
            return 0;
        }
        s.read_samples(out)
    } else {
        0
    }
}

/// Transition the stream to Running.
pub fn pcm_start(id: u32) {
    let mut tbl = PCM_STREAMS.lock();
    if let Some(s) = tbl.find_mut(id) {
        s.state = PcmState::Running;
    }
}

/// Pause a running stream (preserves buffer contents).
pub fn pcm_pause(id: u32) {
    let mut tbl = PCM_STREAMS.lock();
    if let Some(s) = tbl.find_mut(id) {
        if s.state == PcmState::Running {
            s.state = PcmState::Paused;
        }
    }
}

/// Discard all buffered audio and return the stream to Prepared state.
pub fn pcm_drop(id: u32) {
    let mut tbl = PCM_STREAMS.lock();
    if let Some(s) = tbl.find_mut(id) {
        s.write_period = 0;
        s.read_period = 0;
        s.write_sample_pos = 0;
        s.state = PcmState::Prepared;
    }
}

/// Return available frames (readable for capture, writable for playback).
pub fn pcm_avail(id: u32) -> usize {
    let tbl = PCM_STREAMS.lock();
    for slot in tbl.slots.iter() {
        if let Some(ref s) = slot {
            if s.id == id {
                return s.avail_frames();
            }
        }
    }
    0
}

/// Return the current state of a stream.
pub fn pcm_state(id: u32) -> PcmState {
    let tbl = PCM_STREAMS.lock();
    for slot in tbl.slots.iter() {
        if let Some(ref s) = slot {
            if s.id == id {
                return s.state;
            }
        }
    }
    PcmState::Disconnected
}

/// Called from the HDA interrupt handler to fetch one period of audio.
///
/// Fills `out` with `PCM_PERIOD_FRAMES * 2` interleaved stereo i16 samples.
/// Returns `true` if real audio was pulled; `false` means the buffer ran dry
/// (underrun) and silence was written instead.
pub fn pcm_pull_period(id: u32, out: &mut [i16; PERIOD_SAMPLES]) -> bool {
    let mut tbl = PCM_STREAMS.lock();
    if let Some(s) = tbl.find_mut(id) {
        s.pull_period(out)
    } else {
        for s in out.iter_mut() {
            *s = 0;
        }
        false
    }
}

/// Initialise the PCM subsystem (currently a no-op; table is statically initialised).
pub fn init() {
    serial_println!(
        "    [pcm] PCM device abstraction ready ({} slots, {}-frame periods, {} periods/stream)",
        MAX_PCM_DEVICES,
        PCM_PERIOD_FRAMES,
        PCM_PERIODS
    );
}
