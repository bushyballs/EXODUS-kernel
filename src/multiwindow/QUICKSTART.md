# Multi-Window Module Quick Start Guide

Get up and running with the Genesis multiwindow subsystem in 5 minutes.

## Module Location

```
genesis/src/multiwindow/
```

## Initialization

The module is automatically initialized during kernel boot in Phase 18n.

```rust
// In main.rs - already configured
multiwindow::init();
```

## Basic Usage Examples

### 1. Create a Freeform Window (Desktop Style)

```rust
use multiwindow::freeform;

// Create a window
let window_id = freeform::create_window(
    app_id,      // Your app's ID
    100, 100,    // Position (x, y)
    800, 600,    // Size (width, height)
    true,        // Resizable
    title_hash,  // Window title hash
);

// Maximize it
freeform::maximize(window_id);

// Later: restore
freeform::restore(window_id);

// Close it
freeform::close_window(window_id);
```

### 2. Split-Screen Mode

```rust
use multiwindow::{split_screen, SplitOrientation, SplitRatio};

// Create vertical split (side-by-side)
let session = split_screen::create_split(
    browser_id,
    terminal_id,
    SplitOrientation::Vertical,
    SplitRatio::Equal,
).unwrap();

// User wants more space for browser (70/30 split)
split_screen::adjust_divider(session, 70);

// Done with split
split_screen::end_split(session);
```

### 3. Picture-in-Picture Video

```rust
use multiwindow::pip;

// Create small video overlay (16:9 aspect)
let pip_id = pip::create_pip(
    video_app_id,
    1600, 900,   // Top-right area
    320, 180,    // 320×180 size
    16, 9,       // 16:9 aspect ratio
).unwrap();

// Snap to bottom-right corner
pip::snap_to_corner(pip_id, 2, 16);

// Make 90% opaque
pip::set_opacity(pip_id, 230);
```

### 4. Apply Predefined Layout

```rust
use multiwindow::{integration, Layout};

// Get your window IDs
let window_ids = vec![w1, w2, w3, w4];

// Arrange in 2×2 grid
integration::apply_layout(Layout::Grid2x2, &window_ids);

// Or side-by-side
let window_ids = vec![browser_id, editor_id];
integration::apply_layout(Layout::SideBySide, &window_ids);
```

### 5. Snap Window to Screen Half

```rust
use multiwindow::{integration, freeform, SnapZone};

// Get active window
let window = freeform::get_top_window().unwrap();

// Snap to left half
integration::snap_to_zone(window.id, SnapZone::Left);

// Or snap to top-right quarter
integration::snap_to_zone(window.id, SnapZone::TopRight);
```

### 6. Handle User Gestures

```rust
use multiwindow::{gestures, integration};

// In your touch event handler:
gestures::touch_down(touch_id, x, y, timestamp);
gestures::touch_move(touch_id, new_x, new_y, timestamp);

// On release, get gesture
if let Some(gesture) = gestures::touch_up(touch_id, timestamp) {
    // Let integration layer handle it
    integration::handle_gesture(gesture);
}
```

### 7. Process Keyboard Shortcuts

```rust
use multiwindow::{hotkeys, integration, Modifiers};

// In your keyboard handler:
let modifiers = Modifiers {
    ctrl: is_ctrl_pressed,
    alt: is_alt_pressed,
    shift: is_shift_pressed,
    super_key: is_win_pressed,
};

// Process the key
integration::handle_hotkey(modifiers, key_code);
```

### 8. Custom Hotkey Binding

```rust
use multiwindow::{hotkeys, Modifiers, WindowAction};

// Register Ctrl+Shift+T to tile windows
hotkeys::register_hotkey(
    Modifiers {
        ctrl: true,
        shift: true,
        alt: false,
        super_key: false,
    },
    84, // 'T' key
    WindowAction::TileWindows,
);
```

### 9. Show All Windows (Overview Mode)

```rust
use multiwindow::{freeform, integration, Layout};

// Get all windows
let windows = freeform::get_windows();
let ids: Vec<u32> = windows.iter().map(|w| w.id).collect();

// Show in grid
if ids.len() <= 4 {
    integration::apply_layout(Layout::Grid2x2, &ids);
} else {
    integration::apply_layout(Layout::Grid3x3, &ids);
}
```

### 10. Window Management Operations

```rust
use multiwindow::freeform;

// Cascade all windows
freeform::cascade();

// Tile all windows in grid
freeform::tile_all();

// Minimize all (show desktop)
for window in freeform::get_windows() {
    freeform::minimize(window.id);
}

// Bring specific window to front
freeform::bring_to_front(my_window_id);
```

## Common Patterns

### Pattern 1: Maximize on First Launch

```rust
let window_id = freeform::create_window(app_id, 0, 0, 800, 600, true, hash);
freeform::maximize(window_id);
```

### Pattern 2: PIP Corner Toggle

```rust
// Cycle through corners: 0→1→2→3→0
let corner = (current_corner + 1) % 4;
pip::snap_to_corner(pip_id, corner, 16);
```

### Pattern 3: Responsive Layout

```rust
let count = window_ids.len();
let layout = match count {
    1 => Layout::Fullscreen,
    2 => Layout::SideBySide,
    3 => Layout::ThreeColumn,
    4 => Layout::Grid2x2,
    5..=6 => Layout::Grid2x3,
    _ => Layout::Grid3x3,
};
integration::apply_layout(layout, &window_ids);
```

### Pattern 4: Window Finder

```rust
// Find window at mouse position
if let Some(window_id) = freeform::hit_test(mouse_x, mouse_y) {
    freeform::bring_to_front(window_id);
}
```

### Pattern 5: Save Window State

```rust
// Before minimizing, save state
let window = freeform::get_top_window().unwrap();
let saved_state = (window.x, window.y, window.width, window.height);

// After restore, apply state
freeform::move_window(window_id, saved_state.0, saved_state.1);
freeform::resize_window(window_id, saved_state.2, saved_state.3);
```

## Configuration

### Set Screen Resolution

```rust
use multiwindow::layouts::{ScreenConfig, set_screen_config};

let config = ScreenConfig {
    width: 3840,        // 4K width
    height: 2160,       // 4K height
    taskbar_height: 64, // Custom taskbar
    margin: 12,         // Window margins
};
set_screen_config(config);
```

## Default Hotkeys Reference

| Hotkey | Action |
|--------|--------|
| Win+Left | Snap left |
| Win+Right | Snap right |
| Win+Up | Maximize |
| Win+Down | Minimize |
| Win+D | Show desktop |
| Win+Tab | Show all windows |
| Alt+F4 | Close window |
| F11 | Fullscreen |
| Alt+Tab | Next window |
| Win+1-9 | Switch workspace |

## Default Gestures Reference

| Gesture | Action |
|---------|--------|
| 3-finger swipe up | Show all windows |
| 3-finger swipe down | Show desktop |
| 3-finger swipe left | Next window |
| 3-finger swipe right | Previous window |
| 4-finger tap | Overview mode |
| Double tap | Maximize window |
| Swipe left | Snap left |
| Swipe right | Snap right |

## Troubleshooting

### Problem: Window not appearing
```rust
// Check if window was created
if window_id == 0 {
    // Creation failed
}

// Verify window exists
let windows = freeform::get_windows();
if !windows.iter().any(|w| w.id == window_id) {
    // Window was closed or doesn't exist
}
```

### Problem: Layout not applying
```rust
// Ensure window count matches layout capacity
let rects = layouts::calculate_layout(layout, window_ids.len(), &config);
if rects.len() != window_ids.len() {
    // Layout can't accommodate this many windows
}
```

### Problem: Gesture not recognized
```rust
// Reset gesture state if stuck
gestures::reset();

// Check thresholds in gesture recognizer
// May need to adjust swipe_threshold_pixels
```

### Problem: Hotkey not working
```rust
// Check if binding exists
let bindings = hotkeys::get_all_bindings();
let exists = bindings.iter().any(|h| {
    h.key_code == my_key && h.modifiers.ctrl == is_ctrl
});
```

## Performance Tips

1. **Limit PIP windows** - Max 3 for smooth rendering
2. **Limit split sessions** - Max 4 to avoid compositor overhead
3. **Use snap zones** - Faster than manual positioning
4. **Batch layout changes** - Apply layout once, not per-window
5. **Minimize animation complexity** - Use EaseOut for most UI

## Next Steps

- Read [README.md](./README.md) for comprehensive API reference
- See [IMPLEMENTATION.md](./IMPLEMENTATION.md) for technical details
- Check individual module files for advanced features
- Integrate with your display compositor
- Add visual window decorations
- Implement window preview thumbnails

## Support

For issues or questions about the multiwindow subsystem:
1. Check the README.md for API documentation
2. Review IMPLEMENTATION.md for design decisions
3. Examine source code comments for implementation details
4. Refer to Genesis OS kernel documentation

---

**Quick Reference Card:**

```rust
// Freeform window
let w = freeform::create_window(id, x, y, w, h, resizable, hash);

// Split-screen
let s = split_screen::create_split(id1, id2, orientation, ratio);

// Picture-in-picture
let p = pip::create_pip(id, x, y, w, h, aspect_x, aspect_y);

// Layout
integration::apply_layout(Layout::Grid2x2, &[w1, w2, w3, w4]);

// Snap
integration::snap_to_zone(window_id, SnapZone::Left);

// Gesture
integration::handle_gesture(gesture);

// Hotkey
integration::handle_hotkey(modifiers, key_code);
```

Happy window managing! 🪟
