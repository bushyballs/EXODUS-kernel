use super::font;
use super::theme;
use crate::drivers::framebuffer::Color;
use crate::sync::Mutex;
/// Desktop shell for Genesis
///
/// The desktop shell provides:
///   - Taskbar with app launcher and system tray
///   - Application launcher (search + grid)
///   - Desktop icons
///   - System notifications
///   - Wallpaper
///
/// This is what the user sees and interacts with.
/// Inspired by: KDE Plasma (power + customization), GNOME (simplicity),
/// iOS (gesture fluidity), Android (app drawer). All code is original.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec::Vec;

/// Application entry in the launcher
#[derive(Debug, Clone)]
pub struct AppEntry {
    pub name: String,
    pub icon_color: u32, // solid color icon for now
    pub command: String,
}

/// Desktop shell state
pub struct DesktopShell {
    /// Whether the app launcher is open
    pub launcher_open: bool,
    /// Registered applications
    pub apps: Vec<AppEntry>,
    /// System tray items
    pub tray_items: Vec<String>,
    /// Clock text
    pub clock_text: String,
    /// Notification queue
    pub notifications: Vec<Notification>,
}

/// A desktop notification
#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub icon_color: u32,
    pub timestamp: u64,
}

impl DesktopShell {
    pub fn new() -> Self {
        DesktopShell {
            launcher_open: false,
            apps: alloc::vec![
                AppEntry {
                    name: String::from("Terminal"),
                    icon_color: Color::rgb(50, 50, 70).to_u32(),
                    command: String::from("hoags-terminal"),
                },
                AppEntry {
                    name: String::from("Files"),
                    icon_color: Color::rgb(60, 140, 180).to_u32(),
                    command: String::from("hoags-files"),
                },
                AppEntry {
                    name: String::from("Bid Command"),
                    icon_color: Color::rgb(0, 200, 220).to_u32(),
                    command: String::from("bid-command"),
                },
                AppEntry {
                    name: String::from("AI Hub"),
                    icon_color: Color::rgb(100, 60, 180).to_u32(),
                    command: String::from("hoags-ai"),
                },
                AppEntry {
                    name: String::from("Settings"),
                    icon_color: Color::rgb(80, 80, 100).to_u32(),
                    command: String::from("hoags-settings"),
                },
                AppEntry {
                    name: String::from("Browser"),
                    icon_color: Color::rgb(30, 120, 200).to_u32(),
                    command: String::from("hoags-browser"),
                },
            ],
            tray_items: alloc::vec![
                String::from("Net"),
                String::from("Vol"),
                String::from("Bat"),
            ],
            clock_text: String::from("12:00"),
            notifications: Vec::new(),
        }
    }

    /// Toggle the app launcher
    pub fn toggle_launcher(&mut self) {
        self.launcher_open = !self.launcher_open;
    }

    /// Draw the taskbar into the compositor's back buffer
    pub fn draw_taskbar(&self, buf: &mut [u32], screen_w: u32, screen_h: u32) {
        let theme = theme::THEME.lock();
        let taskbar_y = screen_h - theme.taskbar_height;
        let taskbar_bg = theme.taskbar_bg.to_u32();
        let text_color = theme.taskbar_text.to_u32();
        let accent = theme.accent.to_u32();

        // Taskbar background
        for y in taskbar_y..screen_h {
            for x in 0..screen_w {
                if (y * screen_w + x) < buf.len() as u32 {
                    buf[(y * screen_w + x) as usize] = taskbar_bg;
                }
            }
        }

        // Top border line (accent color)
        for x in 0..screen_w {
            let idx = (taskbar_y * screen_w + x) as usize;
            if idx < buf.len() {
                buf[idx] = accent;
            }
        }

        // App launcher button (left side)
        let btn_w = 80u32;
        let btn_h = theme.taskbar_height - 4;
        let btn_y = taskbar_y + 2;
        for y in btn_y..btn_y + btn_h {
            for x in 4..4 + btn_w {
                let idx = (y * screen_w + x) as usize;
                if idx < buf.len() {
                    buf[idx] = accent;
                }
            }
        }

        // "HOAGS" text on launcher button
        font::draw_string(buf, screen_w, 12, btn_y + 6, "HOAGS", 0xFFFFFF);

        // Clock (right side)
        let clock_x = screen_w - 60;
        font::draw_string(
            buf,
            screen_w,
            clock_x,
            taskbar_y + 8,
            &self.clock_text,
            text_color,
        );

        // Tray items
        let mut tray_x = clock_x - 120;
        for item in &self.tray_items {
            font::draw_string(buf, screen_w, tray_x, taskbar_y + 8, item, text_color);
            tray_x += 40;
        }
    }

    /// Draw the app launcher overlay
    pub fn draw_launcher(&self, buf: &mut [u32], screen_w: u32, screen_h: u32) {
        if !self.launcher_open {
            return;
        }

        let theme = theme::THEME.lock();

        // Launcher panel (left side, full height minus taskbar)
        let panel_w = 280u32;
        let panel_h = screen_h - theme.taskbar_height;
        let panel_bg = Color::rgb(20, 20, 32).to_u32();

        for y in 0..panel_h {
            for x in 0..panel_w {
                let idx = (y * screen_w + x) as usize;
                if idx < buf.len() {
                    buf[idx] = panel_bg;
                }
            }
        }

        // Right border
        for y in 0..panel_h {
            let idx = (y * screen_w + panel_w) as usize;
            if idx < buf.len() {
                buf[idx] = theme.accent.to_u32();
            }
        }

        // Title
        font::draw_string(buf, screen_w, 16, 16, "Applications", theme.accent.to_u32());

        // App list
        let mut app_y = 48u32;
        for app in &self.apps {
            // Icon (colored square)
            for dy in 0..24u32 {
                for dx in 0..24u32 {
                    let idx = ((app_y + dy) * screen_w + 16 + dx) as usize;
                    if idx < buf.len() {
                        buf[idx] = app.icon_color;
                    }
                }
            }

            // App name
            font::draw_string(buf, screen_w, 48, app_y + 4, &app.name, 0xDDDDEE);

            app_y += 36;
        }
    }

    /// Add a notification
    pub fn notify(&mut self, title: &str, body: &str) {
        self.notifications.push(Notification {
            title: String::from(title),
            body: String::from(body),
            icon_color: Color::rgb(0, 200, 220).to_u32(),
            timestamp: 0,
        });
    }
}

/// Global shell instance
pub static SHELL: Mutex<Option<DesktopShell>> = Mutex::new(None);

/// Initialize the desktop shell
pub fn init() {
    *SHELL.lock() = Some(DesktopShell::new());
    serial_println!("  Shell: Hoags Desktop Shell initialized");
}

/// Send a notification
pub fn notify(title: &str, body: &str) {
    if let Some(ref mut shell) = *SHELL.lock() {
        shell.notify(title, body);
    }
}
