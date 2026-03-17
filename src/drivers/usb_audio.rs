/// usb_audio — USB Audio Class 2.0 gadget function
///
/// Presents a UAC2 streaming interface to the USB host:
///   - Two AudioStreaming interfaces: one for playback (host→device),
///     one for capture (device→host)
///   - Isochronous endpoints for real-time PCM transfer
///   - Fixed-size ring buffers for PCM samples (no heap)
///   - Volume/mute control via Feature Unit
///
/// Inspired by: Linux UAC2 gadget (f_uac2.c). All code is original.
/// Rules: no_std, no heap, no floats, no panics, saturating counters.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const UAC2_CHANNELS: usize = 2; // stereo
pub const UAC2_BIT_DEPTH: usize = 16; // S16LE
pub const UAC2_SAMPLE_RATE: u32 = 48_000; // 48 kHz
pub const UAC2_BYTES_PER_SAMPLE: usize = UAC2_CHANNELS * (UAC2_BIT_DEPTH / 8); // 4

/// Ring buffer: 48 ms of audio at 48 kHz = 2304 stereo S16 frames = 9216 bytes
const RING_FRAMES: usize = 2304;
const RING_BYTES: usize = RING_FRAMES * UAC2_BYTES_PER_SAMPLE; // 9216

// UAC2 class-specific request codes
pub const UAC2_SET_CUR: u8 = 0x01;
pub const UAC2_GET_CUR: u8 = 0x81;

// Feature Unit control selectors
pub const FU_MUTE_CONTROL: u8 = 0x01;
pub const FU_VOLUME_CONTROL: u8 = 0x02;

// ---------------------------------------------------------------------------
// PCM ring buffer (lock-protected, no atomics — kernel context only)
// ---------------------------------------------------------------------------

struct RingBuf {
    buf: [u8; RING_BYTES],
    head: usize,  // write pointer (producer)
    tail: usize,  // read  pointer (consumer)
    count: usize, // bytes available
}

impl RingBuf {
    const fn new() -> Self {
        RingBuf {
            buf: [0u8; RING_BYTES],
            head: 0,
            tail: 0,
            count: 0,
        }
    }

    /// Push raw PCM bytes. Drops oldest bytes on overflow (overwrite mode).
    fn push(&mut self, data: &[u8]) {
        let mut i = 0usize;
        while i < data.len() {
            self.buf[self.head] = data[i];
            self.head = (self.head.saturating_add(1)) % RING_BYTES;
            if self.count < RING_BYTES {
                self.count = self.count.saturating_add(1);
            } else {
                // Overwrite: advance tail
                self.tail = (self.tail.saturating_add(1)) % RING_BYTES;
            }
            i = i.saturating_add(1);
        }
    }

    /// Pop up to `out.len()` bytes. Returns bytes actually read.
    fn pop(&mut self, out: &mut [u8]) -> usize {
        let to_read = out.len().min(self.count);
        let mut i = 0usize;
        while i < to_read {
            out[i] = self.buf[self.tail];
            self.tail = (self.tail.saturating_add(1)) % RING_BYTES;
            i = i.saturating_add(1);
        }
        self.count = self.count.saturating_sub(to_read);
        to_read
    }

    fn available(&self) -> usize {
        self.count
    }
}

// ---------------------------------------------------------------------------
// UAC2 state
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq)]
pub enum StreamDir {
    Playback,
    Capture,
}

pub struct UsbAudioState {
    pub enabled: bool,
    pub playback_active: bool,
    pub capture_active: bool,
    pub volume_db: i16, // Q8.8 dB, range -127..0
    pub muted: bool,
    pub sample_rate: u32,
    pub frames_played: u64,
    pub frames_captured: u64,
}

impl UsbAudioState {
    const fn new() -> Self {
        UsbAudioState {
            enabled: false,
            playback_active: false,
            capture_active: false,
            volume_db: 0,
            muted: false,
            sample_rate: UAC2_SAMPLE_RATE,
            frames_played: 0,
            frames_captured: 0,
        }
    }
}

// Use separate Mutex for state and each ring to avoid nested locks.
static AUDIO_STATE: Mutex<UsbAudioState> = Mutex::new(UsbAudioState::new());
static PLAYBACK_RING: Mutex<RingBuf> = Mutex::new(RingBuf::new());
static CAPTURE_RING: Mutex<RingBuf> = Mutex::new(RingBuf::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Enable the UAC2 gadget function.
pub fn uac2_enable() {
    let mut st = AUDIO_STATE.lock();
    st.enabled = true;
}

/// Disable and reset.
pub fn uac2_disable() {
    let mut st = AUDIO_STATE.lock();
    st.enabled = false;
    st.playback_active = false;
    st.capture_active = false;
    st.frames_played = 0;
    st.frames_captured = 0;
}

/// Start streaming in one direction (called when host opens isochronous pipe).
pub fn uac2_start_stream(dir: StreamDir) {
    let mut st = AUDIO_STATE.lock();
    match dir {
        StreamDir::Playback => st.playback_active = true,
        StreamDir::Capture => st.capture_active = true,
    }
}

/// Stop streaming in one direction.
pub fn uac2_stop_stream(dir: StreamDir) {
    let mut st = AUDIO_STATE.lock();
    match dir {
        StreamDir::Playback => st.playback_active = false,
        StreamDir::Capture => st.capture_active = false,
    }
}

/// Receive a playback isochronous packet from the USB host (host→device).
/// `pcm_data` must be raw S16LE stereo bytes.
/// Returns the number of frames consumed.
pub fn uac2_recv_playback(pcm_data: &[u8]) -> usize {
    let frames = pcm_data.len() / UAC2_BYTES_PER_SAMPLE;
    {
        let st = AUDIO_STATE.lock();
        if !st.enabled || !st.playback_active {
            return 0;
        }
    }
    PLAYBACK_RING.lock().push(pcm_data);
    let mut st = AUDIO_STATE.lock();
    st.frames_played = st.frames_played.saturating_add(frames as u64);
    frames
}

/// Drain playback PCM into `out` (for the audio hardware to consume).
/// Returns bytes actually read.
pub fn uac2_drain_playback(out: &mut [u8]) -> usize {
    PLAYBACK_RING.lock().pop(out)
}

/// Feed capture PCM from the audio hardware into the capture ring buffer.
pub fn uac2_feed_capture(pcm_data: &[u8]) {
    {
        let st = AUDIO_STATE.lock();
        if !st.enabled || !st.capture_active {
            return;
        }
    }
    CAPTURE_RING.lock().push(pcm_data);
    let frames = pcm_data.len() / UAC2_BYTES_PER_SAMPLE;
    let mut st = AUDIO_STATE.lock();
    st.frames_captured = st.frames_captured.saturating_add(frames as u64);
}

/// Build an isochronous IN packet for sending to the USB host (capture path).
/// Returns bytes written.
pub fn uac2_build_capture_packet(out: &mut [u8]) -> usize {
    CAPTURE_RING.lock().pop(out)
}

/// Handle a UAC2 class-specific control request.
/// Returns Ok(response_len) or Err(()).
pub fn uac2_handle_control(
    request: u8,
    control_selector: u8,
    data: &[u8],
    resp: &mut [u8; 8],
) -> Result<usize, ()> {
    match request {
        UAC2_SET_CUR => match control_selector {
            FU_MUTE_CONTROL => {
                if data.is_empty() {
                    return Err(());
                }
                AUDIO_STATE.lock().muted = data[0] != 0;
                Ok(0)
            }
            FU_VOLUME_CONTROL => {
                if data.len() < 2 {
                    return Err(());
                }
                let vol = ((data[1] as i16) << 8) | data[0] as i16;
                AUDIO_STATE.lock().volume_db = vol;
                Ok(0)
            }
            _ => Err(()),
        },
        UAC2_GET_CUR => {
            let st = AUDIO_STATE.lock();
            match control_selector {
                FU_MUTE_CONTROL => {
                    resp[0] = if st.muted { 1 } else { 0 };
                    Ok(1)
                }
                FU_VOLUME_CONTROL => {
                    resp[0] = (st.volume_db & 0xFF) as u8;
                    resp[1] = ((st.volume_db >> 8) & 0xFF) as u8;
                    Ok(2)
                }
                _ => Err(()),
            }
        }
        _ => Err(()),
    }
}

/// Returns (playback_available_bytes, capture_available_bytes).
pub fn uac2_buffer_levels() -> (usize, usize) {
    (
        PLAYBACK_RING.lock().available(),
        CAPTURE_RING.lock().available(),
    )
}

/// Build the UAC2 class-specific descriptors into buf.
/// Returns bytes written.
pub fn uac2_build_descriptors(buf: &mut [u8; 256]) -> usize {
    // Minimal UAC2 class-specific AC interface descriptor (header only, 9 bytes)
    // bcdADC=0x0200, wTotalLength=9, bInCollection=0
    let desc: [u8; 9] = [
        9,    // bLength
        0x24, // bDescriptorType = CS_INTERFACE
        0x01, // bDescriptorSubtype = HEADER
        0x00, 0x02, // bcdADC = 2.00
        0x09, 0x00, // wTotalLength = 9
        0x00, // bCategory = FUNCTION_SUBCLASS_UNDEFINED
        0x00, // bmControls
    ];
    let len = desc.len().min(256);
    let mut i = 0usize;
    while i < len {
        buf[i] = desc[i];
        i = i.saturating_add(1);
    }
    len
}

pub fn init() {
    serial_println!(
        "[usb_audio] USB Audio Class 2.0 gadget initialized (48kHz stereo S16LE, {}B ring)",
        RING_BYTES
    );
}
