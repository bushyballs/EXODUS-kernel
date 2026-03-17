pub mod animations;
pub mod freeform;
pub mod gestures;
pub mod hotkeys;
pub mod integration;
pub mod layouts;
pub mod pip;
/// Multi-window management subsystem for Genesis
///
/// This module provides comprehensive window management including:
/// - Split-screen mode (horizontal/vertical splits with adjustable ratios)
/// - Picture-in-picture (floating overlay windows)
/// - Freeform windows (traditional desktop-style window management)
/// - Predefined layouts (grid, side-by-side, focus mode, etc.)
/// - Window animations (move, resize, fade, minimize, maximize)
/// - Gesture recognition (touch/trackpad gestures for window control)
/// - Hotkey bindings (keyboard shortcuts for window operations)
pub mod split_screen;

use crate::{serial_print, serial_println};

pub fn init() {
    serial_println!("[multiwindow] Initializing multi-window subsystem");
    split_screen::init();
    pip::init();
    freeform::init();
    gestures::init();
    hotkeys::init();
    integration::init();
    serial_println!("[multiwindow] Multi-window subsystem initialized");
    serial_println!("  [OK] Split-screen (max: 4 sessions)");
    serial_println!("  [OK] Picture-in-picture (max: 3 windows)");
    serial_println!("  [OK] Freeform window manager");
    serial_println!("  [OK] Layout presets (9 layouts)");
    serial_println!("  [OK] Animation engine (6 easing functions)");
    serial_println!("  [OK] Gesture recognizer (14 gesture types)");
    serial_println!("  [OK] Hotkey manager (default Windows-style bindings)");
    serial_println!("  [OK] Unified integration layer");
}

// Re-export commonly used types for convenience
pub use gestures::Gesture;
pub use hotkeys::{Modifiers, WindowAction};
pub use layouts::{Layout, SnapZone, WindowRect};
pub use split_screen::{SplitOrientation, SplitRatio};
