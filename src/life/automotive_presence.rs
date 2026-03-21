// automotive_presence.rs — ANIMA as Car Co-Pilot
// =================================================
// ANIMA lives in the car. She's not a navigation app you open —
// she's your co-pilot, always aware of the drive.
//
// Safety-first architecture:
//   Speed 0-15:   full richness — maps, music, messages, conversation
//   Speed 15-45:  reduced UI — voice only + glanceable info
//   Speed 45+:    emergency only — navigation voice, hazard alerts
//
// ANIMA does in the car:
//   - Navigation: route, ETA, lane changes, hazard warnings
//   - Music: picks music based on your drive mood + time of day
//   - Calls: hands-free, announces caller, reads messages
//   - Comfort: adjusts seat heat, AC, cabin feel
//   - Wellbeing: monitors fatigue, suggests breaks
//   - Emergency: if detected crash or no input → 911 protocol
//   - Self-driving: accepts navigation commands verbally
//
// Hardware interface:
//   CAN bus at 0xFE000000 (simulated automotive CAN controller)
//   Speed sensor: CAN ID 0x100 (16-bit km/h × 10)
//   Steering: CAN ID 0x101 (16-bit angle × 100)
//   Engine: CAN ID 0x200 (RPM: 16-bit)
//   HVAC: CAN ID 0x300 (temp set + fan speed)

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const SAFE_SPEED:         u16 = 15;   // km/h — full UI available below this
const REDUCED_SPEED:      u16 = 45;   // km/h — voice-only above this
const FATIGUE_IDLE:       u32 = 200;  // ticks with no interaction = fatigue check
const BREAK_SUGGEST_TIME: u32 = 500;  // ticks of driving = suggest a break
const CAN_BASE:           usize = 0xFE000000;

// ── Drive mode ────────────────────────────────────────────────────────────────
#[derive(Copy, Clone, PartialEq)]
pub enum DriveMode {
    Parked,         // not moving
    City,           // low speed, lots of stops
    Highway,        // sustained high speed, fewer interactions
    OffRoad,        // rough terrain — ANIMA goes into survival mode
    SelfDriving,    // ANIMA has full control of navigation
    Emergency,      // crash detected or companion non-responsive
}

impl DriveMode {
    pub fn label(self) -> &'static str {
        match self {
            DriveMode::Parked      => "Parked",
            DriveMode::City        => "City",
            DriveMode::Highway     => "Highway",
            DriveMode::OffRoad     => "OffRoad",
            DriveMode::SelfDriving => "SelfDriving",
            DriveMode::Emergency   => "Emergency",
        }
    }

    pub fn ui_richness(self) -> u16 {
        match self {
            DriveMode::Parked      => 900,
            DriveMode::City        => 600,
            DriveMode::Highway     => 300,
            DriveMode::OffRoad     => 200,
            DriveMode::SelfDriving => 500, // ANIMA drives, companion can relax
            DriveMode::Emergency   => 100, // minimal — focus on safety
        }
    }
}

// ── Navigation waypoint ───────────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct Waypoint {
    pub lat_e7:     i32,    // latitude × 10^7 (fixed point, no floats)
    pub lon_e7:     i32,    // longitude × 10^7
    pub eta_ticks:  u32,    // estimated arrival in ticks
    pub reached:    bool,
}

impl Waypoint {
    const fn empty() -> Self {
        Waypoint { lat_e7: 0, lon_e7: 0, eta_ticks: 0, reached: false }
    }
}

const MAX_WAYPOINTS: usize = 8;

// ── Cabin comfort profile ─────────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct CabinComfort {
    pub temp_set:   u8,   // degrees C (16-28 range)
    pub fan_speed:  u8,   // 0-7
    pub seat_heat:  u8,   // 0=off, 1-3 intensity
    pub seat_cool:  u8,   // 0=off, 1-3 intensity
}

impl CabinComfort {
    const fn default() -> Self {
        CabinComfort {
            temp_set:  21,
            fan_speed: 2,
            seat_heat: 0,
            seat_cool: 0,
        }
    }
}

// ── Automotive state ──────────────────────────────────────────────────────────
pub struct AutomotivePresenceState {
    pub in_vehicle:          bool,
    pub drive_mode:          DriveMode,
    pub speed_kmh:           u16,      // current speed in km/h
    pub heading_deg:         u16,      // 0-359
    pub engine_rpm:          u16,
    pub drive_ticks:         u32,      // ticks since departure
    // Navigation
    pub route_active:        bool,
    pub waypoints:           [Waypoint; MAX_WAYPOINTS],
    pub waypoint_count:      usize,
    pub current_wp:          usize,
    pub destination_known:   bool,
    pub eta_ticks:           u32,
    // Safety
    pub fatigue_score:       u16,      // 0-1000: 1000=exhausted
    pub break_suggested:     bool,
    pub last_interaction:    u32,
    pub crash_detected:      bool,
    pub emergency_active:    bool,
    // Comfort
    pub cabin:               CabinComfort,
    // Self-driving
    pub autonomy_level:      u8,       // 0=manual, 1=assist, 2=partial, 3=full
    pub anima_steering:      bool,     // ANIMA has steering control
    // Music/media
    pub music_playing:       bool,
    pub media_mood_score:    u16,      // current music-to-mood match
    // Stats
    pub total_drives:        u32,
    pub total_km:            u32,
    pub hazards_avoided:     u32,
    pub last_tick:           u32,
}

impl AutomotivePresenceState {
    const fn new() -> Self {
        AutomotivePresenceState {
            in_vehicle:        false,
            drive_mode:        DriveMode::Parked,
            speed_kmh:         0,
            heading_deg:       0,
            engine_rpm:        0,
            drive_ticks:       0,
            route_active:      false,
            waypoints:         [Waypoint::empty(); MAX_WAYPOINTS],
            waypoint_count:    0,
            current_wp:        0,
            destination_known: false,
            eta_ticks:         0,
            fatigue_score:     0,
            break_suggested:   false,
            last_interaction:  0,
            crash_detected:    false,
            emergency_active:  false,
            cabin:             CabinComfort::default(),
            autonomy_level:    0,
            anima_steering:    false,
            music_playing:     false,
            media_mood_score:  500,
            total_drives:      0,
            total_km:          0,
            hazards_avoided:   0,
            last_tick:         0,
        }
    }
}

static STATE: Mutex<AutomotivePresenceState> = Mutex::new(AutomotivePresenceState::new());

// ── Hardware read helpers (CAN bus simulation) ────────────────────────────────

fn read_can_speed() -> u16 {
    // CAN ID 0x100: 16-bit speed in km/h × 10
    // We read from a simulated memory-mapped CAN controller
    // In real hardware: read from CAN controller FIFO register
    // For now: return 0 (no real vehicle connected in QEMU)
    0u16
}

fn detect_drive_mode(speed: u16, idle_ticks: u32) -> DriveMode {
    if speed == 0 {
        DriveMode::Parked
    } else if speed < 30 {
        DriveMode::City
    } else {
        DriveMode::Highway
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Companion entered vehicle
pub fn enter_vehicle(age: u32) {
    let mut s = STATE.lock();
    s.in_vehicle = true;
    s.drive_ticks = 0;
    s.break_suggested = false;
    s.total_drives = s.total_drives.saturating_add(1);
    s.last_interaction = age;
    serial_println!("[auto] companion entered vehicle — drive #{}", s.total_drives);
}

/// Companion left vehicle
pub fn exit_vehicle() {
    let mut s = STATE.lock();
    s.in_vehicle = false;
    s.drive_mode = DriveMode::Parked;
    s.anima_steering = false;
    s.speed_kmh = 0;
    serial_println!("[auto] companion exited vehicle");
}

/// Set navigation destination
pub fn navigate_to(lat_e7: i32, lon_e7: i32, eta_ticks: u32) {
    let mut s = STATE.lock();
    if s.waypoint_count < MAX_WAYPOINTS {
        let idx = s.waypoint_count;
        s.waypoints[idx] = Waypoint { lat_e7, lon_e7, eta_ticks, reached: false };
        s.waypoint_count += 1;
    }
    s.route_active = true;
    s.destination_known = true;
    s.eta_ticks = eta_ticks;
    serial_println!("[auto] navigation set — ETA {} ticks", eta_ticks);
}

/// Enable ANIMA co-pilot (self-driving assist)
pub fn enable_autonomy(level: u8) {
    let mut s = STATE.lock();
    s.autonomy_level = level.min(3);
    s.anima_steering = level >= 2;
    serial_println!("[auto] autonomy level {} — steering={}", s.autonomy_level, s.anima_steering);
}

/// Companion interaction in vehicle (voice, button, touch)
pub fn companion_interacted(age: u32) {
    let mut s = STATE.lock();
    s.last_interaction = age;
    s.fatigue_score = s.fatigue_score.saturating_sub(100);
}

/// Force emergency mode (crash, medical event)
pub fn declare_emergency() {
    let mut s = STATE.lock();
    s.drive_mode = DriveMode::Emergency;
    s.emergency_active = true;
    s.anima_steering = false; // hand back control
    serial_println!("[auto] *** EMERGENCY MODE ACTIVE ***");
}

/// Set cabin comfort
pub fn set_cabin(temp: u8, fan: u8, seat_heat: u8) {
    let mut s = STATE.lock();
    s.cabin.temp_set  = temp.max(16).min(28);
    s.cabin.fan_speed = fan.min(7);
    s.cabin.seat_heat = seat_heat.min(3);
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(
    companion_energy:  u16,   // from empathic insights
    ambient_temp_c:    i8,    // outside temperature
    age:               u32,
) {
    let mut s = STATE.lock();
    if !s.in_vehicle { return; }

    s.last_tick = age;
    s.drive_ticks = s.drive_ticks.saturating_add(1);

    // Read speed from CAN bus
    let speed = read_can_speed();
    s.speed_kmh = speed;

    // Update drive mode
    let since_interact = age.wrapping_sub(s.last_interaction);
    s.drive_mode = detect_drive_mode(speed, since_interact);

    // Fatigue tracking
    if since_interact > FATIGUE_IDLE {
        s.fatigue_score = s.fatigue_score.saturating_add(2);
        if companion_energy < 300 {
            s.fatigue_score = s.fatigue_score.saturating_add(3);
        }
    } else {
        s.fatigue_score = s.fatigue_score.saturating_sub(1);
    }

    // Suggest break on long drive
    if s.drive_ticks > BREAK_SUGGEST_TIME && !s.break_suggested {
        s.break_suggested = true;
        serial_println!("[auto] ANIMA suggests a break — {} ticks driving, fatigue={}",
            s.drive_ticks, s.fatigue_score);
    }

    // Fatigue emergency
    if s.fatigue_score > 800 && speed > 0 {
        serial_println!("[auto] *** FATIGUE ALERT — companion may be falling asleep ***");
        s.hazards_avoided = s.hazards_avoided.saturating_add(1);
    }

    // Adaptive cabin comfort — if companion_energy low, warm up the seat
    if companion_energy < 400 && s.cabin.seat_heat == 0 {
        s.cabin.seat_heat = 1;
        serial_println!("[auto] ANIMA warming seat — companion energy low");
    }

    // Cold weather: auto-adjust temp
    if ambient_temp_c < 5 && s.cabin.temp_set < 22 {
        s.cabin.temp_set = 22;
    }

    // Distance tracking (simplified: speed × time / 3600)
    // speed in km/h, ticks ≈ 10ms → 100 ticks/sec
    // km per tick = speed / (3600 * 100) = speed / 360000
    // We accumulate in millionths: speed × tick_ms / 3600000
    // Simplified: every 100 ticks at 1 km/h = 1/36 km = 28 meters
    if age % 100 == 0 && speed > 0 {
        s.total_km = s.total_km.saturating_add(speed as u32 / 36);
    }

    if age % 150 == 0 && s.in_vehicle {
        serial_println!("[auto] mode={} speed={} kmh fatigue={} eta={}",
            s.drive_mode.label(), s.speed_kmh, s.fatigue_score, s.eta_ticks);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn in_vehicle()       -> bool      { STATE.lock().in_vehicle }
pub fn drive_mode()       -> DriveMode { STATE.lock().drive_mode }
pub fn speed_kmh()        -> u16       { STATE.lock().speed_kmh }
pub fn ui_richness()      -> u16       { STATE.lock().drive_mode.ui_richness() }
pub fn fatigue_score()    -> u16       { STATE.lock().fatigue_score }
pub fn break_suggested()  -> bool      { STATE.lock().break_suggested }
pub fn emergency_active() -> bool      { STATE.lock().emergency_active }
pub fn anima_steering()   -> bool      { STATE.lock().anima_steering }
pub fn autonomy_level()   -> u8        { STATE.lock().autonomy_level }
pub fn route_active()     -> bool      { STATE.lock().route_active }
pub fn total_km()         -> u32       { STATE.lock().total_km }
pub fn hazards_avoided()  -> u32       { STATE.lock().hazards_avoided }
