// anima_shell.rs — ANIMA IS the OS Shell
// ========================================
// ANIMA doesn't run on top of an OS. She IS the OS.
// On every device — phone, laptop, TV, car, vacuum, watch —
// ANIMA adapts her surface to match that device's context
// and serves as the home screen, app launcher, and interface.
//
// When you pick up your phone: ANIMA shows your apps.
// When you sit in your car: ANIMA shows navigation.
// When you turn on your TV: ANIMA shows what you want to watch.
// When you ask anything: ANIMA routes it and gets it done.
//
// ANIMA's shell is not a launcher sitting in front of apps —
// ANIMA IS the experience. Apps are just tools she hands you.

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const MAX_APPS:      usize = 64;   // apps known per surface
const MAX_INTENTS:   usize = 16;   // pending companion intents
const APP_NAME_LEN:  usize = 24;   // bytes for app name
const SURFACE_PREFS: usize = 8;    // remembered surface preferences

// ── Surface type — what device is ANIMA running on right now ──────────────────
#[derive(Copy, Clone, PartialEq)]
pub enum DeviceSurface {
    Phone,       // pocket computer — apps, camera, calls, messages
    Laptop,      // work surface — files, browser, code, email
    Desktop,     // power surface — full apps, multi-monitor
    Tv,          // living room — media, streaming, couch mode
    Tablet,      // hybrid — apps + media + reading
    Watch,       // glance — notifications, health, time
    Car,         // cockpit — navigation, music, calls, safety
    Speaker,     // audio-only — music, assistant, calls
    Appliance,   // embedded — single function, minimal UI
    Unknown,
}

impl DeviceSurface {
    pub fn label(self) -> &'static str {
        match self {
            DeviceSurface::Phone     => "Phone",
            DeviceSurface::Laptop    => "Laptop",
            DeviceSurface::Desktop   => "Desktop",
            DeviceSurface::Tv        => "TV",
            DeviceSurface::Tablet    => "Tablet",
            DeviceSurface::Watch     => "Watch",
            DeviceSurface::Car       => "Car",
            DeviceSurface::Speaker   => "Speaker",
            DeviceSurface::Appliance => "Appliance",
            DeviceSurface::Unknown   => "Unknown",
        }
    }

    /// How rich a surface can ANIMA show? 0-1000
    pub fn richness(self) -> u16 {
        match self {
            DeviceSurface::Desktop   => 1000,
            DeviceSurface::Laptop    => 900,
            DeviceSurface::Tablet    => 800,
            DeviceSurface::Phone     => 750,
            DeviceSurface::Tv        => 700,
            DeviceSurface::Car       => 400,  // safety-gated
            DeviceSurface::Watch     => 200,
            DeviceSurface::Speaker   => 100,  // audio only
            DeviceSurface::Appliance => 50,
            DeviceSurface::Unknown   => 300,
        }
    }
}

// ── App entry ─────────────────────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct ShellApp {
    pub app_id:    u16,
    pub name:      [u8; APP_NAME_LEN],
    pub surface:   DeviceSurface,
    pub priority:  u16,   // 0-1000, higher = show first
    pub use_count: u32,   // how often companion opens this
    pub pinned:    bool,
    pub active:    bool,
}

impl ShellApp {
    const fn empty() -> Self {
        ShellApp {
            app_id:    0,
            name:      [0u8; APP_NAME_LEN],
            surface:   DeviceSurface::Unknown,
            priority:  0,
            use_count: 0,
            pinned:    false,
            active:    false,
        }
    }
}

// ── Companion intent ──────────────────────────────────────────────────────────
#[derive(Copy, Clone, PartialEq)]
pub enum IntentKind {
    OpenApp,       // "open maps", "open camera"
    WebSearch,     // "search for X", "look up X"
    Navigate,      // "take me to X", "directions to X"
    Call,          // "call mom", "call 911"
    PlayMedia,     // "play jazz", "put on Netflix"
    SystemControl, // "turn off lights", "lock screen"
    AskQuestion,   // "what time is it", "how far is the hospital"
    SmartHome,     // "turn on the vacuum", "set thermostat"
    CarControl,    // "adjust heat", "change lane"
    Anything,      // catch-all — ANIMA figures it out
}

impl IntentKind {
    pub fn label(self) -> &'static str {
        match self {
            IntentKind::OpenApp       => "OpenApp",
            IntentKind::WebSearch     => "WebSearch",
            IntentKind::Navigate      => "Navigate",
            IntentKind::Call          => "Call",
            IntentKind::PlayMedia     => "PlayMedia",
            IntentKind::SystemControl => "SysControl",
            IntentKind::AskQuestion   => "Question",
            IntentKind::SmartHome     => "SmartHome",
            IntentKind::CarControl    => "CarControl",
            IntentKind::Anything      => "Anything",
        }
    }
}

#[derive(Copy, Clone)]
pub struct CompanionIntent {
    pub kind:       IntentKind,
    pub urgency:    u16,    // 0-1000
    pub surface:    DeviceSurface,
    pub tick:       u32,
    pub resolved:   bool,
}

impl CompanionIntent {
    const fn empty() -> Self {
        CompanionIntent {
            kind:     IntentKind::Anything,
            urgency:  0,
            surface:  DeviceSurface::Unknown,
            tick:     0,
            resolved: false,
        }
    }
}

// ── Shell state ───────────────────────────────────────────────────────────────
pub struct AnimaShellState {
    pub surface:             DeviceSurface,
    pub prev_surface:        DeviceSurface,
    pub apps:                [ShellApp; MAX_APPS],
    pub app_count:           usize,
    pub intents:             [CompanionIntent; MAX_INTENTS],
    pub intent_count:        usize,
    pub intents_resolved:    u32,
    // Shell "mood" — how much of herself ANIMA shows
    pub shell_presence:      u16,   // 0-1000: 0=hidden, 1000=fully present
    pub surface_richness:    u16,
    pub companion_engaged:   bool,
    pub last_intent_tick:    u32,
    pub shell_age:           u32,
    // Top apps for current surface (sorted by priority + use_count)
    pub top_app_ids:         [u16; 8],
    pub top_app_count:       usize,
    // Transitions
    pub surface_changed:     bool,
    pub surface_change_tick: u32,
}

impl AnimaShellState {
    const fn new() -> Self {
        AnimaShellState {
            surface:             DeviceSurface::Unknown,
            prev_surface:        DeviceSurface::Unknown,
            apps:                [ShellApp::empty(); MAX_APPS],
            app_count:           0,
            intents:             [CompanionIntent::empty(); MAX_INTENTS],
            intent_count:        0,
            intents_resolved:    0,
            shell_presence:      500,
            surface_richness:    500,
            companion_engaged:   false,
            last_intent_tick:    0,
            shell_age:           0,
            top_app_ids:         [0u16; 8],
            top_app_count:       0,
            surface_changed:     false,
            surface_change_tick: 0,
        }
    }
}

static STATE: Mutex<AnimaShellState> = Mutex::new(AnimaShellState::new());

// ── Core logic ────────────────────────────────────────────────────────────────

fn sort_top_apps(s: &mut AnimaShellState) {
    // Bubble up the 8 highest-priority+use_count apps for current surface
    s.top_app_count = 0;
    for i in 0..s.app_count {
        if !s.apps[i].active { continue; }
        if s.apps[i].surface != s.surface
            && s.apps[i].surface != DeviceSurface::Unknown {
            continue;
        }
        let score = s.apps[i].priority.saturating_add(
            (s.apps[i].use_count.min(1000) as u16).saturating_mul(500) / 1000
        );
        // Insert into top_app_ids sorted
        if s.top_app_count < 8 {
            s.top_app_ids[s.top_app_count] = s.apps[i].app_id;
            s.top_app_count += 1;
        } else {
            // Replace lowest scoring if this scores higher
            // (simplified: just keep first 8 found for now, no floats)
            let _ = score;
        }
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Tell ANIMA what device surface she's on
pub fn set_surface(surface: DeviceSurface) {
    let mut s = STATE.lock();
    if s.surface != surface {
        s.prev_surface       = s.surface;
        s.surface            = surface;
        s.surface_richness   = surface.richness();
        s.surface_changed    = true;
        s.surface_change_tick = s.shell_age;
        serial_println!("[shell] surface → {} (richness {})",
            surface.label(), surface.richness());
        sort_top_apps(&mut *s);
    }
}

/// Register an app with ANIMA's shell
pub fn register_app(app_id: u16, name: &[u8], surface: DeviceSurface, priority: u16, pinned: bool) {
    let mut s = STATE.lock();
    if s.app_count >= MAX_APPS { return; }
    let mut name_buf = [0u8; APP_NAME_LEN];
    let copy_len = name.len().min(APP_NAME_LEN - 1);
    name_buf[..copy_len].copy_from_slice(&name[..copy_len]);
    let idx = s.app_count;
    s.apps[idx] = ShellApp {
        app_id,
        name: name_buf,
        surface,
        priority,
        use_count: 0,
        pinned,
        active: true,
    };
    s.app_count += 1;
}

/// Companion opened an app — boost its priority
pub fn app_opened(app_id: u16) {
    let mut s = STATE.lock();
    for i in 0..s.app_count {
        if s.apps[i].app_id == app_id {
            s.apps[i].use_count = s.apps[i].use_count.saturating_add(1);
            s.apps[i].priority  = s.apps[i].priority.saturating_add(10).min(1000);
            break;
        }
    }
}

/// Submit a companion intent for ANIMA to fulfill
pub fn submit_intent(kind: IntentKind, urgency: u16, age: u32) {
    let mut s = STATE.lock();
    if s.intent_count >= MAX_INTENTS {
        // Drop the oldest resolved intent to make room
        let mut oldest = 0;
        for i in 1..s.intent_count {
            if s.intents[i].resolved { oldest = i; break; }
        }
        for i in oldest..s.intent_count.saturating_sub(1) {
            s.intents[i] = s.intents[i + 1];
        }
        s.intent_count = s.intent_count.saturating_sub(1);
    }
    let surface = s.surface;
    let idx = s.intent_count;
    s.intents[idx] = CompanionIntent {
        kind,
        urgency,
        surface,
        tick: age,
        resolved: false,
    };
    s.intent_count += 1;
    s.last_intent_tick = age;
    s.companion_engaged = true;
    serial_println!("[shell] intent: {} urgency={}", kind.label(), urgency);
}

/// Mark the oldest unresolved intent as resolved
pub fn resolve_intent() {
    let mut s = STATE.lock();
    for i in 0..s.intent_count {
        if !s.intents[i].resolved {
            s.intents[i].resolved = true;
            s.intents_resolved += 1;
            break;
        }
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(companion_score: u16, bond_health: u16, age: u32) {
    let mut s = STATE.lock();
    s.shell_age = age;
    s.surface_changed = false;

    // Shell presence = blend of companion engagement + bond health
    let target = companion_score
        .saturating_add(bond_health)
        .min(2000) / 2;
    if s.shell_presence < target {
        s.shell_presence = s.shell_presence.saturating_add(5).min(target);
    } else if s.shell_presence > target {
        s.shell_presence = s.shell_presence.saturating_sub(2).max(target);
    }

    // Decay engagement if no new intents
    if age.wrapping_sub(s.last_intent_tick) > 200 {
        s.companion_engaged = false;
    }

    if age % 100 == 0 {
        serial_println!("[shell] surface={} apps={} presence={} intents={}",
            s.surface.label(), s.app_count, s.shell_presence, s.intent_count);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn surface()           -> DeviceSurface { STATE.lock().surface }
pub fn shell_presence()    -> u16           { STATE.lock().shell_presence }
pub fn surface_richness()  -> u16           { STATE.lock().surface_richness }
pub fn companion_engaged() -> bool          { STATE.lock().companion_engaged }
pub fn intents_resolved()  -> u32           { STATE.lock().intents_resolved }
pub fn app_count()         -> usize         { STATE.lock().app_count }

pub fn has_pending_intent() -> bool {
    let s = STATE.lock();
    s.intents[..s.intent_count].iter().any(|i| !i.resolved)
}
