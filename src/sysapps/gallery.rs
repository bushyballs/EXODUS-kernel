use crate::sync::Mutex;
/// Photo and video gallery for Genesis OS
///
/// Provides a media gallery with album management, favorites, thumbnail
/// browsing, sorting by date, and sharing. Media items are identified
/// by path hashes and thumbnails are rendered from pre-computed hashes.
///
/// Inspired by: Google Photos, Apple Photos, Shotwell. All code is original.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Type of media file
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaType {
    Photo,
    Video,
    Gif,
    Screenshot,
    Panorama,
    Raw,
}

/// Sort criteria for media items
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaSort {
    DateNewest,
    DateOldest,
    SizeLargest,
    SizeSmallest,
    NameAsc,
    NameDesc,
}

/// Result codes for gallery operations
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GalleryResult {
    Success,
    NotFound,
    AlreadyExists,
    AlbumFull,
    IoError,
}

/// A single media item in the gallery
#[derive(Debug, Clone)]
pub struct MediaItem {
    pub id: u64,
    pub path_hash: u64,
    pub media_type: MediaType,
    pub thumbnail_hash: u64,
    pub date_taken: u64,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub favorite: bool,
    pub album_hash: u64,
}

/// An album grouping media items
#[derive(Debug, Clone)]
pub struct Album {
    pub id: u64,
    pub name_hash: u64,
    pub items: Vec<u64>,
    pub cover_hash: u64,
}

/// Gallery view mode
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GalleryView {
    AllPhotos,
    Albums,
    Favorites,
    Videos,
    Search,
}

/// Persistent gallery state
struct GalleryState {
    items: Vec<MediaItem>,
    albums: Vec<Album>,
    next_item_id: u64,
    next_album_id: u64,
    current_view: GalleryView,
    sort_mode: MediaSort,
    selected_album: Option<u64>,
    selected_items: Vec<u64>,
    slideshow_active: bool,
    slideshow_interval_ms: u32,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static GALLERY: Mutex<Option<GalleryState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn default_state() -> GalleryState {
    GalleryState {
        items: Vec::new(),
        albums: Vec::new(),
        next_item_id: 1,
        next_album_id: 1,
        current_view: GalleryView::AllPhotos,
        sort_mode: MediaSort::DateNewest,
        selected_album: None,
        selected_items: Vec::new(),
        slideshow_active: false,
        slideshow_interval_ms: 3000,
    }
}

fn detect_media_type(path_hash: u64) -> MediaType {
    match path_hash & 0x0F {
        0x00..=0x07 => MediaType::Photo,
        0x08..=0x0A => MediaType::Video,
        0x0B => MediaType::Gif,
        0x0C => MediaType::Screenshot,
        0x0D => MediaType::Panorama,
        0x0E..=0x0F => MediaType::Raw,
        _ => MediaType::Photo,
    }
}

fn sort_items(items: &mut Vec<MediaItem>, mode: MediaSort) {
    items.sort_by(|a, b| match mode {
        MediaSort::DateNewest => b.date_taken.cmp(&a.date_taken),
        MediaSort::DateOldest => a.date_taken.cmp(&b.date_taken),
        MediaSort::SizeLargest => b.size.cmp(&a.size),
        MediaSort::SizeSmallest => a.size.cmp(&b.size),
        MediaSort::NameAsc => a.path_hash.cmp(&b.path_hash),
        MediaSort::NameDesc => b.path_hash.cmp(&a.path_hash),
    });
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Scan for media files and populate the gallery library
pub fn scan_media(dir_hash: u64) -> u32 {
    let mut guard = GALLERY.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return 0,
    };

    let mut discovered = 0u32;
    // Simulate discovering media files in a directory tree
    for i in 0u64..12 {
        let path_hash = dir_hash.wrapping_add(i.wrapping_mul(0xABCD));
        let media_type = detect_media_type(path_hash);
        let width = match media_type {
            MediaType::Video => 1920,
            MediaType::Panorama => 8000,
            _ => 3024,
        };
        let height = match media_type {
            MediaType::Video => 1080,
            MediaType::Panorama => 2000,
            _ => 4032,
        };
        let id = state.next_item_id;
        state.next_item_id += 1;
        state.items.push(MediaItem {
            id,
            path_hash,
            media_type,
            thumbnail_hash: path_hash.wrapping_mul(0x1234_5678),
            date_taken: 1_700_000_000 + i * 86400,
            width,
            height,
            size: (i + 1) * 2_500_000,
            favorite: false,
            album_hash: 0,
        });
        discovered += 1;
    }

    sort_items(&mut state.items, state.sort_mode);
    discovered
}

/// Get all albums
pub fn get_albums() -> Vec<Album> {
    let guard = GALLERY.lock();
    match guard.as_ref() {
        Some(state) => state.albums.clone(),
        None => Vec::new(),
    }
}

/// Create a new album
pub fn create_album(name_hash: u64) -> GalleryResult {
    let mut guard = GALLERY.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return GalleryResult::IoError,
    };
    // Check for duplicate names
    if state.albums.iter().any(|a| a.name_hash == name_hash) {
        return GalleryResult::AlreadyExists;
    }
    let id = state.next_album_id;
    state.next_album_id += 1;
    state.albums.push(Album {
        id,
        name_hash,
        items: Vec::new(),
        cover_hash: 0,
    });
    GalleryResult::Success
}

/// Add a media item to an album
pub fn add_to_album(item_id: u64, album_id: u64) -> GalleryResult {
    let mut guard = GALLERY.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return GalleryResult::IoError,
    };
    // Verify item exists
    let item_exists = state.items.iter().any(|i| i.id == item_id);
    if !item_exists {
        return GalleryResult::NotFound;
    }
    let album = match state.albums.iter_mut().find(|a| a.id == album_id) {
        Some(a) => a,
        None => return GalleryResult::NotFound,
    };
    if album.items.contains(&item_id) {
        return GalleryResult::AlreadyExists;
    }
    album.items.push(item_id);
    // Set cover to first item if not already set
    if album.cover_hash == 0 {
        if let Some(item) = state.items.iter().find(|i| i.id == item_id) {
            album.cover_hash = item.thumbnail_hash;
        }
    }
    // Update item album_hash
    if let Some(item) = state.items.iter_mut().find(|i| i.id == item_id) {
        item.album_hash = album.name_hash;
    }
    GalleryResult::Success
}

/// Remove a media item from an album
pub fn remove_from_album(item_id: u64, album_id: u64) -> GalleryResult {
    let mut guard = GALLERY.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return GalleryResult::IoError,
    };
    let album = match state.albums.iter_mut().find(|a| a.id == album_id) {
        Some(a) => a,
        None => return GalleryResult::NotFound,
    };
    let before = album.items.len();
    album.items.retain(|&id| id != item_id);
    if album.items.len() < before {
        GalleryResult::Success
    } else {
        GalleryResult::NotFound
    }
}

/// Delete a media item from the library
pub fn delete_item(item_id: u64) -> GalleryResult {
    let mut guard = GALLERY.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return GalleryResult::IoError,
    };
    let before = state.items.len();
    state.items.retain(|i| i.id != item_id);
    if state.items.len() < before {
        // Also remove from all albums
        for album in state.albums.iter_mut() {
            album.items.retain(|&id| id != item_id);
        }
        GalleryResult::Success
    } else {
        GalleryResult::NotFound
    }
}

/// Delete an album (does not delete its media items)
pub fn delete_album(album_id: u64) -> GalleryResult {
    let mut guard = GALLERY.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return GalleryResult::IoError,
    };
    let before = state.albums.len();
    state.albums.retain(|a| a.id != album_id);
    if state.albums.len() < before {
        GalleryResult::Success
    } else {
        GalleryResult::NotFound
    }
}

/// Toggle favorite status on a media item
pub fn set_favorite(item_id: u64, favorite: bool) -> GalleryResult {
    let mut guard = GALLERY.lock();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return GalleryResult::IoError,
    };
    if let Some(item) = state.items.iter_mut().find(|i| i.id == item_id) {
        item.favorite = favorite;
        GalleryResult::Success
    } else {
        GalleryResult::NotFound
    }
}

/// Sort all items by date (newest or oldest first)
pub fn sort_by_date(newest_first: bool) {
    let mut guard = GALLERY.lock();
    if let Some(state) = guard.as_mut() {
        state.sort_mode = if newest_first {
            MediaSort::DateNewest
        } else {
            MediaSort::DateOldest
        };
        sort_items(&mut state.items, state.sort_mode);
    }
}

/// Sort by a specific criteria
pub fn sort_by(mode: MediaSort) {
    let mut guard = GALLERY.lock();
    if let Some(state) = guard.as_mut() {
        state.sort_mode = mode;
        sort_items(&mut state.items, mode);
    }
}

/// Share a media item (returns a share token hash)
pub fn share(item_id: u64) -> Option<u64> {
    let guard = GALLERY.lock();
    let state = guard.as_ref()?;
    let item = state.items.iter().find(|i| i.id == item_id)?;
    // Generate a share token from the path hash
    let token = item.path_hash.wrapping_mul(0xFEDC_BA98_7654_3210);
    Some(token)
}

/// Get all favorite items
pub fn get_favorites() -> Vec<MediaItem> {
    let guard = GALLERY.lock();
    match guard.as_ref() {
        Some(state) => state.items.iter().filter(|i| i.favorite).cloned().collect(),
        None => Vec::new(),
    }
}

/// Get items in a specific album
pub fn get_album_items(album_id: u64) -> Vec<MediaItem> {
    let guard = GALLERY.lock();
    let state = match guard.as_ref() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let album = match state.albums.iter().find(|a| a.id == album_id) {
        Some(a) => a,
        None => return Vec::new(),
    };
    state
        .items
        .iter()
        .filter(|i| album.items.contains(&i.id))
        .cloned()
        .collect()
}

/// Get total item count
pub fn item_count() -> usize {
    let guard = GALLERY.lock();
    match guard.as_ref() {
        Some(state) => state.items.len(),
        None => 0,
    }
}

/// Set gallery view mode
pub fn set_view(view: GalleryView) {
    let mut guard = GALLERY.lock();
    if let Some(state) = guard.as_mut() {
        state.current_view = view;
    }
}

/// Get current view mode
pub fn get_view() -> GalleryView {
    let guard = GALLERY.lock();
    match guard.as_ref() {
        Some(state) => state.current_view,
        None => GalleryView::AllPhotos,
    }
}

/// Start slideshow
pub fn start_slideshow(interval_ms: u32) {
    let mut guard = GALLERY.lock();
    if let Some(state) = guard.as_mut() {
        state.slideshow_active = true;
        state.slideshow_interval_ms = interval_ms;
    }
}

/// Stop slideshow
pub fn stop_slideshow() {
    let mut guard = GALLERY.lock();
    if let Some(state) = guard.as_mut() {
        state.slideshow_active = false;
    }
}

/// Select multiple items
pub fn select_items(ids: &[u64]) {
    let mut guard = GALLERY.lock();
    if let Some(state) = guard.as_mut() {
        state.selected_items.clear();
        for &id in ids {
            if state.items.iter().any(|i| i.id == id) {
                state.selected_items.push(id);
            }
        }
    }
}

/// Clear selection
pub fn clear_selection() {
    let mut guard = GALLERY.lock();
    if let Some(state) = guard.as_mut() {
        state.selected_items.clear();
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialize the gallery subsystem
pub fn init() {
    let mut guard = GALLERY.lock();
    *guard = Some(default_state());
    serial_println!("    Gallery ready");
}
