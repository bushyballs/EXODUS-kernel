/// Hoags Display Server — the compositor for Genesis
///
/// A Wayland-inspired display server built from scratch.
/// Handles window compositing, input routing, and rendering.
///
/// Architecture:
///   Compositor (this module) manages all windows and surfaces
///   Clients (applications) create surfaces and draw into buffers
///   The compositor blends all surfaces and presents to the framebuffer
///   Input events are routed to the focused window
///
/// Inspired by: Wayland (client-server protocol, no X legacy),
/// iOS (smooth 60fps compositing), Android (surface flinger),
/// Fuchsia (scenic). All code is original.
use crate::{serial_print, serial_println};
pub mod accessibility;
pub mod adaptive_ui;
pub mod ai_display;
pub mod animation;
pub mod boot_anim;
pub mod clipboard;
pub mod color_mgr;
pub mod color_space;
pub mod compositor;
pub mod cursor;
pub mod damage;
pub mod drm;
pub mod font;
pub mod gamma;
pub mod hdr;
pub mod input;
pub mod multi_monitor;
pub mod render2d;
pub mod screen_recorder;
pub mod screenshot;
pub mod shell;
pub mod theme;
pub mod vrr;
pub mod vtmux;
pub mod wallpaper;
pub mod wayland;
pub mod widget;
pub mod window;

/// Initialize the display subsystem
pub fn init() {
    theme::init();
    font::init();
    input::init();
    compositor::init();
    shell::init();
    wayland::init();
    vtmux::init();
    clipboard::init();
    drm::init();
    ai_display::init();
    adaptive_ui::init();
    boot_anim::init();
    wallpaper::init();
    screen_recorder::init();
    multi_monitor::init();
    color_mgr::init();
    widget::init();
    damage::init();
    cursor::init();
    gamma::init();
    hdr::init();
    vrr::init();
    screenshot::init();
    accessibility::init();
    color_space::init();

    serial_println!("  Display: Hoags Compositor + Wayland + widget toolkit + AI adaptive display + damage, cursor, gamma, hdr, vrr, screenshot, accessibility, color_space");
}
