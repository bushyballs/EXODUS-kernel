/// App launcher for Genesis — home screen and app grid
///
/// Displays: app icons on home screen, app drawer, folders,
/// widgets, wallpaper, and dock. Users arrange apps freely.
///
/// Inspired by: Android Launcher3, iOS SpringBoard. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Home screen item
pub enum HomeItem {
    App {
        app_id: String,
        label: String,
        icon: Vec<u8>,
        x: u8,
        y: u8,
    },
    Folder {
        label: String,
        apps: Vec<String>,
        x: u8,
        y: u8,
    },
    Widget {
        app_id: String,
        widget_id: u32,
        x: u8,
        y: u8,
        width: u8,
        height: u8,
    },
}

/// Home screen page
pub struct HomePage {
    pub items: Vec<HomeItem>,
    pub wallpaper: Vec<u8>,
}

/// Dock item (bottom bar — always visible)
pub struct DockItem {
    pub app_id: String,
    pub label: String,
}

/// App drawer entry
pub struct AppEntry {
    pub app_id: String,
    pub label: String,
    pub icon: Vec<u8>,
    pub category: String,
    pub installed_at: u64,
}

/// Launcher state
pub struct Launcher {
    pub pages: Vec<HomePage>,
    pub current_page: usize,
    pub dock: Vec<DockItem>,
    pub app_drawer: Vec<AppEntry>,
    /// Grid dimensions
    pub grid_columns: u8,
    pub grid_rows: u8,
    /// Whether app drawer is open
    pub drawer_open: bool,
    /// Search query
    pub search_query: String,
}

impl Launcher {
    const fn new() -> Self {
        Launcher {
            pages: Vec::new(),
            current_page: 0,
            dock: Vec::new(),
            app_drawer: Vec::new(),
            grid_columns: 5,
            grid_rows: 5,
            drawer_open: false,
            search_query: String::new(),
        }
    }

    fn setup_defaults(&mut self) {
        // Create first home page
        self.pages.push(HomePage {
            items: Vec::new(),
            wallpaper: Vec::new(),
        });

        // Default dock items
        self.dock = alloc::vec![
            DockItem {
                app_id: String::from("com.genesis.phone"),
                label: String::from("Phone")
            },
            DockItem {
                app_id: String::from("com.genesis.messages"),
                label: String::from("Messages")
            },
            DockItem {
                app_id: String::from("com.genesis.browser"),
                label: String::from("Browser")
            },
            DockItem {
                app_id: String::from("com.genesis.camera"),
                label: String::from("Camera")
            },
        ];

        // System apps in drawer
        let system_apps = [
            ("com.genesis.settings", "Settings", "system"),
            ("com.genesis.files", "Files", "tools"),
            ("com.genesis.terminal", "Terminal", "tools"),
            ("com.genesis.calculator", "Calculator", "tools"),
            ("com.genesis.clock", "Clock", "tools"),
            ("com.genesis.calendar", "Calendar", "productivity"),
            ("com.genesis.contacts", "Contacts", "communication"),
            ("com.genesis.gallery", "Gallery", "media"),
            ("com.genesis.music", "Music", "media"),
            ("com.genesis.notes", "Notes", "productivity"),
            ("com.genesis.maps", "Maps", "navigation"),
            ("com.genesis.weather", "Weather", "info"),
        ];

        for (id, label, category) in &system_apps {
            self.app_drawer.push(AppEntry {
                app_id: String::from(*id),
                label: String::from(*label),
                icon: Vec::new(),
                category: String::from(*category),
                installed_at: 0,
            });
        }
    }

    /// Add app to home screen
    pub fn add_to_home(&mut self, page: usize, app_id: &str, x: u8, y: u8) -> bool {
        if page >= self.pages.len() {
            return false;
        }
        self.pages[page].items.push(HomeItem::App {
            app_id: String::from(app_id),
            label: String::from(app_id),
            icon: Vec::new(),
            x,
            y,
        });
        true
    }

    /// Remove app from home screen
    pub fn remove_from_home(&mut self, page: usize, app_id: &str) {
        if page < self.pages.len() {
            self.pages[page].items.retain(|item| match item {
                HomeItem::App { app_id: id, .. } => id != app_id,
                _ => true,
            });
        }
    }

    /// Switch home page
    pub fn switch_page(&mut self, page: usize) {
        if page < self.pages.len() {
            self.current_page = page;
        }
    }

    /// Open/close app drawer
    pub fn toggle_drawer(&mut self) {
        self.drawer_open = !self.drawer_open;
    }

    /// Search apps
    pub fn search(&self, query: &str) -> Vec<&AppEntry> {
        let q = query.to_lowercase();
        self.app_drawer
            .iter()
            .filter(|a| a.label.to_lowercase().contains(&q) || a.app_id.contains(&q))
            .collect()
    }

    /// Get page count
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Get app count in drawer
    pub fn app_count(&self) -> usize {
        self.app_drawer.len()
    }
}

static LAUNCHER: Mutex<Launcher> = Mutex::new(Launcher::new());

pub fn init() {
    LAUNCHER.lock().setup_defaults();
    crate::serial_println!(
        "  [launcher] App launcher initialized ({} apps)",
        LAUNCHER.lock().app_count()
    );
}

pub fn toggle_drawer() {
    LAUNCHER.lock().toggle_drawer();
}
pub fn switch_page(page: usize) {
    LAUNCHER.lock().switch_page(page);
}
pub fn app_count() -> usize {
    LAUNCHER.lock().app_count()
}
