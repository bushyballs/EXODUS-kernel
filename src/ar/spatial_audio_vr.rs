use crate::sync::Mutex;
use alloc::vec;
/// VR spatial audio for Genesis AR/VR
///
/// Ambisonics encoding/decoding, HRTF binaural rendering,
/// room simulation, sound source tracking, reverb zones,
/// distance attenuation, Doppler effect.
///
/// All positions in millimeters. Q16 fixed-point for gains and coefficients.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

const Q16_ONE: i32 = 65536;
const MAX_SOURCES: usize = 32;
const MAX_REVERB_ZONES: usize = 8;
const AMBISONIC_CHANNELS: usize = 4; // First-order ambisonics (W, X, Y, Z)

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum AudioRolloff {
    Linear,
    InverseDistance,
    InverseDistanceSquared,
    Logarithmic,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ReverbPreset {
    None,
    SmallRoom,
    MediumRoom,
    LargeHall,
    Cathedral,
    Outdoor,
    Cave,
    Custom,
}

#[derive(Clone, Copy)]
pub struct AudioPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Clone, Copy)]
pub struct ListenerState {
    pub position: AudioPosition,
    pub forward_x: i32, // Q16 direction
    pub forward_y: i32,
    pub forward_z: i32,
    pub up_x: i32, // Q16 up vector
    pub up_y: i32,
    pub up_z: i32,
}

#[derive(Clone, Copy)]
pub struct HrtfCoefficients {
    pub left_delay_samples: u16,
    pub right_delay_samples: u16,
    pub left_gain_q16: i32,
    pub right_gain_q16: i32,
    pub left_filter: [i32; 4], // Q16 FIR taps
    pub right_filter: [i32; 4],
}

#[derive(Clone, Copy)]
struct AmbisonicGains {
    w: i32, // Q16
    x: i32,
    y: i32,
    z: i32,
}

#[derive(Clone, Copy)]
pub struct SoundSource {
    pub id: u32,
    pub position: AudioPosition,
    pub prev_position: AudioPosition,
    pub gain_q16: i32,
    pub rolloff: AudioRolloff,
    pub min_distance_mm: u32,
    pub max_distance_mm: u32,
    pub doppler_factor_q16: i32,
    pub spatial: bool,
    pub active: bool,
    pub looping: bool,
    pub priority: u8,
}

#[derive(Clone, Copy)]
pub struct ReverbZone {
    pub id: u32,
    pub center: AudioPosition,
    pub radius_mm: u32,
    pub preset: ReverbPreset,
    pub wet_q16: i32,   // wet/dry mix
    pub decay_q16: i32, // decay time factor
    pub diffusion_q16: i32,
    pub pre_delay_ms: u16,
    pub active: bool,
}

#[derive(Clone, Copy)]
struct RoomProperties {
    width_mm: u32,
    height_mm: u32,
    depth_mm: u32,
    absorption_q16: i32, // wall absorption coefficient
    reflection_count: u8,
}

// ---------------------------------------------------------------------------
// HRTF lookup table (simplified elevation/azimuth grid)
// ---------------------------------------------------------------------------

struct HrtfTable {
    // Azimuth: 0..360 in 15-degree steps = 24 entries
    // Elevation: -90..90 in 30-degree steps = 7 entries
    entries: [[HrtfCoefficients; 24]; 7],
}

impl HrtfTable {
    fn default_table() -> Self {
        let default_coeff = HrtfCoefficients {
            left_delay_samples: 0,
            right_delay_samples: 0,
            left_gain_q16: Q16_ONE,
            right_gain_q16: Q16_ONE,
            left_filter: [Q16_ONE, 0, 0, 0],
            right_filter: [Q16_ONE, 0, 0, 0],
        };
        let mut table = HrtfTable {
            entries: [[default_coeff; 24]; 7],
        };

        // Populate with simplified HRTF model based on interaural time/level differences
        for elev_idx in 0..7_usize {
            let elevation = (elev_idx as i32) * 30 - 90; // -90 to +90
            for az_idx in 0..24_usize {
                let azimuth = (az_idx as i32) * 15; // 0 to 345

                // ITD: interaural time difference based on azimuth
                // sin(azimuth) approximated with Q16
                let sin_az = sin_q16(azimuth);
                let itd = (((sin_az as i64) * 22) / Q16_ONE as i64) as i32; // ~22 samples max at 44100Hz

                let (l_delay, r_delay) = if itd >= 0 {
                    (0u16, itd as u16)
                } else {
                    ((-itd) as u16, 0u16)
                };

                // ILD: interaural level difference
                let cos_az = cos_q16(azimuth);
                let left_gain = (((Q16_ONE as i64 + cos_az as i64) * Q16_ONE as i64)
                    / (2 * Q16_ONE as i64)) as i32;
                let right_gain = (((Q16_ONE as i64 - cos_az as i64) * Q16_ONE as i64)
                    / (2 * Q16_ONE as i64)) as i32;

                // Elevation affects high-frequency filter
                let elev_factor = (((elevation.abs() as i64) * Q16_ONE as i64) / 90) as i32;
                let hf_atten =
                    Q16_ONE - (((elev_factor as i64) * Q16_ONE as i64 / 4) / Q16_ONE as i64) as i32;

                table.entries[elev_idx][az_idx] = HrtfCoefficients {
                    left_delay_samples: l_delay,
                    right_delay_samples: r_delay,
                    left_gain_q16: left_gain.max(Q16_ONE / 8),
                    right_gain_q16: right_gain.max(Q16_ONE / 8),
                    left_filter: [hf_atten, 0, 0, 0],
                    right_filter: [hf_atten, 0, 0, 0],
                };
            }
        }
        table
    }

    fn lookup(&self, azimuth_deg: i32, elevation_deg: i32) -> HrtfCoefficients {
        let az = ((azimuth_deg % 360) + 360) % 360;
        let az_idx = ((az as usize) / 15).min(23);

        let elev_clamped = elevation_deg.max(-90).min(90);
        let elev_idx = (((elev_clamped + 90) as usize) / 30).min(6);

        self.entries[elev_idx][az_idx]
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct SpatialAudioEngine {
    listener: ListenerState,
    sources: Vec<SoundSource>,
    reverb_zones: Vec<ReverbZone>,
    hrtf: HrtfTable,
    room: RoomProperties,
    master_gain_q16: i32,
    speed_of_sound_mm_s: i32, // ~343000 mm/s
    sample_rate: u32,
    next_id: u32,
    enabled: bool,
    ambisonic_output: [i32; AMBISONIC_CHANNELS], // Q16 bus
}

static SPATIAL_AUDIO: Mutex<Option<SpatialAudioEngine>> = Mutex::new(None);

impl SpatialAudioEngine {
    fn new() -> Self {
        SpatialAudioEngine {
            listener: ListenerState {
                position: AudioPosition { x: 0, y: 0, z: 0 },
                forward_x: 0,
                forward_y: 0,
                forward_z: Q16_ONE,
                up_x: 0,
                up_y: Q16_ONE,
                up_z: 0,
            },
            sources: Vec::new(),
            reverb_zones: Vec::new(),
            hrtf: HrtfTable::default_table(),
            room: RoomProperties {
                width_mm: 5000,
                height_mm: 3000,
                depth_mm: 5000,
                absorption_q16: Q16_ONE / 2,
                reflection_count: 4,
            },
            master_gain_q16: Q16_ONE,
            speed_of_sound_mm_s: 343000,
            sample_rate: 48000,
            next_id: 1,
            enabled: true,
            ambisonic_output: [0; AMBISONIC_CHANNELS],
        }
    }

    /// Add a spatial sound source
    fn add_source(&mut self, pos: AudioPosition, rolloff: AudioRolloff) -> Option<u32> {
        if self.sources.len() >= MAX_SOURCES {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.sources.push(SoundSource {
            id,
            position: pos,
            prev_position: pos,
            gain_q16: Q16_ONE,
            rolloff,
            min_distance_mm: 100,
            max_distance_mm: 50000,
            doppler_factor_q16: Q16_ONE,
            spatial: true,
            active: true,
            looping: false,
            priority: 128,
        });
        Some(id)
    }

    /// Add a reverb zone
    fn add_reverb_zone(
        &mut self,
        center: AudioPosition,
        radius: u32,
        preset: ReverbPreset,
    ) -> Option<u32> {
        if self.reverb_zones.len() >= MAX_REVERB_ZONES {
            return None;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let (wet, decay, diffusion, pre_delay) = match preset {
            ReverbPreset::None => (0, 0, 0, 0),
            ReverbPreset::SmallRoom => (Q16_ONE / 4, Q16_ONE / 2, Q16_ONE * 3 / 4, 5),
            ReverbPreset::MediumRoom => (Q16_ONE / 3, Q16_ONE * 2 / 3, Q16_ONE / 2, 15),
            ReverbPreset::LargeHall => (Q16_ONE / 2, Q16_ONE * 4 / 5, Q16_ONE / 3, 30),
            ReverbPreset::Cathedral => (Q16_ONE * 2 / 3, Q16_ONE * 9 / 10, Q16_ONE / 4, 50),
            ReverbPreset::Outdoor => (Q16_ONE / 8, Q16_ONE / 4, Q16_ONE, 2),
            ReverbPreset::Cave => (Q16_ONE * 3 / 4, Q16_ONE, Q16_ONE / 5, 40),
            ReverbPreset::Custom => (Q16_ONE / 3, Q16_ONE / 2, Q16_ONE / 2, 20),
        };

        self.reverb_zones.push(ReverbZone {
            id,
            center,
            radius_mm: radius,
            preset,
            wet_q16: wet,
            decay_q16: decay,
            diffusion_q16: diffusion,
            pre_delay_ms: pre_delay,
            active: true,
        });
        Some(id)
    }

    /// Compute distance between listener and source in mm
    fn source_distance(&self, src: &SoundSource) -> i64 {
        let dx = (src.position.x - self.listener.position.x) as i64;
        let dy = (src.position.y - self.listener.position.y) as i64;
        let dz = (src.position.z - self.listener.position.z) as i64;
        isqrt_i64(dx * dx + dy * dy + dz * dz)
    }

    /// Compute attenuation based on rolloff model
    fn compute_attenuation(&self, src: &SoundSource) -> i32 {
        let dist = self.source_distance(src);
        let min_d = src.min_distance_mm as i64;
        let max_d = src.max_distance_mm as i64;

        if dist <= min_d {
            return Q16_ONE;
        }
        if dist >= max_d {
            return 0;
        }

        let range = max_d - min_d;
        if range == 0 {
            return Q16_ONE;
        }
        let d = dist - min_d;

        match src.rolloff {
            AudioRolloff::Linear => (((range - d) * Q16_ONE as i64) / range) as i32,
            AudioRolloff::InverseDistance => ((min_d * Q16_ONE as i64) / dist) as i32,
            AudioRolloff::InverseDistanceSquared => {
                ((min_d * min_d * Q16_ONE as i64) / (dist * dist).max(1)) as i32
            }
            AudioRolloff::Logarithmic => {
                // Approximate log attenuation: 1 - log(d/min) / log(max/min)
                // Use linear approximation of log ratio
                let ratio = ((d * Q16_ONE as i64) / range) as i32;
                (Q16_ONE - ratio).max(0)
            }
        }
    }

    /// Compute azimuth and elevation from listener to source (degrees)
    fn compute_direction(&self, src: &SoundSource) -> (i32, i32) {
        let dx = (src.position.x - self.listener.position.x) as i64;
        let dy = (src.position.y - self.listener.position.y) as i64;
        let dz = (src.position.z - self.listener.position.z) as i64;

        let horiz_dist = isqrt_i64(dx * dx + dz * dz);

        // Azimuth: atan2(dx, dz) approximation in degrees
        let azimuth = if horiz_dist > 0 { atan2_deg(dx, dz) } else { 0 };

        // Elevation: atan2(dy, horiz_dist) approximation
        let elevation = if horiz_dist > 0 {
            atan2_deg(dy, horiz_dist)
        } else if dy > 0 {
            90
        } else {
            -90
        };

        (azimuth, elevation)
    }

    /// Compute Doppler pitch shift factor (Q16)
    fn compute_doppler(&self, src: &SoundSource) -> i32 {
        if src.doppler_factor_q16 == 0 {
            return Q16_ONE;
        }
        let speed = self.speed_of_sound_mm_s as i64;
        // Radial velocity: change in distance per frame
        let prev_dist = {
            let dx = (src.prev_position.x - self.listener.position.x) as i64;
            let dy = (src.prev_position.y - self.listener.position.y) as i64;
            let dz = (src.prev_position.z - self.listener.position.z) as i64;
            isqrt_i64(dx * dx + dy * dy + dz * dz)
        };
        let curr_dist = self.source_distance(src);
        let radial_vel = (curr_dist - prev_dist) * 60; // approximate mm/s at 60fps

        // Doppler ratio: speed / (speed + radial_vel)
        let denom = speed + ((radial_vel * src.doppler_factor_q16 as i64) / Q16_ONE as i64);
        if denom <= 0 {
            return Q16_ONE * 2; // cap at 2x pitch
        }
        (((speed * Q16_ONE as i64) / denom) as i32)
            .max(Q16_ONE / 4)
            .min(Q16_ONE * 4)
    }

    /// Encode a source into first-order ambisonics gains
    fn encode_ambisonic(&self, src: &SoundSource) -> AmbisonicGains {
        let dist = self.source_distance(src);
        if dist == 0 {
            return AmbisonicGains {
                w: Q16_ONE,
                x: 0,
                y: 0,
                z: 0,
            };
        }
        let dx = (src.position.x - self.listener.position.x) as i64;
        let dy = (src.position.y - self.listener.position.y) as i64;
        let dz = (src.position.z - self.listener.position.z) as i64;

        // Normalize direction and apply attenuation
        let atten = self.compute_attenuation(src) as i64;
        let w = ((atten * 46341) / Q16_ONE as i64) as i32; // 1/sqrt(2) ~= 0.707 ~= 46341 in Q16
        let x_gain = ((dx * atten) / dist) as i32;
        let y_gain = ((dy * atten) / dist) as i32;
        let z_gain = ((dz * atten) / dist) as i32;

        AmbisonicGains {
            w,
            x: x_gain,
            y: y_gain,
            z: z_gain,
        }
    }

    /// Find active reverb zone for listener position
    fn active_reverb(&self) -> Option<&ReverbZone> {
        let lx = self.listener.position.x as i64;
        let ly = self.listener.position.y as i64;
        let lz = self.listener.position.z as i64;

        let mut best: Option<&ReverbZone> = None;
        let mut best_dist = i64::MAX;

        for zone in &self.reverb_zones {
            if !zone.active {
                continue;
            }
            let dx = lx - zone.center.x as i64;
            let dy = ly - zone.center.y as i64;
            let dz = lz - zone.center.z as i64;
            let dist = isqrt_i64(dx * dx + dy * dy + dz * dz);
            if dist < zone.radius_mm as i64 && dist < best_dist {
                best_dist = dist;
                best = Some(zone);
            }
        }
        best
    }

    /// Process all sources for one audio frame
    fn process_frame(&mut self) {
        if !self.enabled {
            return;
        }

        // Clear ambisonic bus
        self.ambisonic_output = [0; AMBISONIC_CHANNELS];

        for i in 0..self.sources.len() {
            if !self.sources[i].active {
                continue;
            }
            let gains = self.encode_ambisonic(&self.sources[i]);
            self.ambisonic_output[0] = self.ambisonic_output[0].saturating_add(gains.w);
            self.ambisonic_output[1] = self.ambisonic_output[1].saturating_add(gains.x);
            self.ambisonic_output[2] = self.ambisonic_output[2].saturating_add(gains.y);
            self.ambisonic_output[3] = self.ambisonic_output[3].saturating_add(gains.z);
        }

        // Apply master gain
        for ch in &mut self.ambisonic_output {
            *ch = (((*ch as i64) * self.master_gain_q16 as i64) / Q16_ONE as i64) as i32;
        }
    }

    /// Remove a source by ID
    fn remove_source(&mut self, id: u32) -> bool {
        if let Some(pos) = self.sources.iter().position(|s| s.id == id) {
            self.sources.remove(pos);
            true
        } else {
            false
        }
    }

    fn source_count(&self) -> usize {
        self.sources.iter().filter(|s| s.active).count()
    }
}

// ---------------------------------------------------------------------------
// Math utilities
// ---------------------------------------------------------------------------

fn isqrt_i64(val: i64) -> i64 {
    if val <= 0 {
        return 0;
    }
    let v = val as u64;
    let mut x = v;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + v / x) / 2;
    }
    x as i64
}

/// Approximate sin in Q16 for angle in degrees
fn sin_q16(deg: i32) -> i32 {
    let d = ((deg % 360) + 360) % 360;
    // Simple piecewise linear approximation
    let val = if d <= 90 {
        (((d as i64) * Q16_ONE as i64) / 90) as i32
    } else if d <= 180 {
        ((((180 - d) as i64) * Q16_ONE as i64) / 90) as i32
    } else if d <= 270 {
        -((((d - 180) as i64) * Q16_ONE as i64) / 90) as i32
    } else {
        -((((360 - d) as i64) * Q16_ONE as i64) / 90) as i32
    };
    val
}

/// Approximate cos in Q16 for angle in degrees
fn cos_q16(deg: i32) -> i32 {
    sin_q16(deg + 90)
}

/// Approximate atan2 returning degrees
fn atan2_deg(y: i64, x: i64) -> i32 {
    if x == 0 && y == 0 {
        return 0;
    }
    let abs_y = if y < 0 { -y } else { y };
    let abs_x = if x < 0 { -x } else { x };

    // Octant-based approximation
    let angle = if abs_x >= abs_y {
        ((abs_y * 45) / abs_x.max(1)) as i32
    } else {
        90 - ((abs_x * 45) / abs_y.max(1)) as i32
    };

    let angle = if x < 0 { 180 - angle } else { angle };
    let angle = if y < 0 { -angle } else { angle };
    ((angle % 360) + 360) % 360
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn update_listener(pos: AudioPosition, fwd_x: i32, fwd_y: i32, fwd_z: i32) {
    let mut s = SPATIAL_AUDIO.lock();
    if let Some(e) = s.as_mut() {
        e.listener.position = pos;
        e.listener.forward_x = fwd_x;
        e.listener.forward_y = fwd_y;
        e.listener.forward_z = fwd_z;
    }
}

pub fn add_source(pos: AudioPosition, rolloff: AudioRolloff) -> Option<u32> {
    let mut s = SPATIAL_AUDIO.lock();
    s.as_mut().and_then(|e| e.add_source(pos, rolloff))
}

pub fn remove_source(id: u32) -> bool {
    let mut s = SPATIAL_AUDIO.lock();
    s.as_mut().map_or(false, |e| e.remove_source(id))
}

pub fn add_reverb_zone(center: AudioPosition, radius: u32, preset: ReverbPreset) -> Option<u32> {
    let mut s = SPATIAL_AUDIO.lock();
    s.as_mut()
        .and_then(|e| e.add_reverb_zone(center, radius, preset))
}

pub fn process_frame() {
    let mut s = SPATIAL_AUDIO.lock();
    if let Some(e) = s.as_mut() {
        e.process_frame();
    }
}

pub fn source_count() -> usize {
    let s = SPATIAL_AUDIO.lock();
    s.as_ref().map_or(0, |e| e.source_count())
}

pub fn init() {
    let mut s = SPATIAL_AUDIO.lock();
    *s = Some(SpatialAudioEngine::new());
    serial_println!("    AR: VR spatial audio (ambisonics, HRTF, reverb) ready");
}
