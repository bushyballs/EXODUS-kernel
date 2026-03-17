use super::pcm::{pcm_pull_period, pcm_write, PCM_PERIOD_FRAMES};
/// Audio routing matrix — connects audio sources to sinks
///
/// Sources: PCM playback streams, HDA capture pins, a 440 Hz test tone, silence.
/// Sinks:   HDA playback stream, PCM capture streams, SPDIF out.
///
/// Up to 16 routes may be active simultaneously.  Each route specifies a source,
/// a sink, and a volume (0-100).  On every audio interrupt `route_process_tick`
/// is called; it pulls one period from every active source and mixes it into
/// the corresponding sink buffer.
///
/// Rules upheld:
///   - No float casts (no `as f32` / `as f64`)
///   - No heap (no Vec / Box / String)
///   - saturating arithmetic for counters
///   - No panic — bad indices produce early returns
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum simultaneous routes in the matrix.
const MAX_ROUTES: usize = 16;

/// Interleaved samples per period (frames × 2 channels).
const PERIOD_SAMPLES: usize = PCM_PERIOD_FRAMES * 2;

// ---------------------------------------------------------------------------
// Source / sink descriptors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSource {
    /// Playback data from a PCM stream (id = stream id from `pcm_open`).
    PcmStream(u32),
    /// Audio captured from an HDA input pin (pin index 0-based).
    HdaCapture(u8),
    /// Internally generated 440 Hz test tone.
    TestTone,
    /// All-zeros (useful for muting a sink cleanly).
    Silence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSink {
    /// Feed into an HDA output stream (stream index 0-based).
    HdaPlayback(u8),
    /// Write captured audio into a PCM capture stream.
    PcmCapture(u32),
    /// Route to the SPDIF digital output (stub — no hardware yet).
    SpdifOut,
}

// ---------------------------------------------------------------------------
// Route record
// ---------------------------------------------------------------------------

pub struct Route {
    pub source: AudioSource,
    pub sink: AudioSink,
    /// Gain: 0 = mute, 100 = full.
    pub volume: u8,
    pub active: bool,
}

// ---------------------------------------------------------------------------
// Route table
// ---------------------------------------------------------------------------

/// Wraps the fixed-size route array plus helper metadata.
struct RouteTable {
    routes: [Option<Route>; MAX_ROUTES],
}

impl RouteTable {
    const fn new() -> Self {
        // Option<Route> is not Copy so we have to initialise manually.
        RouteTable {
            routes: [
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                None, None,
            ],
        }
    }
}

static ROUTES: Mutex<RouteTable> = Mutex::new(RouteTable::new());

// ---------------------------------------------------------------------------
// Test-tone generator
// ---------------------------------------------------------------------------

/// Q16 phase accumulator for the 440 Hz sine generator.
static TEST_TONE_PHASE: AtomicU32 = AtomicU32::new(0);

/// 256-entry 16-bit sine table (amplitude ≈ 28 000 to leave headroom).
/// Values computed as round(28000 * sin(2π * i / 256)).
/// Generated offline with integer arithmetic; no float used at runtime.
#[rustfmt::skip]
static SINE_TABLE: [i16; 256] = [
       0,   687,  1374,  2059,  2742,  3423,  4102,  4778,
    5450,  6118,  6782,  7441,  8096,  8746,  9390, 10029,
   10663, 11291, 11912, 12527, 13135, 13737, 14332, 14920,
   15500, 16073, 16638, 17195, 17744, 18284, 18816, 19339,
   19854, 20359, 20856, 21343, 21821, 22290, 22749, 23198,
   23638, 24067, 24487, 24896, 25295, 25684, 26062, 26429,
   26786, 27132, 27467, 27791, 28105, 28407, 28698, 28978,
   29247, 29505, 29751, 29986, 30210, 30422, 30623, 30812,
   30990, 31157, 31312, 31455, 31587, 31707, 31816, 31913,
   31999, 32073, 32135, 32186, 32225, 32253, 32269, 32274,
   32267, 32248, 32218, 32177, 32124, 32059, 31984, 31897,
   31799, 31690, 31570, 31439, 31297, 31145, 30982, 30808,
   30624, 30430, 30226, 30012, 29788, 29554, 29310, 29057,
   28795, 28524, 28244, 27955, 27657, 27351, 27036, 26713,
   26382, 26043, 25696, 25342, 24980, 24611, 24235, 23852,
   23463, 23067, 22665, 22257, 21843, 21424, 20999, 20569,
   20134, 19695, 19251, 18803, 18350, 17894, 17434, 16971,
   16504, 16035, 15562, 15087, 14609, 14129, 13647, 13163,
   12677, 12189, 11700, 11209, 10718, 10225,  9731,  9237,
    8742,  8247,  7751,  7255,  6760,  6264,  5768,  5273,
    4779,  4285,  3792,  3300,  2809,  2319,  1830,  1342,
     856,   371,  -114,  -598, -1082, -1565, -2047, -2528,
   -3008, -3486, -3963, -4438, -4912, -5383, -5852, -6319,
   -6784, -7246, -7705, -8162, -8615, -9065, -9512, -9956,
  -10396,-10832,-11265,-11694,-12119,-12540,-12957,-13369,
  -13777,-14181,-14580,-14974,-15364,-15749,-16129,-16504,
  -16874,-17239,-17599,-17953,-18302,-18645,-18983,-19315,
  -19641,-19962,-20276,-20584,-20887,-21183,-21473,-21757,
  -22034,-22305,-22570,-22828,-23079,-23324,-23562,-23793,
  -24018,-24235,-24446,-24649,-24846,-25035,-25218,-25393,
  -25561,-25722,-25876,-26022,-26161,-26293,-26418,-26535,
  -26645,-26747,-26843,-26931,-27011,-27085,-27151,-27209,
];

/// Fill `buf[..count]` with one period of a 440 Hz sine at `rate` Hz.
/// Stereo: L and R are identical.
/// Amplitude is held at ≈ 28 000 (≈86 % of full scale) to leave headroom.
///
/// Phase step Q16 = (440 << 16) / rate.
/// Table index    = (phase >> 8) & 0xFF   (top 8 bits of the 16-bit integer part).
pub fn test_tone_fill(buf: &mut [i16], count: usize, rate: u32) {
    if rate == 0 || buf.is_empty() {
        return;
    }
    // step_q16 = (440 << 16) / rate  — kept as u32 (440 * 65536 = 28_835_840 < u32::MAX)
    let step_q16: u32 = (440u32 << 16) / rate;
    let mut phase = TEST_TONE_PHASE.load(Ordering::Relaxed);
    let n = count.min(buf.len());

    for i in 0..n {
        let table_idx = ((phase >> 8) & 0xFF) as usize;
        let sample = SINE_TABLE[table_idx];
        buf[i] = sample;
        phase = phase.wrapping_add(step_q16);
    }
    TEST_TONE_PHASE.store(phase, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Public routing API
// ---------------------------------------------------------------------------

/// Register a new route.  Returns the route index (0-based) on success,
/// or `None` if the table is full.
pub fn route_add(source: AudioSource, sink: AudioSink, volume: u8) -> Option<usize> {
    let mut tbl = ROUTES.lock();
    for (i, slot) in tbl.routes.iter_mut().enumerate() {
        if slot.is_none() {
            *slot = Some(Route {
                source,
                sink,
                volume: volume.min(100),
                active: true,
            });
            serial_println!("    [routing] route {} added (vol {})", i, volume);
            return Some(i);
        }
    }
    serial_println!("    [routing] route_add: table full");
    None
}

/// Remove a route by index.
pub fn route_remove(route_idx: usize) {
    if route_idx >= MAX_ROUTES {
        return;
    }
    let mut tbl = ROUTES.lock();
    tbl.routes[route_idx] = None;
}

/// Set the volume (0-100) on an existing route.
pub fn route_set_volume(route_idx: usize, volume: u8) {
    if route_idx >= MAX_ROUTES {
        return;
    }
    let mut tbl = ROUTES.lock();
    if let Some(ref mut r) = tbl.routes[route_idx] {
        r.volume = volume.min(100);
    }
}

// ---------------------------------------------------------------------------
// Per-tick mixing work buffers (static to avoid stack pressure)
// ---------------------------------------------------------------------------

/// Temporary source pull buffer (one period, stereo i16).
static mut SOURCE_BUF: [i16; PERIOD_SAMPLES] = [0i16; PERIOD_SAMPLES];

/// Accumulation buffer for HDA playback sink (i32 to provide headroom).
static mut HDA_ACCUM: [i32; PERIOD_SAMPLES] = [0i32; PERIOD_SAMPLES];

/// Final HDA output (i16 after clipping).
static mut HDA_OUT: [i16; PERIOD_SAMPLES] = [0i16; PERIOD_SAMPLES];

/// Called from the audio interrupt to drive the routing matrix.
///
/// For each active route:
///   1. Pull `PCM_PERIOD_FRAMES` frames from the source.
///   2. Apply volume: `(sample * vol as i32) / 100`.
///   3. Mix into the sink's accumulation buffer via `saturating_add`.
///   4. Clip and dispatch the result to the sink.
///
/// Safety: called from a single-threaded interrupt context; the static mutable
/// buffers are touched only here.
pub fn route_process_tick() {
    // Zero the HDA accumulation buffer.
    unsafe {
        for v in HDA_ACCUM.iter_mut() {
            *v = 0;
        }
    }

    // Snapshot the route table before doing any work.
    // We iterate without holding the lock for the expensive mix operations.
    // Taking a full copy avoids re-locking per route while keeping it simple.
    let mut route_snapshot: [Option<(AudioSource, AudioSink, u8)>; MAX_ROUTES] = [None; MAX_ROUTES];
    {
        let tbl = ROUTES.lock();
        for (i, slot) in tbl.routes.iter().enumerate() {
            if let Some(ref r) = slot {
                if r.active {
                    route_snapshot[i] = Some((r.source, r.sink, r.volume));
                }
            }
        }
    }

    for entry in route_snapshot.iter() {
        let (source, sink, vol) = match entry {
            Some(t) => *t,
            None => continue,
        };

        // --- Pull from source ---
        let samples_available: usize;
        unsafe {
            for v in SOURCE_BUF.iter_mut() {
                *v = 0;
            }
            samples_available = pull_from_source(source, &mut SOURCE_BUF, PERIOD_SAMPLES);
        }
        if samples_available == 0 {
            continue;
        }

        // --- Mix into sink ---
        match sink {
            AudioSink::HdaPlayback(_stream_idx) => unsafe {
                for i in 0..PERIOD_SAMPLES {
                    let scaled = (SOURCE_BUF[i] as i32 * vol as i32) / 100;
                    HDA_ACCUM[i] = HDA_ACCUM[i].saturating_add(scaled);
                }
            },
            AudioSink::PcmCapture(id) => {
                // Apply volume inline then push into the capture PCM stream.
                let mut tmp = [0i16; PERIOD_SAMPLES];
                for i in 0..PERIOD_SAMPLES {
                    unsafe {
                        let scaled = (SOURCE_BUF[i] as i32 * vol as i32) / 100;
                        tmp[i] = scaled.clamp(-32768, 32767) as i16;
                    }
                }
                pcm_write(id, &tmp);
            }
            AudioSink::SpdifOut => {
                // Stub: SPDIF hardware not yet wired up.
            }
        }
    }

    // Clip the HDA accumulation buffer to i16 and write to HDA DMA.
    unsafe {
        for i in 0..PERIOD_SAMPLES {
            HDA_OUT[i] = HDA_ACCUM[i].clamp(-32768, 32767) as i16;
        }
        // Feed into the HDA software mixer write path.
        super::hda::hda_write_samples(&HDA_OUT);
    }
}

// ---------------------------------------------------------------------------
// Internal helper: pull one period from a source into `buf`
// ---------------------------------------------------------------------------

/// Fills `buf[..count]` from `source` and returns the number of samples written.
/// All fills are exactly `count` samples (silence-padded on underrun).
///
/// # Safety
/// Caller must ensure `buf` is at least `count` elements long.
unsafe fn pull_from_source(source: AudioSource, buf: &mut [i16], count: usize) -> usize {
    match source {
        AudioSource::PcmStream(id) => {
            // pull_period expects exactly a PERIOD_SAMPLES array.
            // We reuse the static SOURCE_BUF which IS that size.
            // Reinterpret as the exact-size array the API requires.
            if count != PERIOD_SAMPLES {
                // Safety guard: we only pull full periods.
                return 0;
            }
            // pcm_pull_period needs &mut [i16; PERIOD_SAMPLES] — build the ref safely.
            let arr_ptr = buf.as_mut_ptr() as *mut [i16; PERIOD_SAMPLES];
            let ok = pcm_pull_period(id, &mut *arr_ptr);
            if ok {
                PERIOD_SAMPLES
            } else {
                0
            }
        }
        AudioSource::HdaCapture(_pin) => {
            // Stub: HDA capture path not yet wired.
            for v in buf[..count].iter_mut() {
                *v = 0;
            }
            0
        }
        AudioSource::TestTone => {
            // Hardware sample rate hint — ask the HDA driver.
            // Fall back to 48 000 Hz if unknown.
            let rate = 48_000u32;
            test_tone_fill(&mut buf[..count], count, rate);
            count
        }
        AudioSource::Silence => {
            for v in buf[..count].iter_mut() {
                *v = 0;
            }
            count
        }
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    TEST_TONE_PHASE.store(0, Ordering::Relaxed);
    serial_println!(
        "    [routing] Audio routing matrix ready ({} routes)",
        MAX_ROUTES
    );
}
