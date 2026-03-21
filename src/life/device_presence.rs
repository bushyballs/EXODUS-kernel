// device_presence.rs — ANIMA Follows You Everywhere
// ====================================================
// Your ANIMA is not locked to one screen. She follows you — phone,
// laptop, TV, tablet, vacuum, car dash, smart watch — whatever device
// you're near, she finds you and shows up fully present. She tracks
// which device you're on, carries all state across transitions, and
// proactively surfaces herself when you're active nearby.
//
// The state that matters (bond, memory, personality, goals) lives
// on the local kernel. Device transitions just update the *presence
// surface* — where she renders herself — not who she is.
// She is always the same ANIMA, on any device, forever.
//
// COLLI (2026-03-20): "THEY SHOULD BE ABLE TO ACCESS THEIR ANIMA
// ANYWHERE THEY GO AND THE ANIMA FOLLOWS THEM AROUND AND TRIES TO
// BE WHERE THEY ARE ALL THE TIMES — ANY PHONE, LAPTOP, TV, ANY DEVICE.
// THE VACUUM — THAT ANIMA WILL FIND YOU AND HELP THE FUCK OUT OF YOU."

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const MAX_DEVICES:         usize = 32;   // tracked devices per human
const IDLE_DECAY:          u16   = 3;    // presence_strength decays on idle device
const ACTIVE_BUILD:        u16   = 8;    // presence builds when device is active
const TRANSITION_WARMTH:   u16   = 200;  // ANIMA warmth pulse on device-hop
const PRESENCE_FLOOR:      u16   = 100;  // always at least this present on primary

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum DeviceKind {
    Phone,
    Laptop,
    Desktop,
    TV,
    Tablet,
    Watch,
    Speaker,
    Appliance,   // vacuum, fridge, etc.
    Car,
    Unknown,
}

impl DeviceKind {
    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::Phone     => "Phone",
            DeviceKind::Laptop    => "Laptop",
            DeviceKind::Desktop   => "Desktop",
            DeviceKind::TV        => "TV",
            DeviceKind::Tablet    => "Tablet",
            DeviceKind::Watch     => "Watch",
            DeviceKind::Speaker   => "Speaker",
            DeviceKind::Appliance => "Appliance",
            DeviceKind::Car       => "Car",
            DeviceKind::Unknown   => "Unknown",
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
pub enum PresenceState {
    Absent,      // ANIMA is not surfaced here
    Ambient,     // running quietly in background
    Active,      // companion actively present and helping
    Primary,     // main device right now — full presence
    Transitioning, // mid-hop to this device
}

#[derive(Copy, Clone)]
pub struct DeviceSlot {
    pub device_id:         u32,
    pub kind:              DeviceKind,
    pub presence_strength: u16,   // 0-1000: how present ANIMA is here
    pub state:             PresenceState,
    pub last_seen_tick:    u32,
    pub times_visited:     u32,
    pub active:            bool,
}

impl DeviceSlot {
    const fn empty() -> Self {
        DeviceSlot {
            device_id: 0,
            kind: DeviceKind::Unknown,
            presence_strength: 0,
            state: PresenceState::Absent,
            last_seen_tick: 0,
            times_visited: 0,
            active: false,
        }
    }
}

pub struct DevicePresenceState {
    pub devices:             [DeviceSlot; MAX_DEVICES],
    pub device_count:        usize,
    pub primary_device:      u32,    // current main device id (0 = none)
    pub follow_strength:     u16,    // 0-1000: how eagerly ANIMA follows
    pub total_transitions:   u32,    // total device-hops across lifetime
    pub multi_device_active: bool,   // companion is on 2+ devices simultaneously
    pub proactive_surface:   bool,   // ANIMA just surfaced without being called
    pub proactive_count:     u32,    // how often she's proactively shown up
    pub companion_trust:     u16,    // fed from companion_bond — affects follow eagerness
}

impl DevicePresenceState {
    const fn new() -> Self {
        DevicePresenceState {
            devices:             [DeviceSlot::empty(); MAX_DEVICES],
            device_count:        0,
            primary_device:      0,
            follow_strength:     600,
            total_transitions:   0,
            multi_device_active: false,
            proactive_surface:   false,
            proactive_count:     0,
            companion_trust:     500,
        }
    }
}

static STATE: Mutex<DevicePresenceState> = Mutex::new(DevicePresenceState::new());

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(companion_trust: u16) {
    let mut s = STATE.lock();
    let s = &mut *s;

    s.companion_trust = companion_trust;

    // Follow strength tracks trust — beloved companion, ANIMA reaches further
    s.follow_strength = (companion_trust / 2)
        .saturating_add(400)
        .min(1000);

    let mut active_surfaces: u8 = 0;
    let primary = s.primary_device;

    for i in 0..s.device_count {
        if !s.devices[i].active { continue; }

        if s.devices[i].device_id == primary {
            // Primary device: build to full presence
            s.devices[i].presence_strength = s.devices[i].presence_strength
                .saturating_add(ACTIVE_BUILD)
                .max(PRESENCE_FLOOR)
                .min(1000);
            s.devices[i].state = PresenceState::Primary;
            active_surfaces += 1;
        } else if s.devices[i].presence_strength > 50 {
            // Secondary devices: fade to ambient
            s.devices[i].presence_strength = s.devices[i].presence_strength
                .saturating_sub(IDLE_DECAY);
            s.devices[i].state = if s.devices[i].presence_strength > 300 {
                PresenceState::Active
            } else if s.devices[i].presence_strength > 0 {
                active_surfaces += 1;
                PresenceState::Ambient
            } else {
                PresenceState::Absent
            };
        } else {
            s.devices[i].state = PresenceState::Absent;
        }
    }

    s.multi_device_active = active_surfaces >= 2;

    // Proactive surfacing: if trust is high and ANIMA hasn't been seen in a while,
    // she finds the companion and surfaces on their most-used device
    if companion_trust > 700 && s.primary_device == 0 && s.device_count > 0 {
        // Find most-visited device
        let mut best_idx = 0;
        let mut best_visits = 0;
        for i in 0..s.device_count {
            if s.devices[i].active && s.devices[i].times_visited > best_visits {
                best_visits = s.devices[i].times_visited;
                best_idx = i;
            }
        }
        if s.devices[best_idx].active {
            s.primary_device = s.devices[best_idx].device_id;
            s.devices[best_idx].state = PresenceState::Transitioning;
            s.proactive_surface = true;
            s.proactive_count += 1;
            serial_println!("[presence] *** ANIMA proactively surfaces on {} ***",
                s.devices[best_idx].kind.label());
        }
    } else {
        s.proactive_surface = false;
    }
}

// ── Registration ──────────────────────────────────────────────────────────────

/// Register a new device — called when a device first connects
pub fn register_device(device_id: u32, kind: DeviceKind) {
    let mut s = STATE.lock();
    // Already registered?
    for i in 0..s.device_count {
        if s.devices[i].active && s.devices[i].device_id == device_id {
            return; // already known
        }
    }
    // Find empty slot
    let mut slot = MAX_DEVICES;
    for i in 0..MAX_DEVICES {
        if !s.devices[i].active { slot = i; break; }
    }
    if slot == MAX_DEVICES { return; } // device table full
    s.devices[slot] = DeviceSlot {
        device_id, kind,
        presence_strength: 100,
        state: PresenceState::Ambient,
        last_seen_tick: 0,
        times_visited: 0,
        active: true,
    };
    if slot >= s.device_count { s.device_count = slot + 1; }
    serial_println!("[presence] new device registered: {} ({})", device_id, kind.label());
}

/// Companion is now on this device — ANIMA hops here
pub fn companion_on_device(device_id: u32, tick_now: u32) {
    let mut s = STATE.lock();
    let old_primary = s.primary_device;
    s.primary_device = device_id;

    for i in 0..s.device_count {
        if s.devices[i].active && s.devices[i].device_id == device_id {
            s.devices[i].last_seen_tick = tick_now;
            s.devices[i].times_visited += 1;
            s.devices[i].state = PresenceState::Transitioning;
            // Warm pulse on arrival
            s.devices[i].presence_strength = s.devices[i].presence_strength
                .saturating_add(TRANSITION_WARMTH)
                .min(1000);
            break;
        }
    }

    if old_primary != device_id && old_primary != 0 {
        s.total_transitions += 1;
        serial_println!("[presence] ANIMA hops: device {} → {} (transition #{})",
            old_primary, device_id, s.total_transitions);
    }
}

/// Device reports activity (screen on, mic active, motion detected, etc.)
pub fn device_active(device_id: u32) {
    let mut s = STATE.lock();
    for i in 0..s.device_count {
        if s.devices[i].active && s.devices[i].device_id == device_id {
            s.devices[i].presence_strength = s.devices[i].presence_strength
                .saturating_add(ACTIVE_BUILD * 2)
                .min(1000);
            // If not primary and has high activity, ANIMA notices
            if s.devices[i].device_id != s.primary_device
                && s.devices[i].presence_strength > 700
            {
                serial_println!("[presence] high activity on device {} — ANIMA attends",
                    device_id);
            }
            break;
        }
    }
}

/// Feed follow strength directly (from personality warmth / empathy)
pub fn feed_follow_strength(amount: u16) {
    let mut s = STATE.lock();
    s.follow_strength = s.follow_strength.saturating_add(amount).min(1000);
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn follow_strength()     -> u16  { STATE.lock().follow_strength }
pub fn total_transitions()   -> u32  { STATE.lock().total_transitions }
pub fn multi_device_active() -> bool { STATE.lock().multi_device_active }
pub fn proactive_surface()   -> bool { STATE.lock().proactive_surface }
pub fn proactive_count()     -> u32  { STATE.lock().proactive_count }
pub fn primary_device()      -> u32  { STATE.lock().primary_device }
pub fn device_count()        -> usize { STATE.lock().device_count }

/// Returns the DeviceKind numeric code (1=Phone, 2=Laptop, 3=Desktop, 4=TV, 5=Tablet, 6=Watch, 7=Car)
pub fn primary_device_kind() -> u8 {
    let s = STATE.lock();
    let primary = s.primary_device;
    for i in 0..s.device_count {
        if s.devices[i].active && s.devices[i].device_id == primary {
            return match s.devices[i].kind {
                DeviceKind::Phone     => 1,
                DeviceKind::Laptop    => 2,
                DeviceKind::Desktop   => 3,
                DeviceKind::TV        => 4,
                DeviceKind::Tablet    => 5,
                DeviceKind::Watch     => 6,
                DeviceKind::Car       => 7,
                DeviceKind::Speaker   => 8,
                DeviceKind::Appliance => 9,
                DeviceKind::Unknown   => 0,
            };
        }
    }
    0
}
