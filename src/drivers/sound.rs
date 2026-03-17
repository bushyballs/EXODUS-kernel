/// Audio framework — ALSA-inspired, no-heap, no-std.
///
/// Provides a sound-card registry and per-card PCM device abstraction.
/// The AC97 codec I/O base is defined for future hardware wiring; all
/// current operations are simulated through the static tables.
///
/// ## Safety / kernel rules enforced
/// - No `alloc::*` — no Vec, Box, String.
/// - No float arithmetic (`as f32` / `as f64` forbidden).
/// - No panic paths — fallible operations return `Option<T>` or `bool` / `usize`.
/// - All counters use `saturating_add` / `saturating_sub`.
/// - All sequence numbers use `wrapping_add`.
/// - Structs held in static Mutex are `Copy` and provide `const fn empty()`.
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const MAX_SOUND_CARDS: usize = 4;
pub const MAX_PCM_DEVICES: usize = 8;
pub const PCM_BUF_SIZE: usize = 4096;

/// AC97 mixer / PCM I/O base address.
pub const AC97_IO_BASE: u16 = 0x340;

// ---------------------------------------------------------------------------
// PCM sample format tags
// ---------------------------------------------------------------------------

pub const SNDRV_PCM_FORMAT_S8: u8 = 0;
pub const SNDRV_PCM_FORMAT_S16_LE: u8 = 2;
pub const SNDRV_PCM_FORMAT_S32_LE: u8 = 10;

// ---------------------------------------------------------------------------
// PCM device state machine
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PcmState {
    Open,
    Setup,
    Prepared,
    Running,
    Stopped,
}

// ---------------------------------------------------------------------------
// SoundCard
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct SoundCard {
    pub id: u32,
    /// Human-readable card name (UTF-8); only first `name_len` bytes valid.
    pub name: [u8; 32],
    /// Short driver identifier (UTF-8); only first meaningful bytes valid
    /// (NUL-terminated at most 16 bytes).
    pub driver: [u8; 16],
    /// Slot is occupied when `true`.
    pub active: bool,
}

impl SoundCard {
    pub const fn empty() -> Self {
        SoundCard {
            id: 0,
            name: [0u8; 32],
            driver: [0u8; 16],
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// PcmDevice
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct PcmDevice {
    /// Owning card id.
    pub card_id: u32,
    /// Device number within the card (0-based).
    pub device_num: u8,
    /// `true` = capture (ADC), `false` = playback (DAC).
    pub is_capture: bool,
    pub state: PcmState,
    /// One of the `SNDRV_PCM_FORMAT_*` constants.
    pub format: u8,
    pub channels: u8,
    pub rate_hz: u32,
    pub period_size: u32,
    /// Circular audio data buffer.
    pub buf: [u8; PCM_BUF_SIZE],
    /// Write position within `buf` (byte index, wraps at PCM_BUF_SIZE).
    pub buf_pos: u32,
    /// Number of bytes currently stored in the circular buffer.
    pub buf_fill: u32,
    /// Incremented (saturating) each time a write is attempted when buf full.
    pub underrun_count: u32,
    /// Slot is occupied when `true`.
    pub active: bool,
}

impl PcmDevice {
    pub const fn empty() -> Self {
        PcmDevice {
            card_id: 0,
            device_num: 0,
            is_capture: false,
            state: PcmState::Open,
            format: SNDRV_PCM_FORMAT_S16_LE,
            channels: 2,
            rate_hz: 48000,
            period_size: 1024,
            buf: [0u8; PCM_BUF_SIZE],
            buf_pos: 0,
            buf_fill: 0,
            underrun_count: 0,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global tables
// ---------------------------------------------------------------------------

static SOUND_CARDS: Mutex<[SoundCard; MAX_SOUND_CARDS]> =
    Mutex::new([SoundCard::empty(); MAX_SOUND_CARDS]);

static PCM_DEVICES: Mutex<[PcmDevice; MAX_PCM_DEVICES]> =
    Mutex::new([PcmDevice::empty(); MAX_PCM_DEVICES]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy at most 32 bytes from `src` into `dst[..32]`, return bytes copied.
fn copy32(dst: &mut [u8; 32], src: &[u8]) -> usize {
    let len = src.len().min(32);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len
}

/// Copy at most 16 bytes from `src` into `dst[..16]`, return bytes copied.
fn copy16(dst: &mut [u8; 16], src: &[u8]) -> usize {
    let len = src.len().min(16);
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    len
}

/// Find the index of the sound card with `id`, or `None`.
fn card_index(cards: &[SoundCard; MAX_SOUND_CARDS], id: u32) -> Option<usize> {
    for i in 0..MAX_SOUND_CARDS {
        if cards[i].active && cards[i].id == id {
            return Some(i);
        }
    }
    None
}

/// Find the first free sound card slot, or `None`.
fn free_card_slot(cards: &[SoundCard; MAX_SOUND_CARDS]) -> Option<usize> {
    for i in 0..MAX_SOUND_CARDS {
        if !cards[i].active {
            return Some(i);
        }
    }
    None
}

/// Find the first free PCM device slot, or `None`.
fn free_pcm_slot(devs: &[PcmDevice; MAX_PCM_DEVICES]) -> Option<usize> {
    for i in 0..MAX_PCM_DEVICES {
        if !devs[i].active {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Sound card management
// ---------------------------------------------------------------------------

/// Register a new sound card.
///
/// Returns `Some(card_id)` on success, `None` if the card table is full.
pub fn snd_register_card(name: &[u8], driver: &[u8]) -> Option<u32> {
    let mut cards = SOUND_CARDS.lock();
    let slot = free_card_slot(&cards)?;

    // id is 1-based slot number — stable within a session.
    let id = (slot as u32).wrapping_add(1);

    cards[slot] = SoundCard::empty();
    cards[slot].id = id;
    copy32(&mut cards[slot].name, name);
    copy16(&mut cards[slot].driver, driver);
    cards[slot].active = true;

    Some(id)
}

// ---------------------------------------------------------------------------
// PCM device lifecycle
// ---------------------------------------------------------------------------

/// Open a PCM device on card `card_id`.
///
/// Creates a new slot in the PCM device table.  Returns the PCM device slot
/// index as `Some(pcm_idx)`, or `None` if the card is not found or the PCM
/// table is full.
pub fn snd_pcm_open(card_id: u32, device_num: u8, is_capture: bool) -> Option<u32> {
    // Verify the card exists.
    {
        let cards = SOUND_CARDS.lock();
        card_index(&cards, card_id)?;
    }

    let mut devs = PCM_DEVICES.lock();
    let slot = free_pcm_slot(&devs)?;

    devs[slot] = PcmDevice::empty();
    devs[slot].card_id = card_id;
    devs[slot].device_num = device_num;
    devs[slot].is_capture = is_capture;
    devs[slot].state = PcmState::Open;
    devs[slot].active = true;

    Some(slot as u32)
}

/// Configure hardware parameters for an open PCM device.
///
/// Transitions state to `Setup`.  Returns `false` if `pcm_idx` is out of
/// range or the slot is inactive.
pub fn snd_pcm_hw_params(
    pcm_idx: u32,
    format: u8,
    channels: u8,
    rate_hz: u32,
    period_size: u32,
) -> bool {
    let idx = pcm_idx as usize;
    if idx >= MAX_PCM_DEVICES {
        return false;
    }
    let mut devs = PCM_DEVICES.lock();
    if !devs[idx].active {
        return false;
    }
    devs[idx].format = format;
    devs[idx].channels = channels;
    devs[idx].rate_hz = rate_hz;
    devs[idx].period_size = period_size;
    devs[idx].state = PcmState::Setup;
    true
}

/// Prepare a PCM device for playback or capture.
///
/// Clears the circular buffer and transitions state to `Prepared`.
/// Returns `false` if `pcm_idx` is out of range or the slot is inactive.
pub fn snd_pcm_prepare(pcm_idx: u32) -> bool {
    let idx = pcm_idx as usize;
    if idx >= MAX_PCM_DEVICES {
        return false;
    }
    let mut devs = PCM_DEVICES.lock();
    if !devs[idx].active {
        return false;
    }
    // Zero the buffer without heap.
    let mut i = 0usize;
    while i < PCM_BUF_SIZE {
        devs[idx].buf[i] = 0;
        i = i.saturating_add(1);
    }
    devs[idx].buf_pos = 0;
    devs[idx].buf_fill = 0;
    devs[idx].underrun_count = 0;
    devs[idx].state = PcmState::Prepared;
    true
}

/// Start a PCM device (transitions to `Running`).
///
/// Returns `false` if `pcm_idx` is out of range or the slot is inactive.
pub fn snd_pcm_start(pcm_idx: u32) -> bool {
    let idx = pcm_idx as usize;
    if idx >= MAX_PCM_DEVICES {
        return false;
    }
    let mut devs = PCM_DEVICES.lock();
    if !devs[idx].active {
        return false;
    }
    devs[idx].state = PcmState::Running;
    true
}

/// Stop a PCM device (transitions to `Stopped`).
///
/// Returns `false` if `pcm_idx` is out of range or the slot is inactive.
pub fn snd_pcm_stop(pcm_idx: u32) -> bool {
    let idx = pcm_idx as usize;
    if idx >= MAX_PCM_DEVICES {
        return false;
    }
    let mut devs = PCM_DEVICES.lock();
    if !devs[idx].active {
        return false;
    }
    devs[idx].state = PcmState::Stopped;
    true
}

/// Write audio data into the playback circular buffer.
///
/// Copies up to `len` bytes from `data` into the circular buffer, advancing
/// `buf_pos` with wrapping arithmetic.  If the buffer is full, increments
/// `underrun_count` with `saturating_add` and returns 0.
///
/// Returns the number of bytes actually written.
pub fn snd_pcm_write(pcm_idx: u32, data: &[u8], len: usize) -> usize {
    let idx = pcm_idx as usize;
    if idx >= MAX_PCM_DEVICES || len == 0 {
        return 0;
    }
    let mut devs = PCM_DEVICES.lock();
    if !devs[idx].active {
        return 0;
    }

    let avail = (PCM_BUF_SIZE as u32).saturating_sub(devs[idx].buf_fill) as usize;
    if avail == 0 {
        devs[idx].underrun_count = devs[idx].underrun_count.saturating_add(1);
        return 0;
    }

    let to_write = len.min(avail).min(data.len());
    let mut written = 0usize;

    while written < to_write {
        let pos = devs[idx].buf_pos as usize;
        devs[idx].buf[pos] = data[written];
        devs[idx].buf_pos = ((devs[idx].buf_pos as usize).wrapping_add(1) % PCM_BUF_SIZE) as u32;
        devs[idx].buf_fill = devs[idx].buf_fill.saturating_add(1);
        written = written.saturating_add(1);
    }

    written
}

/// Read audio data from the capture circular buffer.
///
/// Copies up to `len` bytes from the front of `buf` into `out`.
/// Returns the number of bytes actually read.
pub fn snd_pcm_read(pcm_idx: u32, out: &mut [u8; PCM_BUF_SIZE], len: usize) -> usize {
    let idx = pcm_idx as usize;
    if idx >= MAX_PCM_DEVICES || len == 0 {
        return 0;
    }
    let mut devs = PCM_DEVICES.lock();
    if !devs[idx].active {
        return 0;
    }

    let fill = devs[idx].buf_fill as usize;
    let to_read = len.min(fill).min(PCM_BUF_SIZE);

    // Calculate read start: buf_pos is the *write* head; read head is
    // (buf_pos - buf_fill) wrapping around PCM_BUF_SIZE.
    let write_head = devs[idx].buf_pos as usize;
    let read_head = if write_head >= fill {
        write_head.saturating_sub(fill)
    } else {
        PCM_BUF_SIZE.saturating_sub(fill.saturating_sub(write_head))
    };

    let mut read = 0usize;
    while read < to_read {
        let src_pos = (read_head.wrapping_add(read)) % PCM_BUF_SIZE;
        out[read] = devs[idx].buf[src_pos];
        read = read.saturating_add(1);
    }

    // Consume the read bytes from the fill counter.
    devs[idx].buf_fill = devs[idx].buf_fill.saturating_sub(to_read as u32);

    read
}

/// Return the number of bytes available (free space) in the buffer.
///
/// Returns 0 if `pcm_idx` is out of range or the slot is inactive.
pub fn snd_pcm_avail(pcm_idx: u32) -> u32 {
    let idx = pcm_idx as usize;
    if idx >= MAX_PCM_DEVICES {
        return 0;
    }
    let devs = PCM_DEVICES.lock();
    if !devs[idx].active {
        return 0;
    }
    (PCM_BUF_SIZE as u32).saturating_sub(devs[idx].buf_fill)
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the audio framework.
///
/// Registers one virtual AC97 sound card and opens a stereo 48 kHz S16-LE
/// playback PCM device on it.
pub fn init() {
    let card_name = b"Genesis Audio";
    let card_driver = b"ac97";

    match snd_register_card(card_name, card_driver) {
        None => {
            serial_println!("[sound] audio framework initialized (no card slot available)");
        }
        Some(card_id) => {
            // Open a playback PCM device (device_num=0, capture=false).
            if let Some(pcm_idx) = snd_pcm_open(card_id, 0, false) {
                snd_pcm_hw_params(
                    pcm_idx,
                    SNDRV_PCM_FORMAT_S16_LE,
                    2,     // stereo
                    48000, // 48 kHz
                    1024,  // period size in frames
                );
                snd_pcm_prepare(pcm_idx);
            }
            serial_println!("[sound] audio framework initialized");
        }
    }
}
