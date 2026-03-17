/// Media player for Genesis — audio/video playback
///
/// Manages media sessions, playback state, queue management,
/// and media controls. Integrates with lock screen and status bar.
///
/// Inspired by: Android MediaSession, ExoPlayer. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Playback state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Idle,
    Buffering,
    Playing,
    Paused,
    Stopped,
    Error,
}

/// Repeat mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    One,
    All,
}

/// Media item
pub struct MediaItem {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub uri: String,
    pub mime_type: String,
    /// Album art (ARGB data)
    pub artwork: Vec<u8>,
}

/// Media session (one per active player)
pub struct MediaSession {
    pub id: u32,
    pub app_id: String,
    pub state: PlaybackState,
    pub current_item: Option<MediaItem>,
    pub queue: Vec<MediaItem>,
    pub queue_index: usize,
    pub position_ms: u64,
    pub volume: u8, // 0-100
    pub repeat_mode: RepeatMode,
    pub shuffle: bool,
    pub speed: u8, // 10 = 1.0x, 20 = 2.0x
}

impl MediaSession {
    pub fn new(id: u32, app_id: &str) -> Self {
        MediaSession {
            id,
            app_id: String::from(app_id),
            state: PlaybackState::Idle,
            current_item: None,
            queue: Vec::new(),
            queue_index: 0,
            position_ms: 0,
            volume: 80,
            repeat_mode: RepeatMode::Off,
            shuffle: false,
            speed: 10,
        }
    }

    pub fn play(&mut self) {
        if self.current_item.is_some() {
            self.state = PlaybackState::Playing;
        }
    }

    pub fn pause(&mut self) {
        if self.state == PlaybackState::Playing {
            self.state = PlaybackState::Paused;
        }
    }

    pub fn stop(&mut self) {
        self.state = PlaybackState::Stopped;
        self.position_ms = 0;
    }

    pub fn next(&mut self) -> bool {
        if self.queue_index + 1 < self.queue.len() {
            self.queue_index += 1;
            self.load_current();
            true
        } else if self.repeat_mode == RepeatMode::All && !self.queue.is_empty() {
            self.queue_index = 0;
            self.load_current();
            true
        } else {
            false
        }
    }

    pub fn previous(&mut self) -> bool {
        if self.position_ms > 3000 {
            // Restart current track if > 3 seconds in
            self.position_ms = 0;
            true
        } else if self.queue_index > 0 {
            self.queue_index -= 1;
            self.load_current();
            true
        } else {
            false
        }
    }

    pub fn seek(&mut self, position_ms: u64) {
        if let Some(ref item) = self.current_item {
            self.position_ms = position_ms.min(item.duration_ms);
        }
    }

    pub fn add_to_queue(&mut self, item: MediaItem) {
        self.queue.push(item);
        if self.current_item.is_none() {
            self.load_current();
        }
    }

    fn load_current(&mut self) {
        if self.queue_index < self.queue.len() {
            // Move item info to current
            let item = &self.queue[self.queue_index];
            self.current_item = Some(MediaItem {
                title: item.title.clone(),
                artist: item.artist.clone(),
                album: item.album.clone(),
                duration_ms: item.duration_ms,
                uri: item.uri.clone(),
                mime_type: item.mime_type.clone(),
                artwork: item.artwork.clone(),
            });
            self.position_ms = 0;
        }
    }

    /// Progress (0.0 to 1.0)
    pub fn progress(&self) -> f32 {
        match &self.current_item {
            Some(item) if item.duration_ms > 0 => self.position_ms as f32 / item.duration_ms as f32,
            _ => 0.0,
        }
    }

    /// Format position as MM:SS
    pub fn position_string(&self) -> String {
        let secs = self.position_ms / 1000;
        format!("{}:{:02}", secs / 60, secs % 60)
    }

    /// Format duration as MM:SS
    pub fn duration_string(&self) -> String {
        match &self.current_item {
            Some(item) => {
                let secs = item.duration_ms / 1000;
                format!("{}:{:02}", secs / 60, secs % 60)
            }
            None => String::from("0:00"),
        }
    }
}

/// Media session manager
pub struct MediaManager {
    sessions: Vec<MediaSession>,
    active_session: Option<u32>,
    next_id: u32,
}

impl MediaManager {
    const fn new() -> Self {
        MediaManager {
            sessions: Vec::new(),
            active_session: None,
            next_id: 1,
        }
    }

    pub fn create_session(&mut self, app_id: &str) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.sessions.push(MediaSession::new(id, app_id));
        self.active_session = Some(id);
        id
    }

    pub fn get_session(&mut self, id: u32) -> Option<&mut MediaSession> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    pub fn active_session(&mut self) -> Option<&mut MediaSession> {
        let id = self.active_session?;
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    pub fn destroy_session(&mut self, id: u32) {
        self.sessions.retain(|s| s.id != id);
        if self.active_session == Some(id) {
            self.active_session = self.sessions.last().map(|s| s.id);
        }
    }
}

static MEDIA_MANAGER: Mutex<MediaManager> = Mutex::new(MediaManager::new());

pub fn init() {
    crate::serial_println!("  [media-player] Media player initialized");
}

pub fn create_session(app_id: &str) -> u32 {
    MEDIA_MANAGER.lock().create_session(app_id)
}
