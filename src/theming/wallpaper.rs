use crate::sync::Mutex;
/// Wallpaper management for Genesis
///
/// Static, live, dynamic wallpapers, slideshow,
/// color extraction for dynamic theming.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum WallpaperType {
    Static,
    Live,
    Dynamic,
    Slideshow,
}

#[derive(Clone, Copy, PartialEq)]
pub enum WallpaperTarget {
    HomeScreen,
    LockScreen,
    Both,
}

#[derive(Clone, Copy)]
struct Wallpaper {
    id: u32,
    image_hash: u64,
    wp_type: WallpaperType,
    target: WallpaperTarget,
    scroll_enabled: bool,
    dim_on_lock: u8,
    slideshow_interval_min: u16,
    dominant_color: u32,
}

struct WallpaperManager {
    current_home: Option<Wallpaper>,
    current_lock: Option<Wallpaper>,
    favorites: Vec<Wallpaper>,
    next_id: u32,
}

static WALLPAPER: Mutex<Option<WallpaperManager>> = Mutex::new(None);

impl WallpaperManager {
    fn new() -> Self {
        WallpaperManager {
            current_home: None,
            current_lock: None,
            favorites: Vec::new(),
            next_id: 1,
        }
    }

    fn set_wallpaper(
        &mut self,
        image_hash: u64,
        wp_type: WallpaperType,
        target: WallpaperTarget,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        // Simple dominant color extraction from hash
        let dominant = (image_hash as u32) | 0xFF_000000;
        let wp = Wallpaper {
            id,
            image_hash,
            wp_type,
            target,
            scroll_enabled: true,
            dim_on_lock: 40,
            slideshow_interval_min: 30,
            dominant_color: dominant,
        };
        match target {
            WallpaperTarget::HomeScreen => self.current_home = Some(wp),
            WallpaperTarget::LockScreen => self.current_lock = Some(wp),
            WallpaperTarget::Both => {
                self.current_home = Some(wp);
                self.current_lock = Some(wp);
            }
        }
        id
    }

    fn add_to_favorites(&mut self, wp: Wallpaper) {
        if self.favorites.len() < 50 {
            self.favorites.push(wp);
        }
    }

    fn get_dominant_color(&self, target: WallpaperTarget) -> u32 {
        match target {
            WallpaperTarget::HomeScreen | WallpaperTarget::Both => {
                self.current_home.map_or(0xFF_1976D2, |w| w.dominant_color)
            }
            WallpaperTarget::LockScreen => {
                self.current_lock.map_or(0xFF_1976D2, |w| w.dominant_color)
            }
        }
    }
}

pub fn init() {
    let mut wm = WALLPAPER.lock();
    *wm = Some(WallpaperManager::new());
    serial_println!("    Wallpaper manager ready");
}
