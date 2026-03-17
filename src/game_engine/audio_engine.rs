/// Game Audio Engine for Genesis
///
/// Sound effect and music playback with 3D positional audio,
/// crossfading, volume ducking, and bus-based mixing. All volume
/// and distance values use i32 Q16 fixed-point (65536 = 1.0).
/// This is a software mixer that generates sample buffers for
/// the hardware audio driver.

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constants
const Q16_ONE: i32 = 65536;
const Q16_HALF: i32 = 32768;
const Q16_ZERO: i32 = 0;

/// Maximum number of concurrent sound channels.
const MAX_CHANNELS: usize = 32;

/// Maximum number of audio buses.
const MAX_BUSES: usize = 8;

/// Maximum number of registered sound assets.
const MAX_SOUNDS: usize = 128;

/// Maximum listener distance for 3D audio falloff (Q16).
const MAX_AUDIO_DISTANCE: i32 = 655360;  // 10.0 in Q16

/// Default crossfade duration in ticks.
const DEFAULT_CROSSFADE_TICKS: u32 = 120;

/// Q16 multiply: (a * b) >> 16, using i64 to prevent overflow.
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide: (a << 16) / b, using i64 to prevent overflow.
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << 16) / (b as i64)) as i32
}

/// Integer square root using Newton's method.
fn isqrt(value: i32) -> i32 {
    if value <= 0 { return 0; }
    let val = value as u32;
    let mut guess = val;
    let mut last: u32;
    loop {
        last = guess;
        if guess == 0 { return 0; }
        guess = (guess + val / guess) / 2;
        if guess >= last { break; }
    }
    last as i32
}

/// Sound playback state.
#[derive(Clone, Copy, PartialEq)]
pub enum ChannelState {
    Idle,
    Playing,
    Paused,
    FadingIn,
    FadingOut,
    Stopping,
}

/// Sound asset descriptor registered with the engine.
#[derive(Clone, Copy)]
pub struct SoundAsset {
    pub hash: u64,
    pub sample_rate: u32,
    pub sample_count: u32,
    pub channels: u8,         // 1=mono, 2=stereo
    pub loopable: bool,
    pub active: bool,
}

impl SoundAsset {
    fn empty() -> Self {
        SoundAsset {
            hash: 0,
            sample_rate: 44100,
            sample_count: 0,
            channels: 1,
            loopable: false,
            active: false,
        }
    }
}

/// A playing sound channel with position, volume, and fade state.
#[derive(Clone, Copy)]
pub struct AudioChannel {
    pub id: u32,
    pub sound_hash: u64,
    pub state: ChannelState,
    pub volume: i32,          // Q16 [0..Q16_ONE]
    pub target_volume: i32,
    pub pan: i32,             // Q16 [-Q16_ONE..Q16_ONE] (-1=left, 1=right)
    pub pitch: i32,           // Q16 playback rate (Q16_ONE = normal)
    pub position: u32,        // current sample position
    pub bus_id: u32,
    pub looping: bool,
    pub fade_speed: i32,      // Q16 volume change per tick
    pub priority: i32,
    // 3D positional audio
    pub pos_x: i32,           // Q16 world position
    pub pos_y: i32,
    pub spatial: bool,
    pub min_distance: i32,    // Q16 distance for full volume
    pub max_distance: i32,    // Q16 distance for zero volume
    pub computed_volume: i32, // Q16 final volume after spatial + bus
    pub computed_pan: i32,    // Q16 final pan after spatial
}

impl AudioChannel {
    fn empty() -> Self {
        AudioChannel {
            id: 0,
            sound_hash: 0,
            state: ChannelState::Idle,
            volume: Q16_ONE,
            target_volume: Q16_ONE,
            pan: Q16_ZERO,
            pitch: Q16_ONE,
            position: 0,
            bus_id: 0,
            looping: false,
            fade_speed: Q16_ZERO,
            priority: 0,
            pos_x: Q16_ZERO,
            pos_y: Q16_ZERO,
            spatial: false,
            min_distance: Q16_ONE,
            max_distance: MAX_AUDIO_DISTANCE,
            computed_volume: Q16_ONE,
            computed_pan: Q16_ZERO,
        }
    }
}

/// An audio bus groups channels for collective volume/mute control.
#[derive(Clone, Copy)]
pub struct AudioBus {
    pub id: u32,
    pub volume: i32,          // Q16 master volume
    pub muted: bool,
    pub duck_volume: i32,     // Q16 ducking multiplier (Q16_ONE = no duck)
    pub duck_target: i32,
    pub duck_speed: i32,      // Q16 duck ramp speed per tick
    pub active: bool,
}

impl AudioBus {
    fn new(id: u32) -> Self {
        AudioBus {
            id,
            volume: Q16_ONE,
            muted: false,
            duck_volume: Q16_ONE,
            duck_target: Q16_ONE,
            duck_speed: 3277,    // ~0.05 per tick
            active: true,
        }
    }
}

/// Music crossfade state for transitioning between two music tracks.
#[derive(Clone, Copy)]
struct MusicCrossfade {
    outgoing_channel: u32,
    incoming_channel: u32,
    progress: i32,           // Q16 [0..Q16_ONE]
    speed: i32,              // Q16 per tick
    active: bool,
}

impl MusicCrossfade {
    fn empty() -> Self {
        MusicCrossfade {
            outgoing_channel: 0,
            incoming_channel: 0,
            progress: Q16_ZERO,
            speed: Q16_ZERO,
            active: false,
        }
    }
}

/// 3D audio listener position and orientation.
#[derive(Clone, Copy)]
pub struct AudioListener {
    pub x: i32,              // Q16 position
    pub y: i32,
    pub facing_x: i32,      // Q16 normalized direction
    pub facing_y: i32,
}

impl AudioListener {
    fn new() -> Self {
        AudioListener {
            x: Q16_ZERO,
            y: Q16_ZERO,
            facing_x: Q16_ONE,
            facing_y: Q16_ZERO,
        }
    }
}

/// The audio engine manages all channels, buses, and mixing.
struct AudioEngine {
    channels: Vec<AudioChannel>,
    buses: Vec<AudioBus>,
    sounds: Vec<SoundAsset>,
    listener: AudioListener,
    crossfade: MusicCrossfade,
    next_channel_id: u32,
    master_volume: i32,       // Q16
    music_channel: u32,       // current music channel id
    enabled: bool,
}

static AUDIO: Mutex<Option<AudioEngine>> = Mutex::new(None);

impl AudioEngine {
    fn new() -> Self {
        let mut buses = Vec::new();
        // Bus 0 = SFX, Bus 1 = Music, Bus 2 = Voice, Bus 3 = Ambient
        for i in 0..4u32 {
            buses.push(AudioBus::new(i));
        }

        AudioEngine {
            channels: Vec::new(),
            buses,
            sounds: Vec::new(),
            listener: AudioListener::new(),
            crossfade: MusicCrossfade::empty(),
            next_channel_id: 1,
            master_volume: Q16_ONE,
            music_channel: 0,
            enabled: true,
        }
    }

    /// Register a sound asset.
    fn register_sound(&mut self, hash: u64, sample_rate: u32, sample_count: u32,
                      channels: u8, loopable: bool) -> bool {
        if self.sounds.len() >= MAX_SOUNDS {
            serial_println!("    Audio: max sounds reached ({})", MAX_SOUNDS);
            return false;
        }
        let mut asset = SoundAsset::empty();
        asset.hash = hash;
        asset.sample_rate = sample_rate;
        asset.sample_count = sample_count;
        asset.channels = channels;
        asset.loopable = loopable;
        asset.active = true;
        self.sounds.push(asset);
        true
    }

    /// Find a free or lowest-priority channel for a new sound.
    fn find_channel(&self, priority: i32) -> Option<usize> {
        // First look for an idle channel
        for (i, ch) in self.channels.iter().enumerate() {
            if ch.state == ChannelState::Idle {
                return Some(i);
            }
        }
        // If we can allocate more channels, signal that
        if self.channels.len() < MAX_CHANNELS {
            return None; // caller will push a new one
        }
        // Steal lowest priority
        let mut lowest_idx = 0usize;
        let mut lowest_pri = i32::MAX;
        for (i, ch) in self.channels.iter().enumerate() {
            if ch.priority < lowest_pri {
                lowest_pri = ch.priority;
                lowest_idx = i;
            }
        }
        if priority >= lowest_pri {
            Some(lowest_idx)
        } else {
            None
        }
    }

    /// Play a sound effect. Returns the channel id.
    fn play_sound(&mut self, sound_hash: u64, volume: i32, pan: i32,
                  priority: i32, bus_id: u32, looping: bool) -> u32 {
        if !self.enabled { return 0; }

        let id = self.next_channel_id;
        self.next_channel_id = self.next_channel_id.saturating_add(1);

        let channel_slot = self.find_channel(priority);
        let mut ch = AudioChannel::empty();
        ch.id = id;
        ch.sound_hash = sound_hash;
        ch.state = ChannelState::Playing;
        ch.volume = volume;
        ch.target_volume = volume;
        ch.pan = pan;
        ch.bus_id = bus_id;
        ch.looping = looping;
        ch.priority = priority;

        match channel_slot {
            Some(idx) => {
                self.channels[idx] = ch;
            }
            None => {
                if self.channels.len() < MAX_CHANNELS {
                    self.channels.push(ch);
                } else {
                    return 0;
                }
            }
        }
        id
    }

    /// Play a sound with 3D spatial positioning.
    fn play_sound_3d(&mut self, sound_hash: u64, x: i32, y: i32,
                     volume: i32, priority: i32, bus_id: u32) -> u32 {
        let id = self.play_sound(sound_hash, volume, Q16_ZERO, priority, bus_id, false);
        if id == 0 { return 0; }
        // Find the channel and mark it spatial
        for ch in self.channels.iter_mut() {
            if ch.id == id {
                ch.pos_x = x;
                ch.pos_y = y;
                ch.spatial = true;
                break;
            }
        }
        id
    }

    /// Play music with optional crossfade from current track.
    fn play_music(&mut self, sound_hash: u64, crossfade_ticks: u32) -> u32 {
        let new_id = self.play_sound(sound_hash, Q16_ONE, Q16_ZERO, 100, 1, true);
        if new_id == 0 { return 0; }

        if self.music_channel != 0 && crossfade_ticks > 0 {
            // Start crossfade
            let speed = if crossfade_ticks > 0 {
                q16_div(Q16_ONE, crossfade_ticks as i32 * Q16_ONE / Q16_ONE)
            } else {
                Q16_ONE
            };
            // Set incoming channel to start at zero volume
            for ch in self.channels.iter_mut() {
                if ch.id == new_id {
                    ch.volume = Q16_ZERO;
                    ch.target_volume = Q16_ONE;
                    ch.state = ChannelState::FadingIn;
                    ch.fade_speed = speed;
                    break;
                }
            }
            // Set outgoing channel to fade out
            for ch in self.channels.iter_mut() {
                if ch.id == self.music_channel {
                    ch.target_volume = Q16_ZERO;
                    ch.state = ChannelState::FadingOut;
                    ch.fade_speed = speed;
                    break;
                }
            }
            self.crossfade = MusicCrossfade {
                outgoing_channel: self.music_channel,
                incoming_channel: new_id,
                progress: Q16_ZERO,
                speed,
                active: true,
            };
        }
        self.music_channel = new_id;
        new_id
    }

    /// Stop a channel, optionally with a fade-out.
    fn stop_channel(&mut self, channel_id: u32, fade_ticks: u32) {
        for ch in self.channels.iter_mut() {
            if ch.id == channel_id {
                if fade_ticks > 0 {
                    let speed = q16_div(Q16_ONE, fade_ticks as i32 * Q16_ONE / Q16_ONE);
                    ch.target_volume = Q16_ZERO;
                    ch.fade_speed = speed;
                    ch.state = ChannelState::FadingOut;
                } else {
                    ch.state = ChannelState::Idle;
                    ch.position = 0;
                }
                break;
            }
        }
    }

    /// Set the bus volume.
    fn set_bus_volume(&mut self, bus_id: u32, volume: i32) {
        for bus in self.buses.iter_mut() {
            if bus.id == bus_id && bus.active {
                bus.volume = volume;
                break;
            }
        }
    }

    /// Mute or unmute a bus.
    fn set_bus_muted(&mut self, bus_id: u32, muted: bool) {
        for bus in self.buses.iter_mut() {
            if bus.id == bus_id && bus.active {
                bus.muted = muted;
                break;
            }
        }
    }

    /// Apply volume ducking to a bus (e.g., lower music when voice plays).
    fn duck_bus(&mut self, bus_id: u32, target: i32, speed: i32) {
        for bus in self.buses.iter_mut() {
            if bus.id == bus_id && bus.active {
                bus.duck_target = target;
                bus.duck_speed = speed;
                break;
            }
        }
    }

    /// Set the 3D listener position and facing direction.
    fn set_listener(&mut self, x: i32, y: i32, facing_x: i32, facing_y: i32) {
        self.listener.x = x;
        self.listener.y = y;
        self.listener.facing_x = facing_x;
        self.listener.facing_y = facing_y;
    }

    /// Compute 3D spatial volume and pan for a channel.
    fn compute_spatial(&self, ch: &AudioChannel) -> (i32, i32) {
        let dx = ch.pos_x - self.listener.x;
        let dy = ch.pos_y - self.listener.y;
        let dist_sq = q16_mul(dx, dx) + q16_mul(dy, dy);

        // Compute distance in Q16
        let dist = if dist_sq < 0x7FFF {
            isqrt(dist_sq << 16)
        } else {
            isqrt(dist_sq) << 8
        };

        // Volume attenuation based on distance
        let volume = if dist <= ch.min_distance {
            Q16_ONE
        } else if dist >= ch.max_distance {
            Q16_ZERO
        } else {
            let range = ch.max_distance - ch.min_distance;
            if range <= 0 { Q16_ONE }
            else {
                let factor = q16_div(ch.max_distance - dist, range);
                // Quadratic falloff for more natural sound
                q16_mul(factor, factor)
            }
        };

        // Compute stereo pan from angle relative to listener facing
        // Cross product gives left/right side: facing_x * dy - facing_y * dx
        let cross = q16_mul(self.listener.facing_x, dy)
                  - q16_mul(self.listener.facing_y, dx);
        // Normalize to [-Q16_ONE, Q16_ONE]
        let pan = if dist > 0 {
            let raw_pan = q16_div(cross, dist);
            if raw_pan > Q16_ONE { Q16_ONE }
            else if raw_pan < -Q16_ONE { -Q16_ONE }
            else { raw_pan }
        } else {
            Q16_ZERO
        };

        (volume, pan)
    }

    /// Per-frame update: advance fades, ducking, spatial computations.
    fn update(&mut self) {
        if !self.enabled { return; }

        // Update bus ducking
        for bus in self.buses.iter_mut() {
            if !bus.active { continue; }
            if bus.duck_volume != bus.duck_target {
                if bus.duck_volume < bus.duck_target {
                    bus.duck_volume += bus.duck_speed;
                    if bus.duck_volume > bus.duck_target {
                        bus.duck_volume = bus.duck_target;
                    }
                } else {
                    bus.duck_volume -= bus.duck_speed;
                    if bus.duck_volume < bus.duck_target {
                        bus.duck_volume = bus.duck_target;
                    }
                }
            }
        }

        // Update crossfade
        if self.crossfade.active {
            self.crossfade.progress += self.crossfade.speed;
            if self.crossfade.progress >= Q16_ONE {
                self.crossfade.active = false;
                // Stop outgoing channel
                for ch in self.channels.iter_mut() {
                    if ch.id == self.crossfade.outgoing_channel {
                        ch.state = ChannelState::Idle;
                        break;
                    }
                }
            }
        }

        // Update channels
        for ch in self.channels.iter_mut() {
            match ch.state {
                ChannelState::FadingIn => {
                    ch.volume += ch.fade_speed;
                    if ch.volume >= ch.target_volume {
                        ch.volume = ch.target_volume;
                        ch.state = ChannelState::Playing;
                    }
                }
                ChannelState::FadingOut => {
                    ch.volume -= ch.fade_speed;
                    if ch.volume <= Q16_ZERO {
                        ch.volume = Q16_ZERO;
                        ch.state = ChannelState::Idle;
                    }
                }
                ChannelState::Playing => {
                    // Spatial audio computation
                    if ch.spatial {
                        let (vol, pan) = self.compute_spatial(ch);
                        ch.computed_volume = q16_mul(ch.volume, vol);
                        ch.computed_pan = pan;
                    } else {
                        ch.computed_volume = ch.volume;
                        ch.computed_pan = ch.pan;
                    }

                    // Apply bus volume and ducking
                    let bus_vol = self.get_bus_volume(ch.bus_id);
                    ch.computed_volume = q16_mul(ch.computed_volume, bus_vol);
                    ch.computed_volume = q16_mul(ch.computed_volume, self.master_volume);
                }
                _ => {}
            }
        }
    }

    /// Get effective bus volume (volume * duck, or 0 if muted).
    fn get_bus_volume(&self, bus_id: u32) -> i32 {
        for bus in self.buses.iter() {
            if bus.id == bus_id && bus.active {
                if bus.muted { return Q16_ZERO; }
                return q16_mul(bus.volume, bus.duck_volume);
            }
        }
        Q16_ONE
    }

    /// Get count of active (non-idle) channels.
    fn active_channel_count(&self) -> usize {
        self.channels.iter().filter(|c| c.state != ChannelState::Idle).count()
    }

    /// Pause a channel.
    fn pause_channel(&mut self, channel_id: u32) {
        for ch in self.channels.iter_mut() {
            if ch.id == channel_id && ch.state == ChannelState::Playing {
                ch.state = ChannelState::Paused;
                break;
            }
        }
    }

    /// Resume a paused channel.
    fn resume_channel(&mut self, channel_id: u32) {
        for ch in self.channels.iter_mut() {
            if ch.id == channel_id && ch.state == ChannelState::Paused {
                ch.state = ChannelState::Playing;
                break;
            }
        }
    }

    /// Stop all channels on a given bus.
    fn stop_bus(&mut self, bus_id: u32) {
        for ch in self.channels.iter_mut() {
            if ch.bus_id == bus_id && ch.state != ChannelState::Idle {
                ch.state = ChannelState::Idle;
                ch.position = 0;
            }
        }
    }
}

// --- Public API ---

/// Register a sound asset.
pub fn register_sound(hash: u64, sample_rate: u32, sample_count: u32,
                      channels: u8, loopable: bool) -> bool {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.register_sound(hash, sample_rate, sample_count, channels, loopable)
    } else { false }
}

/// Play a sound effect. Returns channel id.
pub fn play_sound(sound_hash: u64, volume: i32, pan: i32,
                  priority: i32, bus_id: u32, looping: bool) -> u32 {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.play_sound(sound_hash, volume, pan, priority, bus_id, looping)
    } else { 0 }
}

/// Play a 3D positional sound. Returns channel id.
pub fn play_sound_3d(sound_hash: u64, x: i32, y: i32,
                     volume: i32, priority: i32, bus_id: u32) -> u32 {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.play_sound_3d(sound_hash, x, y, volume, priority, bus_id)
    } else { 0 }
}

/// Play music with crossfade from current track.
pub fn play_music(sound_hash: u64, crossfade_ticks: u32) -> u32 {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.play_music(sound_hash, crossfade_ticks)
    } else { 0 }
}

/// Stop a channel with optional fade-out.
pub fn stop_channel(channel_id: u32, fade_ticks: u32) {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.stop_channel(channel_id, fade_ticks);
    }
}

/// Set bus volume (Q16).
pub fn set_bus_volume(bus_id: u32, volume: i32) {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.set_bus_volume(bus_id, volume);
    }
}

/// Set master volume (Q16).
pub fn set_master_volume(volume: i32) {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.master_volume = volume;
    }
}

/// Set 3D listener position and facing direction (Q16).
pub fn set_listener(x: i32, y: i32, facing_x: i32, facing_y: i32) {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.set_listener(x, y, facing_x, facing_y);
    }
}

/// Duck a bus volume (e.g., lower music when voice plays).
pub fn duck_bus(bus_id: u32, target: i32, speed: i32) {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.duck_bus(bus_id, target, speed);
    }
}

/// Update the audio engine once per frame.
pub fn update() {
    let mut audio = AUDIO.lock();
    if let Some(ref mut a) = *audio {
        a.update();
    }
}

pub fn init() {
    let mut audio = AUDIO.lock();
    *audio = Some(AudioEngine::new());
    serial_println!("    Audio engine: {} channels, {} buses, 3D spatial, crossfade, ducking",
        MAX_CHANNELS, MAX_BUSES);
}
