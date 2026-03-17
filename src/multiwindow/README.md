# Multi-Window Management Subsystem

A comprehensive window management system for the Genesis OS, providing split-screen, picture-in-picture, freeform windows, layouts, animations, gesture recognition, and hotkey support.

## Architecture Overview

The multiwindow subsystem consists of 8 integrated modules:

1. **split_screen** - Split-screen mode with horizontal/vertical layouts
2. **pip** - Picture-in-picture floating overlay windows
3. **freeform** - Traditional desktop-style window management
4. **layouts** - Predefined window layout configurations
5. **animations** - Window movement, resize, and transition animations
6. **gestures** - Touch and trackpad gesture recognition
7. **hotkeys** - Keyboard shortcut bindings for window operations
8. **integration** - Unified API coordinating all subsystems

## Module Details

### Split Screen (`split_screen.rs`)

Split-screen mode allows running two applications side-by-side with an adjustable divider.

**Features:**
- Horizontal and vertical split orientations
- Customizable split ratios (equal, 1/3-2/3, 2/3-1/3, custom percentage)
- Adjustable divider position
- Swap apps between panes
- Maximum 4 concurrent split sessions

**Example:**
```rust
use multiwindow::split_screen::{SplitOrientation, SplitRatio};

// Create a vertical split with equal ratio
let session_id = split_screen::create_split(
    app1_id,
    app2_id,
    SplitOrientation::Vertical,
    SplitRatio::Equal,
).unwrap();

// Adjust divider to 70/30 split
split_screen::adjust_divider(session_id, 70);

// Swap the two apps
split_screen::swap_apps(session_id);

// End split session
split_screen::end_split(session_id);
```

### Picture-in-Picture (`pip.rs`)

PIP mode creates small floating overlay windows that stay on top of other content.

**Features:**
- Multiple concurrent PIP windows (max 3)
- Aspect ratio locking
- Always-on-top mode
- Opacity control
- Snap to corners
- Move and resize with aspect ratio preservation

**Example:**
```rust
// Create a PIP window (16:9 aspect ratio)
let pip_id = pip::create_pip(
    video_app_id,
    1600, 900,  // position (x, y)
    320, 180,   // size (width, height)
    16, 9,      // aspect ratio
).unwrap();

// Snap to bottom-right corner
pip::snap_to_corner(pip_id, 2, 16);

// Set 80% opacity
pip::set_opacity(pip_id, 204);

// Close PIP window
pip::close_pip(pip_id);
```

### Freeform Windows (`freeform.rs`)

Traditional desktop window manager with overlapping resizable windows.

**Features:**
- Create, move, resize windows
- Minimize, maximize, restore
- Z-order management (bring to front)
- Window decorations (title bar, borders)
- Cascade and tile all windows
- Desktop mode toggle
- Hit testing for mouse clicks

**Example:**
```rust
// Create a freeform window
let window_id = freeform::create_window(
    app_id,
    100, 100,    // position (x, y)
    800, 600,    // size (width, height)
    true,        // resizable
    title_hash,  // window title hash
);

// Maximize window
freeform::maximize(window_id);

// Restore to original size
freeform::restore(window_id);

// Tile all windows in a grid
freeform::tile_all();

// Cascade all windows
freeform::cascade();
```

### Layouts (`layouts.rs`)

Predefined window layout configurations for quick arrangement.

**Layout Types:**
- `Fullscreen` - Single window takes entire screen
- `SideBySide` - Two windows 50/50 split
- `MainSidebar` - Main window 70%, sidebar 30%
- `ThreeColumn` - Three equal columns
- `Grid2x2` - 2×2 grid of windows
- `Grid2x3` - 2×3 grid of windows
- `Grid3x3` - 3×3 grid of windows
- `Focus` - One large centered window
- `PipOverlay` - Main window + PIP in corner

**Snap Zones:**
- Left, Right, Top, Bottom (half-screen snapping)
- TopLeft, TopRight, BottomLeft, BottomRight (quarter-screen)
- Center (centered 80% window)

**Example:**
```rust
use multiwindow::layouts::{Layout, SnapZone};

// Apply side-by-side layout to two windows
let window_ids = vec![window1_id, window2_id];
integration::apply_layout(Layout::SideBySide, &window_ids);

// Snap window to left half
integration::snap_to_zone(window_id, SnapZone::Left);

// Apply 2×2 grid to 4 windows
let window_ids = vec![w1, w2, w3, w4];
integration::apply_layout(Layout::Grid2x2, &window_ids);
```

### Animations (`animations.rs`)

Smooth window transitions using various easing functions.

**Animation Types:**
- Move - Translate window position
- Resize - Change window dimensions
- Fade - Opacity transitions
- Minimize - Slide to taskbar
- Maximize - Expand to fullscreen
- Restore - Return to original size

**Easing Functions:**
- Linear - Constant speed
- EaseIn - Slow start, fast end
- EaseOut - Fast start, slow end
- EaseInOut - Slow start and end, fast middle
- Bounce - Bouncy spring effect
- Elastic - Elastic snap effect

**Animation Presets:**
- Window open: 200ms fade in + scale
- Window close: 150ms fade out + scale
- Minimize: 250ms slide down
- Maximize: 200ms expand
- Move: 100ms smooth drag
- Resize: 150ms smooth resize
- Snap: 180ms quick snap

**Example:**
```rust
use multiwindow::animations::{Animation, AnimationPresets};

// Create move animation
let mut anim = Animation::new_move(
    window_id,
    100, 100,  // from (x, y)
    500, 300,  // to (x, y)
    AnimationPresets::MOVE_DURATION,
);

// Update animation with delta time
let finished = anim.update(16); // 16ms frame time

// Get current interpolated position
let current_x = anim.get_x();
let current_y = anim.get_y();
```

### Gestures (`gestures.rs`)

Touch and trackpad gesture recognition for window control.

**Supported Gestures:**
- `Tap` - Single touch/click
- `DoubleTap` - Quick double touch
- `LongPress` - Touch and hold
- `SwipeLeft/Right/Up/Down` - Single finger swipe
- `PinchIn/Out` - Two finger pinch
- `TwoFingerDrag` - Two finger pan
- `ThreeFingerSwipeLeft/Right/Up/Down` - Three finger swipe
- `FourFingerTap` - Four finger tap

**Gesture to Action Mapping:**
- Three-finger swipe left → Next window
- Three-finger swipe right → Previous window
- Three-finger swipe up → Show all windows
- Three-finger swipe down → Show desktop
- Four-finger tap → Show all windows
- Swipe left → Snap window left
- Swipe right → Snap window right
- Double tap → Maximize window

**Example:**
```rust
use multiwindow::gestures::{Gesture, GestureAction};

// Touch down event
gestures::touch_down(touch_id, x, y, timestamp_ms);

// Touch move event
gestures::touch_move(touch_id, new_x, new_y, timestamp_ms);

// Touch up event (returns recognized gesture)
if let Some(gesture) = gestures::touch_up(touch_id, timestamp_ms) {
    let action = gestures::map_gesture_to_action(gesture);
    integration::handle_gesture(gesture);
}
```

### Hotkeys (`hotkeys.rs`)

Keyboard shortcut bindings for window operations.

**Default Windows-Style Hotkeys:**
- `Win+Left` - Snap window left
- `Win+Right` - Snap window right
- `Win+Up` - Maximize window
- `Win+Down` - Minimize window
- `Win+D` - Show desktop
- `Win+Tab` - Show all windows
- `Alt+F4` - Close window
- `F11` - Toggle fullscreen
- `Alt+Tab` - Next window
- `Win+1-9` - Switch to workspace 1-9

**Example:**
```rust
use multiwindow::hotkeys::{Modifiers, WindowAction};

// Register custom hotkey (Ctrl+Shift+T for tile)
hotkeys::register_hotkey(
    Modifiers { ctrl: true, shift: true, alt: false, super_key: false },
    84, // 'T' key
    WindowAction::TileWindows,
);

// Process key press
if let Some(action) = hotkeys::process_key(modifiers, key_code) {
    integration::handle_hotkey(modifiers, key_code);
}
```

### Integration (`integration.rs`)

Unified API coordinating all subsystems.

**Window Modes:**
- `Freeform` - Traditional overlapping windows
- `Split` - Split-screen mode
- `Pip` - Picture-in-picture overlay
- `Fullscreen` - Single fullscreen app
- `Overview` - Multi-window grid overview

**High-Level API:**
```rust
use multiwindow::integration;

// Apply layout
integration::apply_layout(Layout::Grid2x2, &window_ids);

// Snap window to zone
integration::snap_to_zone(window_id, SnapZone::TopLeft);

// Handle gesture
integration::handle_gesture(Gesture::ThreeFingerSwipeUp);

// Handle hotkey
integration::handle_hotkey(modifiers, key_code);
```

## Usage Examples

### Example 1: Create Split-Screen Session

```rust
use multiwindow::{split_screen, SplitOrientation, SplitRatio};

// Start split-screen with browser and terminal
let session = split_screen::create_split(
    browser_app_id,
    terminal_app_id,
    SplitOrientation::Vertical,
    SplitRatio::TwoThirdsOneThird,
).expect("Failed to create split");

// User can drag divider to adjust ratio
split_screen::adjust_divider(session, 75);
```

### Example 2: Picture-in-Picture Video

```rust
use multiwindow::pip;

// Create PIP window for video player
let pip = pip::create_pip(
    video_player_id,
    1600, 900,
    320, 180,
    16, 9,  // 16:9 aspect ratio
).expect("Too many PIP windows");

// Snap to bottom-right corner with 16px margin
pip::snap_to_corner(pip, 2, 16);

// Make semi-transparent
pip::set_opacity(pip, 230);
```

### Example 3: Apply Grid Layout

```rust
use multiwindow::{integration, Layout};

// Get all open windows
let windows = freeform::get_windows();
let window_ids: Vec<u32> = windows.iter().map(|w| w.id).collect();

// Apply 2×2 grid layout
if window_ids.len() <= 4 {
    integration::apply_layout(Layout::Grid2x2, &window_ids);
} else {
    integration::apply_layout(Layout::Grid3x3, &window_ids);
}
```

### Example 4: Gesture-Based Window Management

```rust
use multiwindow::gestures;

// User performs three-finger swipe up
gestures::touch_down(0, 500, 800, timestamp);
gestures::touch_down(1, 600, 800, timestamp);
gestures::touch_down(2, 700, 800, timestamp);

// All fingers move up
gestures::touch_move(0, 500, 400, timestamp + 100);
gestures::touch_move(1, 600, 400, timestamp + 100);
gestures::touch_move(2, 700, 400, timestamp + 100);

// Release triggers "show all windows" overview
if let Some(gesture) = gestures::touch_up(0, timestamp + 200) {
    integration::handle_gesture(gesture); // Shows grid of all windows
}
```

### Example 5: Hotkey Window Snapping

```rust
use multiwindow::{hotkeys, Modifiers};

// User presses Win+Left
let modifiers = Modifiers::super_key();
let key_code = 37; // Left arrow

if let Some(action) = hotkeys::process_key(modifiers, key_code) {
    // Window snaps to left half of screen
    integration::handle_hotkey(modifiers, key_code);
}
```

## Configuration

### Screen Configuration

```rust
use multiwindow::layouts::{ScreenConfig, set_screen_config};

// Configure for 4K display with custom taskbar
let config = ScreenConfig {
    width: 3840,
    height: 2160,
    taskbar_height: 64,
    margin: 12,
};
set_screen_config(config);
```

### Animation Settings

```rust
// Disable animations for performance
// (done through integration module)
// Would need to add public API:
// integration::set_animations_enabled(false);
```

## Performance Considerations

- **Split-screen**: Max 4 concurrent sessions to avoid compositor overhead
- **PIP**: Max 3 windows to maintain performance
- **Animations**: Use EaseOut for most UI transitions (feels responsive)
- **Gestures**: Touch events processed in real-time, use buffering for multi-touch
- **Layouts**: Pre-calculated rects avoid runtime geometry computations

## Platform Integration

The multiwindow subsystem integrates with other Genesis OS modules:

- **Display**: Framebuffer rendering for window contents
- **Input**: Keyboard and touch event processing
- **Process**: Per-window app sandboxing
- **IPC**: Window event notifications
- **Compositor**: Hardware-accelerated window composition

## Future Enhancements

Potential additions to the multiwindow subsystem:

1. **Virtual Desktops** - Multiple workspaces with window persistence
2. **Window Previews** - Thumbnail generation for overview mode
3. **App Exposé** - Show all windows of a single app
4. **Smart Snapping** - Magnetic snap zones for easier window arrangement
5. **Window Groups** - Tab groups and parent-child window relationships
6. **Persistence** - Save and restore window layouts across reboots
7. **Multi-Monitor** - Extended window management across multiple displays
8. **Touch Gestures** - Additional gesture types (rotation, multi-finger tap)
9. **Accessibility** - Screen reader support, high contrast window borders
10. **Performance** - GPU-accelerated window composition, lazy rendering

## API Reference

See individual module documentation:
- [split_screen.rs](./split_screen.rs)
- [pip.rs](./pip.rs)
- [freeform.rs](./freeform.rs)
- [layouts.rs](./layouts.rs)
- [animations.rs](./animations.rs)
- [gestures.rs](./gestures.rs)
- [hotkeys.rs](./hotkeys.rs)
- [integration.rs](./integration.rs)

## Testing

Manual testing checklist:

- [ ] Split-screen: Create, adjust divider, swap apps, end session
- [ ] PIP: Create, move, resize, snap to corners, adjust opacity, close
- [ ] Freeform: Create, move, resize, maximize, minimize, restore, cascade, tile
- [ ] Layouts: Apply each of 9 layout types
- [ ] Animations: Verify smooth transitions for move/resize/fade
- [ ] Gestures: Test all 14 gesture types
- [ ] Hotkeys: Test default Windows-style bindings
- [ ] Integration: Test mode switching, gesture handling, hotkey handling

## License

Part of the Genesis OS kernel.
Copyright © 2026 Hoags Inc.
