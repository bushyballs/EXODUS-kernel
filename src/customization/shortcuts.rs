use crate::sync::Mutex;
/// Keyboard shortcuts manager for Genesis
///
/// Global hotkeys, per-app shortcuts, macro recording,
/// conflict detection and resolution, shortcut chords,
/// remapping and layered keymaps.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Key and modifier definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Super,
    Hyper,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ShortcutScope {
    Global,
    PerApp,
    Desktop,
    LockScreen,
    Compositor,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ShortcutAction {
    LaunchApp,
    CloseWindow,
    MinimizeWindow,
    MaximizeWindow,
    SwitchWorkspace,
    ScreenCapture,
    ToggleSearch,
    LockScreen,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    BrightnessUp,
    BrightnessDown,
    PlayPause,
    NextTrack,
    PrevTrack,
    OpenTerminal,
    OpenFileManager,
    RunMacro,
    Custom,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ConflictResolution {
    KeepExisting,
    ReplaceExisting,
    DisableBoth,
    AddAsChord,
}

#[derive(Clone, Copy, PartialEq)]
pub enum MacroState {
    Idle,
    Recording,
    Playing,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct KeyCombo {
    scancode: u16,
    modifiers: u8, // bitfield: bit0=Ctrl, bit1=Alt, bit2=Shift, bit3=Super, bit4=Hyper
    chord_scancode: u16, // 0 = no chord (single combo), >0 = second key of chord
    chord_modifiers: u8,
}

impl KeyCombo {
    fn new(scancode: u16, mods: u8) -> Self {
        KeyCombo {
            scancode,
            modifiers: mods,
            chord_scancode: 0,
            chord_modifiers: 0,
        }
    }

    fn with_chord(mut self, scancode: u16, mods: u8) -> Self {
        self.chord_scancode = scancode;
        self.chord_modifiers = mods;
        self
    }

    fn matches(&self, scancode: u16, mods: u8) -> bool {
        self.scancode == scancode && self.modifiers == mods
    }

    fn is_chord(&self) -> bool {
        self.chord_scancode != 0
    }

    fn matches_chord(&self, sc1: u16, m1: u8, sc2: u16, m2: u8) -> bool {
        self.scancode == sc1
            && self.modifiers == m1
            && self.chord_scancode == sc2
            && self.chord_modifiers == m2
    }
}

fn modifier_bit(m: Modifier) -> u8 {
    match m {
        Modifier::Ctrl => 0x01,
        Modifier::Alt => 0x02,
        Modifier::Shift => 0x04,
        Modifier::Super => 0x08,
        Modifier::Hyper => 0x10,
    }
}

#[derive(Clone, Copy)]
struct Shortcut {
    id: u32,
    combo: KeyCombo,
    action: ShortcutAction,
    scope: ShortcutScope,
    app_id: u32, // 0 = any app (for Global/Desktop)
    enabled: bool,
    custom_param: u32, // action-specific parameter (e.g., workspace number)
    macro_id: u32,     // linked macro if action == RunMacro
    hit_count: u32,
}

#[derive(Clone, Copy)]
struct MacroStep {
    scancode: u16,
    modifiers: u8,
    is_press: bool, // true = keydown, false = keyup
    delay_ms: u16,
}

struct Macro {
    id: u32,
    name_hash: u64,
    steps: Vec<MacroStep>,
    repeat_count: u16,
    enabled: bool,
    play_count: u32,
}

struct ChordState {
    first_scancode: u16,
    first_modifiers: u8,
    timestamp: u64,
    waiting: bool,
    timeout_ms: u32,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

struct ShortcutManager {
    shortcuts: Vec<Shortcut>,
    macros: Vec<Macro>,
    chord: ChordState,
    macro_state: MacroState,
    recording_steps: Vec<MacroStep>,
    next_shortcut_id: u32,
    next_macro_id: u32,
    max_macro_steps: usize,
    default_chord_timeout_ms: u32,
}

static SHORTCUTS: Mutex<Option<ShortcutManager>> = Mutex::new(None);

impl ShortcutManager {
    fn new() -> Self {
        ShortcutManager {
            shortcuts: Vec::new(),
            macros: Vec::new(),
            chord: ChordState {
                first_scancode: 0,
                first_modifiers: 0,
                timestamp: 0,
                waiting: false,
                timeout_ms: 500,
            },
            macro_state: MacroState::Idle,
            recording_steps: Vec::new(),
            next_shortcut_id: 1,
            next_macro_id: 1,
            max_macro_steps: 128,
            default_chord_timeout_ms: 500,
        }
    }

    fn register(
        &mut self,
        scancode: u16,
        mods: u8,
        action: ShortcutAction,
        scope: ShortcutScope,
        app_id: u32,
    ) -> u32 {
        if self.shortcuts.len() >= 512 {
            return 0;
        }

        let combo = KeyCombo::new(scancode, mods);
        let conflict = self.find_conflict(&combo, scope, app_id);
        if conflict.is_some() {
            return 0;
        }

        let id = self.next_shortcut_id;
        self.next_shortcut_id = self.next_shortcut_id.saturating_add(1);

        let shortcut = Shortcut {
            id,
            combo,
            action,
            scope,
            app_id,
            enabled: true,
            custom_param: 0,
            macro_id: 0,
            hit_count: 0,
        };
        self.shortcuts.push(shortcut);
        id
    }

    fn register_chord(
        &mut self,
        sc1: u16,
        m1: u8,
        sc2: u16,
        m2: u8,
        action: ShortcutAction,
        scope: ShortcutScope,
    ) -> u32 {
        if self.shortcuts.len() >= 512 {
            return 0;
        }

        let combo = KeyCombo::new(sc1, m1).with_chord(sc2, m2);
        let id = self.next_shortcut_id;
        self.next_shortcut_id = self.next_shortcut_id.saturating_add(1);

        let shortcut = Shortcut {
            id,
            combo,
            action,
            scope,
            app_id: 0,
            enabled: true,
            custom_param: 0,
            macro_id: 0,
            hit_count: 0,
        };
        self.shortcuts.push(shortcut);
        id
    }

    fn find_conflict(&self, combo: &KeyCombo, scope: ShortcutScope, app_id: u32) -> Option<u32> {
        for s in &self.shortcuts {
            if !s.enabled {
                continue;
            }
            if s.combo.scancode != combo.scancode || s.combo.modifiers != combo.modifiers {
                continue;
            }

            // Same scope and overlapping app target
            if s.scope == scope {
                if scope == ShortcutScope::Global
                    || s.app_id == app_id
                    || s.app_id == 0
                    || app_id == 0
                {
                    return Some(s.id);
                }
            }
            // Global conflicts with everything
            if s.scope == ShortcutScope::Global || scope == ShortcutScope::Global {
                return Some(s.id);
            }
        }
        None
    }

    fn resolve_conflict(&mut self, existing_id: u32, resolution: ConflictResolution) -> bool {
        match resolution {
            ConflictResolution::KeepExisting => true,
            ConflictResolution::ReplaceExisting => {
                self.shortcuts.retain(|s| s.id != existing_id);
                true
            }
            ConflictResolution::DisableBoth => {
                if let Some(s) = self.shortcuts.iter_mut().find(|s| s.id == existing_id) {
                    s.enabled = false;
                }
                true
            }
            ConflictResolution::AddAsChord => false, // caller must re-register as chord
        }
    }

    fn unregister(&mut self, shortcut_id: u32) -> bool {
        let len_before = self.shortcuts.len();
        self.shortcuts.retain(|s| s.id != shortcut_id);
        self.shortcuts.len() < len_before
    }

    fn set_enabled(&mut self, shortcut_id: u32, enabled: bool) -> bool {
        if let Some(s) = self.shortcuts.iter_mut().find(|s| s.id == shortcut_id) {
            s.enabled = enabled;
            return true;
        }
        false
    }

    fn set_custom_param(&mut self, shortcut_id: u32, param: u32) -> bool {
        if let Some(s) = self.shortcuts.iter_mut().find(|s| s.id == shortcut_id) {
            s.custom_param = param;
            return true;
        }
        false
    }

    fn link_macro(&mut self, shortcut_id: u32, macro_id: u32) -> bool {
        if let Some(s) = self.shortcuts.iter_mut().find(|s| s.id == shortcut_id) {
            s.action = ShortcutAction::RunMacro;
            s.macro_id = macro_id;
            return true;
        }
        false
    }

    fn process_key(&mut self, scancode: u16, mods: u8, timestamp: u64) -> Option<ShortcutAction> {
        // Record if in macro recording mode
        if self.macro_state == MacroState::Recording {
            if self.recording_steps.len() < self.max_macro_steps {
                let delay = if self.recording_steps.is_empty() {
                    0
                } else {
                    ((timestamp
                        - self
                            .recording_steps
                            .last()
                            .map(|s| s.delay_ms as u64)
                            .unwrap_or(0)) as u16)
                        .min(5000)
                };
                self.recording_steps.push(MacroStep {
                    scancode,
                    modifiers: mods,
                    is_press: true,
                    delay_ms: delay,
                });
            }
        }

        // Check if completing a chord
        if self.chord.waiting {
            self.chord.waiting = false;
            if timestamp - self.chord.timestamp <= self.chord.timeout_ms as u64 {
                for s in &mut self.shortcuts {
                    if !s.enabled {
                        continue;
                    }
                    if s.combo.matches_chord(
                        self.chord.first_scancode,
                        self.chord.first_modifiers,
                        scancode,
                        mods,
                    ) {
                        s.hit_count = s.hit_count.saturating_add(1);
                        return Some(s.action);
                    }
                }
            }
        }

        // Check if this starts a chord
        for s in &self.shortcuts {
            if !s.enabled {
                continue;
            }
            if s.combo.is_chord() && s.combo.matches(scancode, mods) {
                self.chord.first_scancode = scancode;
                self.chord.first_modifiers = mods;
                self.chord.timestamp = timestamp;
                self.chord.waiting = true;
                return None;
            }
        }

        // Single-key shortcut match
        for s in &mut self.shortcuts {
            if !s.enabled {
                continue;
            }
            if !s.combo.is_chord() && s.combo.matches(scancode, mods) {
                s.hit_count = s.hit_count.saturating_add(1);
                return Some(s.action);
            }
        }

        None
    }

    fn start_macro_recording(&mut self) {
        self.macro_state = MacroState::Recording;
        self.recording_steps.clear();
    }

    fn stop_macro_recording(&mut self, name_hash: u64) -> u32 {
        if self.macro_state != MacroState::Recording {
            return 0;
        }
        self.macro_state = MacroState::Idle;

        if self.recording_steps.is_empty() {
            return 0;
        }
        if self.macros.len() >= 64 {
            return 0;
        }

        let id = self.next_macro_id;
        self.next_macro_id = self.next_macro_id.saturating_add(1);

        let steps = core::mem::take(&mut self.recording_steps);
        let mac = Macro {
            id,
            name_hash,
            steps,
            repeat_count: 1,
            enabled: true,
            play_count: 0,
        };
        self.macros.push(mac);
        id
    }

    fn play_macro(&mut self, macro_id: u32) -> bool {
        if self.macro_state != MacroState::Idle {
            return false;
        }

        if let Some(mac) = self
            .macros
            .iter_mut()
            .find(|m| m.id == macro_id && m.enabled)
        {
            mac.play_count = mac.play_count.saturating_add(1);
            self.macro_state = MacroState::Playing;
            // In a real kernel, we'd queue the steps to the input subsystem
            // For now, mark as done
            self.macro_state = MacroState::Idle;
            return true;
        }
        false
    }

    fn delete_macro(&mut self, macro_id: u32) -> bool {
        let len_before = self.macros.len();
        self.macros.retain(|m| m.id != macro_id);
        // Unlink from any shortcuts
        for s in &mut self.shortcuts {
            if s.macro_id == macro_id && s.action == ShortcutAction::RunMacro {
                s.action = ShortcutAction::Custom;
                s.macro_id = 0;
            }
        }
        self.macros.len() < len_before
    }

    fn get_shortcuts_for_app(&self, app_id: u32) -> usize {
        self.shortcuts
            .iter()
            .filter(|s| s.app_id == app_id && s.enabled)
            .count()
    }

    fn setup_defaults(&mut self) {
        // Ctrl+Alt+T -> OpenTerminal
        self.register(
            0x14,
            modifier_bit(Modifier::Ctrl) | modifier_bit(Modifier::Alt),
            ShortcutAction::OpenTerminal,
            ShortcutScope::Global,
            0,
        );
        // Super+E -> OpenFileManager
        self.register(
            0x12,
            modifier_bit(Modifier::Super),
            ShortcutAction::OpenFileManager,
            ShortcutScope::Global,
            0,
        );
        // Super+L -> LockScreen
        self.register(
            0x26,
            modifier_bit(Modifier::Super),
            ShortcutAction::LockScreen,
            ShortcutScope::Global,
            0,
        );
        // Print Screen -> ScreenCapture
        self.register(
            0xB7,
            0,
            ShortcutAction::ScreenCapture,
            ShortcutScope::Global,
            0,
        );
        // Super+Space -> ToggleSearch
        self.register(
            0x39,
            modifier_bit(Modifier::Super),
            ShortcutAction::ToggleSearch,
            ShortcutScope::Global,
            0,
        );
        // Alt+F4 -> CloseWindow
        self.register(
            0x3E,
            modifier_bit(Modifier::Alt),
            ShortcutAction::CloseWindow,
            ShortcutScope::Global,
            0,
        );
        // Volume keys (scancodes for multimedia)
        self.register(
            0xAE,
            0,
            ShortcutAction::VolumeDown,
            ShortcutScope::Global,
            0,
        );
        self.register(0xB0, 0, ShortcutAction::VolumeUp, ShortcutScope::Global, 0);
        self.register(
            0xA0,
            0,
            ShortcutAction::VolumeMute,
            ShortcutScope::Global,
            0,
        );
        // Media keys
        self.register(0xA2, 0, ShortcutAction::PlayPause, ShortcutScope::Global, 0);
        self.register(0x99, 0, ShortcutAction::NextTrack, ShortcutScope::Global, 0);
        self.register(0x90, 0, ShortcutAction::PrevTrack, ShortcutScope::Global, 0);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let mut mgr = ShortcutManager::new();
    mgr.setup_defaults();

    let mut guard = SHORTCUTS.lock();
    *guard = Some(mgr);
    serial_println!("    Shortcuts: hotkey manager ready ({} defaults)", 12);
}

pub fn register_shortcut(
    scancode: u16,
    mods: u8,
    action: ShortcutAction,
    scope: ShortcutScope,
    app_id: u32,
) -> u32 {
    let mut guard = SHORTCUTS.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.register(scancode, mods, action, scope, app_id);
    }
    0
}

pub fn register_chord(
    sc1: u16,
    m1: u8,
    sc2: u16,
    m2: u8,
    action: ShortcutAction,
    scope: ShortcutScope,
) -> u32 {
    let mut guard = SHORTCUTS.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.register_chord(sc1, m1, sc2, m2, action, scope);
    }
    0
}

pub fn unregister_shortcut(shortcut_id: u32) -> bool {
    let mut guard = SHORTCUTS.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.unregister(shortcut_id);
    }
    false
}

pub fn process_key_event(scancode: u16, mods: u8, timestamp: u64) -> Option<ShortcutAction> {
    let mut guard = SHORTCUTS.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.process_key(scancode, mods, timestamp);
    }
    None
}

pub fn start_recording() {
    let mut guard = SHORTCUTS.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.start_macro_recording();
    }
}

pub fn stop_recording(name_hash: u64) -> u32 {
    let mut guard = SHORTCUTS.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.stop_macro_recording(name_hash);
    }
    0
}

pub fn play_macro(macro_id: u32) -> bool {
    let mut guard = SHORTCUTS.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.play_macro(macro_id);
    }
    false
}
