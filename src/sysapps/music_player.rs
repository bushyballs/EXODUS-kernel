use crate::sync::Mutex;
use alloc::vec;
/// Music library and player for Genesis OS
///
/// Full-featured music player with library scanning, playlist management,
/// playback controls (play, pause, seek, volume), shuffle and repeat
/// modes. Track metadata is stored as hashes. Volume and seek positions
/// use Q16 fixed-point arithmetic.
///
/// Inspired by: Rhythmbox, Spotify, foobar2000. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers
// ---------------------------------------------------------------------------

/// Q16 constant: 1.0 = 65536
const Q16_ONE: i32 = 65536;
/// Q16 constant: 0.5 = 32768
const Q16_HALF: i32 = 32768;
/// Q16 constant: 0.0
const Q16_ZERO: i32 = 0;
/// Maximum volume (1.0 in Q16)
const VOLUME_MAX: i32 = Q16_ONE;
/// Volume step increment (0.05 in Q16 = 3277)
const VOLUME_STEP: i32 = 3277;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Playback state
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerState {
    Playing,
    Paused,
    Stopped,
}

/// Playback mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayMode {
    Normal,
    Repeat,
    RepeatOne,
    Shuffle,
}

/// Music genre category (for filtering)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GenreCategory {
    Rock,
    Pop,
    Jazz,
    Classical,
    Electronic,
    HipHop,
    Country,
    RnB,
    Metal,
    Folk,
    Other,
}

/// Result codes for player operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerResult {
    Success,
    NotFound,
    AlreadyExists,
    InvalidState,
    EmptyPlaylist,
    IoError,
}

/// A single music track in the library
#[derive(Debug, Clone)]
pub struct Track {
    pub id: u64,
    pub title_hash: u64,
    pub artist_hash: u64,
    pub album_hash: u64,
    pub duration_ms: u32,
    pub path_hash: u64,
    pub genre_hash: u64,
    pub track_num: u16,
}

/// A user-created playlist
#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: u64,
    pub name_hash: u64,
    pub tracks: Vec<u64>,
}

/// Equalizer band settings (Q16 gain values)
#[derive(Debug, Clone)]
pub struct EqualizerBand {
    pub frequency: u32,
    pub gain_q16: i32,
}

/// Playback queue entry
#[derive(Debug, Clone)]
struct QueueEntry {
    track_id: u64,
    position: usize,
}

/// Persistent music player state
struct MusicPlayerState {
    library: Vec<Track>,
    playlists: Vec<Playlist>,
    queue: Vec<u64>,
    queue_position: usize,
    state: PlayerState,
    play_mode: PlayMode,
    volume_q16: i32,
    seek_position_ms: u32,
    current_track_id: Option<u64>,
    next_track_id: u64,
    next_playlist_id: u64,
    equalizer: Vec<EqualizerBand>,
    muted: bool,
    pre_mute_volume_q16: i32,
    shuffle_seed: u64,
    play_count: u64,
    total_listen_ms: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MUSIC_PLAYER: Mutex<Option<MusicPlayerState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_equalizer() -> Vec<EqualizerBand> {
    vec![
        EqualizerBand {
            frequency: 60,
            gain_q16: Q16_ZERO,
        },
        EqualizerBand {
            frequency: 170,
            gain_q16: Q16_ZERO,
        },
        EqualizerBand {
            frequency: 310,
            gain_q16: Q16_ZERO,
        },
        EqualizerBand {
            frequency: 600,
            gain_q16: Q16_ZERO,
        },
        EqualizerBand {
            frequency: 1000,
            gain_q16: Q16_ZERO,
        },
        EqualizerBand {
            frequency: 3000,
            gain_q16: Q16_ZERO,
        },
        EqualizerBand {
            frequency: 6000,
            gain_q16: Q16_ZERO,
        },
        EqualizerBand {
            frequency: 12000,
            gain_q16: Q16_ZERO,
        },
        EqualizerBand {
            frequency: 14000,
            gain_q16: Q16_ZERO,
        },
        EqualizerBand {
            frequency: 16000,
            gain_q16: Q16_ZERO,
        },
    ]
}

fn default_state() -> MusicPlayerState {
    MusicPlayerState {
        library: Vec::new(),
        playlists: Vec::new(),
        queue: Vec::new(),
        queue_position: 0,
        state: PlayerState::Stopped,
        play_mode: PlayMode::Normal,
        volume_q16: Q16_HALF,
        seek_position_ms: 0,
        current_track_id: None,
        next_track_id: 1,
        next_playlist_id: 1,
        equalizer: default_equalizer(),
        muted: false,
        pre_mute_volume_q16: Q16_HALF,
        shuffle_seed: 0xDEAD_BEEF_CAFE_BABE,
        play_count: 0,
        total_listen_ms: 0,
    }
}

/// Simple pseudo-random using xorshift on the shuffle seed
fn shuffle_next(seed: &mut u64) -> u64 {
    let mut s = *seed;
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    *seed = s;
    s
}

fn resolve_next_track(state: &mut MusicPlayerState) -> Option<u64> {
    if state.queue.is_empty() {
        return None;
    }
    match state.play_mode {
        PlayMode::Normal => {
            if state.queue_position + 1 < state.queue.len() {
                state.queue_position += 1;
                Some(state.queue[state.queue_position])
            } else {
                None
            }
        }
        PlayMode::Repeat => {
            state.queue_position = (state.queue_position + 1) % state.queue.len();
            Some(state.queue[state.queue_position])
        }
        PlayMode::RepeatOne => Some(state.queue[state.queue_position]),
        PlayMode::Shuffle => {
            let idx = (shuffle_next(&mut state.shuffle_seed) as usize) % state.queue.len();
            state.queue_position = idx;
            Some(state.queue[idx])
        }
    }
}

fn resolve_prev_track(state: &mut MusicPlayerState) -> Option<u64> {
    if state.queue.is_empty() {
        return None;
    }
    match state.play_mode {
        PlayMode::RepeatOne => Some(state.queue[state.queue_position]),
        _ => {
            if state.queue_position > 0 {
                state.queue_position -= 1;
            } else if state.play_mode == PlayMode::Repeat {
                state.queue_position = state.queue.len() - 1;
            } else {
                return None;
            }
            Some(state.queue[state.queue_position])
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scan a directory for music files and add them to the library
pub fn scan_library(dir_hash: u64) -> u32 {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return 0,
    };
    let mut count = 0u32;
    // Simulate scanning: generate tracks based on directory hash
    for i in 0u64..16 {
        let path_hash = dir_hash.wrapping_add(i.wrapping_mul(0xFACE));
        // Skip if already in library
        if state.library.iter().any(|t| t.path_hash == path_hash) {
            continue;
        }
        let id = state.next_track_id;
        state.next_track_id += 1;
        state.library.push(Track {
            id,
            title_hash: path_hash.wrapping_mul(0x1111),
            artist_hash: path_hash.wrapping_mul(0x2222) & 0xFFFF_FFFF_FFFF_0000,
            album_hash: path_hash.wrapping_mul(0x3333) & 0xFFFF_FFFF_0000_0000,
            duration_ms: ((i + 1) * 30_000 + 60_000) as u32,
            path_hash,
            genre_hash: path_hash & 0xFF00,
            track_num: (i + 1) as u16,
        });
        count += 1;
    }
    count
}

/// Start playing a track by ID
pub fn play(track_id: u64) -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    if !state.library.iter().any(|t| t.id == track_id) {
        return PlayerResult::NotFound;
    }
    state.current_track_id = Some(track_id);
    state.state = PlayerState::Playing;
    state.seek_position_ms = 0;
    state.play_count += 1;

    // Build queue from library if queue is empty
    if state.queue.is_empty() {
        state.queue = state.library.iter().map(|t| t.id).collect();
    }
    // Set queue position to this track
    if let Some(pos) = state.queue.iter().position(|&id| id == track_id) {
        state.queue_position = pos;
    }
    PlayerResult::Success
}

/// Play all tracks in order, starting from the first
pub fn play_all() -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    if state.library.is_empty() {
        return PlayerResult::EmptyPlaylist;
    }
    state.queue = state.library.iter().map(|t| t.id).collect();
    state.queue_position = 0;
    state.current_track_id = Some(state.queue[0]);
    state.state = PlayerState::Playing;
    state.seek_position_ms = 0;
    state.play_count += 1;
    PlayerResult::Success
}

/// Pause playback
pub fn pause() -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    match state.state {
        PlayerState::Playing => {
            state.state = PlayerState::Paused;
            PlayerResult::Success
        }
        _ => PlayerResult::InvalidState,
    }
}

/// Resume playback
pub fn resume() -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    match state.state {
        PlayerState::Paused => {
            state.state = PlayerState::Playing;
            PlayerResult::Success
        }
        _ => PlayerResult::InvalidState,
    }
}

/// Stop playback completely
pub fn stop() -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    state.state = PlayerState::Stopped;
    state.current_track_id = None;
    state.seek_position_ms = 0;
    PlayerResult::Success
}

/// Skip to next track
pub fn next() -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    if let Some(track_id) = resolve_next_track(state) {
        // Accumulate listen time for the old track
        state.total_listen_ms += state.seek_position_ms as u64;
        state.current_track_id = Some(track_id);
        state.seek_position_ms = 0;
        state.state = PlayerState::Playing;
        state.play_count += 1;
        PlayerResult::Success
    } else {
        state.state = PlayerState::Stopped;
        state.current_track_id = None;
        PlayerResult::EmptyPlaylist
    }
}

/// Go to previous track
pub fn prev() -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    // If we are more than 3 seconds in, restart the current track instead
    if state.seek_position_ms > 3000 {
        state.seek_position_ms = 0;
        return PlayerResult::Success;
    }
    if let Some(track_id) = resolve_prev_track(state) {
        state.total_listen_ms += state.seek_position_ms as u64;
        state.current_track_id = Some(track_id);
        state.seek_position_ms = 0;
        state.state = PlayerState::Playing;
        PlayerResult::Success
    } else {
        state.seek_position_ms = 0;
        PlayerResult::Success
    }
}

/// Seek to a position in milliseconds
pub fn seek(position_ms: u32) -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    let track_id = match state.current_track_id {
        Some(id) => id,
        None => return PlayerResult::InvalidState,
    };
    let duration = state
        .library
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.duration_ms)
        .unwrap_or(0);
    if position_ms > duration {
        return PlayerResult::InvalidState;
    }
    state.seek_position_ms = position_ms;
    PlayerResult::Success
}

/// Set volume (Q16 fixed-point, 0 to Q16_ONE)
pub fn set_volume(volume_q16: i32) -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    let clamped = if volume_q16 < Q16_ZERO {
        Q16_ZERO
    } else if volume_q16 > VOLUME_MAX {
        VOLUME_MAX
    } else {
        volume_q16
    };
    state.volume_q16 = clamped;
    if state.muted && clamped > 0 {
        state.muted = false;
    }
    PlayerResult::Success
}

/// Increase volume by one step
pub fn volume_up() -> PlayerResult {
    let guard = MUSIC_PLAYER.lock();
    let vol = match guard.as_ref() {
        Some(s) => s.volume_q16,
        None => return PlayerResult::IoError,
    };
    drop(guard);
    set_volume(vol + VOLUME_STEP)
}

/// Decrease volume by one step
pub fn volume_down() -> PlayerResult {
    let guard = MUSIC_PLAYER.lock();
    let vol = match guard.as_ref() {
        Some(s) => s.volume_q16,
        None => return PlayerResult::IoError,
    };
    drop(guard);
    set_volume(vol - VOLUME_STEP)
}

/// Toggle mute
pub fn toggle_mute() {
    let mut guard = MUSIC_PLAYER.lock();
    if let Some(state) = guard.as_mut() {
        if state.muted {
            state.volume_q16 = state.pre_mute_volume_q16;
            state.muted = false;
        } else {
            state.pre_mute_volume_q16 = state.volume_q16;
            state.volume_q16 = Q16_ZERO;
            state.muted = true;
        }
    }
}

/// Set the play mode
pub fn set_play_mode(mode: PlayMode) {
    let mut guard = MUSIC_PLAYER.lock();
    if let Some(state) = guard.as_mut() {
        state.play_mode = mode;
    }
}

/// Get current play mode
pub fn get_play_mode() -> PlayMode {
    let guard = MUSIC_PLAYER.lock();
    match guard.as_ref() {
        Some(state) => state.play_mode,
        None => PlayMode::Normal,
    }
}

/// Create a new playlist
pub fn create_playlist(name_hash: u64) -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    if state.playlists.iter().any(|p| p.name_hash == name_hash) {
        return PlayerResult::AlreadyExists;
    }
    let id = state.next_playlist_id;
    state.next_playlist_id += 1;
    state.playlists.push(Playlist {
        id,
        name_hash,
        tracks: Vec::new(),
    });
    PlayerResult::Success
}

/// Add a track to a playlist
pub fn add_to_playlist(track_id: u64, playlist_id: u64) -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    if !state.library.iter().any(|t| t.id == track_id) {
        return PlayerResult::NotFound;
    }
    let playlist = match state.playlists.iter_mut().find(|p| p.id == playlist_id) {
        Some(p) => p,
        None => return PlayerResult::NotFound,
    };
    if playlist.tracks.contains(&track_id) {
        return PlayerResult::AlreadyExists;
    }
    playlist.tracks.push(track_id);
    PlayerResult::Success
}

/// Remove a track from a playlist
pub fn remove_from_playlist(track_id: u64, playlist_id: u64) -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    let playlist = match state.playlists.iter_mut().find(|p| p.id == playlist_id) {
        Some(p) => p,
        None => return PlayerResult::NotFound,
    };
    let before = playlist.tracks.len();
    playlist.tracks.retain(|&id| id != track_id);
    if playlist.tracks.len() < before {
        PlayerResult::Success
    } else {
        PlayerResult::NotFound
    }
}

/// Delete a playlist
pub fn delete_playlist(playlist_id: u64) -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    let before = state.playlists.len();
    state.playlists.retain(|p| p.id != playlist_id);
    if state.playlists.len() < before {
        PlayerResult::Success
    } else {
        PlayerResult::NotFound
    }
}

/// Play a specific playlist
pub fn play_playlist(playlist_id: u64) -> PlayerResult {
    let mut guard = MUSIC_PLAYER.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return PlayerResult::IoError,
    };
    let playlist = match state.playlists.iter().find(|p| p.id == playlist_id) {
        Some(p) => p,
        None => return PlayerResult::NotFound,
    };
    if playlist.tracks.is_empty() {
        return PlayerResult::EmptyPlaylist;
    }
    state.queue = playlist.tracks.clone();
    state.queue_position = 0;
    state.current_track_id = Some(state.queue[0]);
    state.state = PlayerState::Playing;
    state.seek_position_ms = 0;
    state.play_count += 1;
    PlayerResult::Success
}

/// Get the current player state
pub fn get_state() -> PlayerState {
    let guard = MUSIC_PLAYER.lock();
    match guard.as_ref() {
        Some(state) => state.state,
        None => PlayerState::Stopped,
    }
}

/// Get the currently playing track ID
pub fn current_track() -> Option<u64> {
    let guard = MUSIC_PLAYER.lock();
    guard.as_ref().and_then(|s| s.current_track_id)
}

/// Get current seek position in ms
pub fn current_position_ms() -> u32 {
    let guard = MUSIC_PLAYER.lock();
    match guard.as_ref() {
        Some(state) => state.seek_position_ms,
        None => 0,
    }
}

/// Get the volume as Q16
pub fn get_volume_q16() -> i32 {
    let guard = MUSIC_PLAYER.lock();
    match guard.as_ref() {
        Some(state) => state.volume_q16,
        None => Q16_ZERO,
    }
}

/// Get total library track count
pub fn library_count() -> usize {
    let guard = MUSIC_PLAYER.lock();
    match guard.as_ref() {
        Some(state) => state.library.len(),
        None => 0,
    }
}

/// Get all playlists
pub fn get_playlists() -> Vec<Playlist> {
    let guard = MUSIC_PLAYER.lock();
    match guard.as_ref() {
        Some(state) => state.playlists.clone(),
        None => Vec::new(),
    }
}

/// Set an equalizer band gain
pub fn set_eq_band(index: usize, gain_q16: i32) {
    let mut guard = MUSIC_PLAYER.lock();
    if let Some(state) = guard.as_mut() {
        if index < state.equalizer.len() {
            state.equalizer[index].gain_q16 = gain_q16;
        }
    }
}

/// Get total play count
pub fn total_play_count() -> u64 {
    let guard = MUSIC_PLAYER.lock();
    match guard.as_ref() {
        Some(state) => state.play_count,
        None => 0,
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the music player subsystem
pub fn init() {
    let mut guard = MUSIC_PLAYER.lock();
    *guard = Some(default_state());
    serial_println!("    Music player ready");
}
