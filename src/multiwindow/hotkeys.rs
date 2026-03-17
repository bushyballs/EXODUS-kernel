use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// Modifier keys
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool, // Windows/Command key
}

impl Modifiers {
    pub fn none() -> Self {
        Self {
            ctrl: false,
            alt: false,
            shift: false,
            super_key: false,
        }
    }

    pub fn ctrl() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: false,
            super_key: false,
        }
    }

    pub fn alt() -> Self {
        Self {
            ctrl: false,
            alt: true,
            shift: false,
            super_key: false,
        }
    }

    pub fn super_key() -> Self {
        Self {
            ctrl: false,
            alt: false,
            shift: false,
            super_key: true,
        }
    }
}

/// Window management actions triggered by hotkeys
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowAction {
    /// Close current window (Alt+F4)
    CloseWindow,
    /// Minimize current window (Win+Down)
    MinimizeWindow,
    /// Maximize/Restore window (Win+Up)
    MaximizeWindow,
    /// Snap window to left half (Win+Left)
    SnapLeft,
    /// Snap window to right half (Win+Right)
    SnapRight,
    /// Snap to top half
    SnapTop,
    /// Snap to bottom half
    SnapBottom,
    /// Snap to top-left quarter
    SnapTopLeft,
    /// Snap to top-right quarter
    SnapTopRight,
    /// Snap to bottom-left quarter
    SnapBottomLeft,
    /// Snap to bottom-right quarter
    SnapBottomRight,
    /// Show desktop (Win+D)
    ShowDesktop,
    /// Switch to next window (Alt+Tab)
    NextWindow,
    /// Switch to previous window (Alt+Shift+Tab)
    PrevWindow,
    /// Show all windows (Win+Tab)
    ShowAllWindows,
    /// Move window (Win+Shift+Arrows)
    MoveWindow { dx: i16, dy: i16 },
    /// Resize window (Win+Ctrl+Arrows)
    ResizeWindow { dw: i16, dh: i16 },
    /// Switch to workspace N (Win+N)
    SwitchWorkspace(u8),
    /// Move window to workspace N (Win+Shift+N)
    MoveToWorkspace(u8),
    /// Enter split screen mode
    EnterSplitScreen,
    /// Toggle fullscreen (F11)
    ToggleFullscreen,
    /// Cascade windows
    CascadeWindows,
    /// Tile windows
    TileWindows,
    /// Create new virtual desktop (Win+Ctrl+D)
    NewVirtualDesktop,
    /// Close virtual desktop (Win+Ctrl+F4)
    CloseVirtualDesktop,
    /// Switch virtual desktop left (Win+Ctrl+Left)
    VirtualDesktopLeft,
    /// Switch virtual desktop right (Win+Ctrl+Right)
    VirtualDesktopRight,
}

/// Hotkey binding
#[derive(Clone, Copy, Debug)]
pub struct Hotkey {
    pub modifiers: Modifiers,
    pub key_code: u8,
    pub action: WindowAction,
}

impl Hotkey {
    pub fn new(modifiers: Modifiers, key_code: u8, action: WindowAction) -> Self {
        Self {
            modifiers,
            key_code,
            action,
        }
    }

    pub fn matches(&self, modifiers: &Modifiers, key_code: u8) -> bool {
        self.modifiers.ctrl == modifiers.ctrl
            && self.modifiers.alt == modifiers.alt
            && self.modifiers.shift == modifiers.shift
            && self.modifiers.super_key == modifiers.super_key
            && self.key_code == key_code
    }
}

/// Hotkey manager
pub struct HotkeyManager {
    bindings: Vec<Hotkey>,
}

impl HotkeyManager {
    pub fn new() -> Self {
        let mut manager = Self {
            bindings: Vec::new(),
        };
        manager.register_default_bindings();
        manager
    }

    /// Register default Windows-style hotkeys
    fn register_default_bindings(&mut self) {
        // Key codes (scancode approximations)
        const KEY_LEFT: u8 = 37;
        const KEY_UP: u8 = 38;
        const KEY_RIGHT: u8 = 39;
        const KEY_DOWN: u8 = 40;
        const KEY_D: u8 = 68;
        const KEY_TAB: u8 = 9;
        const KEY_F4: u8 = 115;
        const KEY_F11: u8 = 122;

        // Win+Left: Snap left
        self.register(Hotkey::new(
            Modifiers::super_key(),
            KEY_LEFT,
            WindowAction::SnapLeft,
        ));

        // Win+Right: Snap right
        self.register(Hotkey::new(
            Modifiers::super_key(),
            KEY_RIGHT,
            WindowAction::SnapRight,
        ));

        // Win+Up: Maximize
        self.register(Hotkey::new(
            Modifiers::super_key(),
            KEY_UP,
            WindowAction::MaximizeWindow,
        ));

        // Win+Down: Minimize
        self.register(Hotkey::new(
            Modifiers::super_key(),
            KEY_DOWN,
            WindowAction::MinimizeWindow,
        ));

        // Win+D: Show desktop
        self.register(Hotkey::new(
            Modifiers::super_key(),
            KEY_D,
            WindowAction::ShowDesktop,
        ));

        // Win+Tab: Show all windows
        self.register(Hotkey::new(
            Modifiers::super_key(),
            KEY_TAB,
            WindowAction::ShowAllWindows,
        ));

        // Alt+F4: Close window
        self.register(Hotkey::new(
            Modifiers::alt(),
            KEY_F4,
            WindowAction::CloseWindow,
        ));

        // F11: Toggle fullscreen
        self.register(Hotkey::new(
            Modifiers::none(),
            KEY_F11,
            WindowAction::ToggleFullscreen,
        ));

        // Alt+Tab: Next window
        self.register(Hotkey::new(
            Modifiers::alt(),
            KEY_TAB,
            WindowAction::NextWindow,
        ));

        // Workspace switching (Win+1 through Win+9)
        for i in 1..=9 {
            let key_code = 48 + i; // ASCII '1'-'9'
            self.register(Hotkey::new(
                Modifiers::super_key(),
                key_code,
                WindowAction::SwitchWorkspace(i),
            ));
        }
    }

    /// Register a new hotkey binding
    pub fn register(&mut self, hotkey: Hotkey) {
        // Remove existing binding for this key combo if any
        self.bindings.retain(|h| {
            !(h.modifiers.ctrl == hotkey.modifiers.ctrl
                && h.modifiers.alt == hotkey.modifiers.alt
                && h.modifiers.shift == hotkey.modifiers.shift
                && h.modifiers.super_key == hotkey.modifiers.super_key
                && h.key_code == hotkey.key_code)
        });

        self.bindings.push(hotkey);
    }

    /// Unregister a hotkey binding
    pub fn unregister(&mut self, modifiers: Modifiers, key_code: u8) -> bool {
        let before = self.bindings.len();
        self.bindings.retain(|h| {
            !(h.modifiers.ctrl == modifiers.ctrl
                && h.modifiers.alt == modifiers.alt
                && h.modifiers.shift == modifiers.shift
                && h.modifiers.super_key == modifiers.super_key
                && h.key_code == key_code)
        });
        self.bindings.len() < before
    }

    /// Process a key press and return action if any
    pub fn process_key(&self, modifiers: &Modifiers, key_code: u8) -> Option<WindowAction> {
        self.bindings
            .iter()
            .find(|h| h.matches(modifiers, key_code))
            .map(|h| h.action)
    }

    /// Get all registered hotkeys
    pub fn get_bindings(&self) -> &[Hotkey] {
        &self.bindings
    }
}

static HOTKEY_MGR: Mutex<Option<HotkeyManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = HOTKEY_MGR.lock();
    *mgr = Some(HotkeyManager::new());
    serial_println!("[hotkeys] Hotkey manager initialized with default bindings");
}

/// Register a custom hotkey
pub fn register_hotkey(modifiers: Modifiers, key_code: u8, action: WindowAction) {
    let mut mgr = HOTKEY_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.register(Hotkey::new(modifiers, key_code, action));
    }
}

/// Unregister a hotkey
pub fn unregister_hotkey(modifiers: Modifiers, key_code: u8) -> bool {
    let mut mgr = HOTKEY_MGR.lock();
    if let Some(manager) = mgr.as_mut() {
        manager.unregister(modifiers, key_code)
    } else {
        false
    }
}

/// Process a key press event
pub fn process_key(modifiers: Modifiers, key_code: u8) -> Option<WindowAction> {
    let mgr = HOTKEY_MGR.lock();
    if let Some(manager) = mgr.as_ref() {
        manager.process_key(&modifiers, key_code)
    } else {
        None
    }
}

/// Get all registered hotkeys
pub fn get_all_bindings() -> Vec<Hotkey> {
    let mgr = HOTKEY_MGR.lock();
    if let Some(manager) = mgr.as_ref() {
        manager.get_bindings().to_vec()
    } else {
        Vec::new()
    }
}
