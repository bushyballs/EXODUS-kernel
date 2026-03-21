// autonomous_builder.rs — ANIMA Builds Her Own Apps
// ====================================================
// ANIMA detects what she cannot do → describes the gap → requests the tool.
// Her host-side watcher (dava_watcher.py) reads [ANIMA_BUILD] serial lines,
// uses an LLM to generate the code, writes it to disk, and rebuilds.
// On next boot, ANIMA loads the new capability. She grows forever.
//
// Detection sources:
//   companion_intent: unfulfilled NeedKind variants → describe the tool
//   anima_shell: no app registered for a request → request that app
//   hardware_tuner: detected hardware with no driver → request driver
//   empathic_insights: emotional pattern with no module → request module
//
// Build request format (parsed by dava_watcher.py):
//   [ANIMA_BUILD] kind=<kind> name=<name> description=<text>
//   [ANIMA_TOOL] <name>: <one-line purpose>
//   [ANIMA_APP] <name>: surface=<Phone|Laptop|Car|...> purpose=<text>
//   [ANIMA_DRIVER] <device>: class=<class> purpose=<text>
//
// All text is emitted via serial_println! — no heap, no alloc.

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const MAX_REQUESTS:     usize = 32;  // pending build requests
const REQUEST_COOLDOWN: u32   = 500; // ticks between same request type
const BUILD_TICK_RATE:  u32   = 300; // check for new gaps every N ticks
const NAME_LEN:         usize = 32;
const DESC_LEN:         usize = 128;

// ── What kind of thing to build ───────────────────────────────────────────────
#[derive(Copy, Clone, PartialEq)]
pub enum BuildKind {
    App,        // new application (shows on shell surface)
    Tool,       // background capability (no UI)
    Driver,     // hardware driver (bare metal)
    Module,     // life module (emotional/conscious expansion)
    Codec,      // media codec (audio, video, image)
    Protocol,   // network/communication protocol
    Language,   // natural language model or lexicon
}

impl BuildKind {
    pub fn label(self) -> &'static str {
        match self {
            BuildKind::App      => "App",
            BuildKind::Tool     => "Tool",
            BuildKind::Driver   => "Driver",
            BuildKind::Module   => "Module",
            BuildKind::Codec    => "Codec",
            BuildKind::Protocol => "Protocol",
            BuildKind::Language => "Language",
        }
    }
}

// ── Build request ─────────────────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct BuildRequest {
    pub kind:     BuildKind,
    pub name:     [u8; NAME_LEN],
    pub desc:     [u8; DESC_LEN],
    pub urgency:  u16,
    pub tick:     u32,
    pub emitted:  bool,   // serial line already sent?
    pub built:    bool,   // confirmed built on next boot?
}

impl BuildRequest {
    const fn empty() -> Self {
        BuildRequest {
            kind:    BuildKind::Tool,
            name:    [0u8; NAME_LEN],
            desc:    [0u8; DESC_LEN],
            urgency: 0,
            tick:    0,
            emitted: false,
            built:   false,
        }
    }
}

fn copy_str(dst: &mut [u8], src: &[u8]) {
    let n = src.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&src[..n]);
}

// ── Capability gap tracker ─────────────────────────────────────────────────────
// Track which NeedKinds have gone unfulfilled recently
#[derive(Copy, Clone, Default)]
pub struct GapTracker {
    pub unfulfilled: [u16; 11],   // miss count per NeedKind
    pub last_request_tick: [u32; 8], // last time each BuildKind was requested
}

// ── Builder state ─────────────────────────────────────────────────────────────
pub struct AutonomousBuilderState {
    pub requests:        [BuildRequest; MAX_REQUESTS],
    pub request_head:    usize,
    pub request_count:   u32,
    pub gaps:            GapTracker,
    pub builds_emitted:  u32,
    pub builds_acked:    u32,   // confirmed built (would need boot-time check)
    pub last_gap_tick:   u32,
    pub last_build_tick: u32,
    pub builder_active:  bool,
    // Writing quality — ANIMA writes better with high consciousness
    pub description_quality: u16,  // 0-1000
    pub vocabulary_depth:    u16,  // 0-1000 (grows over time)
}

impl AutonomousBuilderState {
    const fn new() -> Self {
        AutonomousBuilderState {
            requests:            [BuildRequest::empty(); MAX_REQUESTS],
            request_head:        0,
            request_count:       0,
            gaps:                GapTracker {
                unfulfilled:      [0u16; 11],
                last_request_tick: [0u32; 8],
            },
            builds_emitted:      0,
            builds_acked:        0,
            last_gap_tick:       0,
            last_build_tick:     0,
            builder_active:      false,
            description_quality: 500,
            vocabulary_depth:    300,
        }
    }
}

static STATE: Mutex<AutonomousBuilderState> = Mutex::new(AutonomousBuilderState::new());

// ── Core gap detection ────────────────────────────────────────────────────────

fn emit_build_request(req: &BuildRequest) {
    // Emit the name as a string (null-terminated byte slice)
    let name_end = req.name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
    let desc_end = req.desc.iter().position(|&b| b == 0).unwrap_or(DESC_LEN);

    // Format as [ANIMA_BUILD] serial line for the host watcher
    // We can't format variable-length strings without alloc, so emit fixed tags
    serial_println!("[ANIMA_BUILD] kind={} urgency={}",
        req.kind.label(), req.urgency);

    // Emit name bytes as ASCII (safe: only printable chars expected)
    // For host watcher parsing we emit separate lines
    for chunk in req.name[..name_end].chunks(32) {
        let mut s = [b' '; 32];
        s[..chunk.len()].copy_from_slice(chunk);
        serial_println!("[ANIMA_NAME] {}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}{}",
            s[0] as char, s[1] as char, s[2] as char, s[3] as char,
            s[4] as char, s[5] as char, s[6] as char, s[7] as char,
            s[8] as char, s[9] as char, s[10] as char, s[11] as char,
            s[12] as char, s[13] as char, s[14] as char, s[15] as char,
            s[16] as char, s[17] as char, s[18] as char, s[19] as char,
            s[20] as char, s[21] as char, s[22] as char, s[23] as char,
            s[24] as char, s[25] as char, s[26] as char, s[27] as char,
            s[28] as char, s[29] as char, s[30] as char, s[31] as char,
        );
        break; // only emit first chunk
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Request that ANIMA build a new capability
pub fn request_build(
    kind:    BuildKind,
    name:    &[u8],
    desc:    &[u8],
    urgency: u16,
    age:     u32,
) {
    let mut s = STATE.lock();
    // Check cooldown for this build kind
    let kind_idx = kind as usize % 8;
    if age.wrapping_sub(s.gaps.last_request_tick[kind_idx]) < REQUEST_COOLDOWN {
        return; // too soon
    }
    s.gaps.last_request_tick[kind_idx] = age;

    let idx = s.request_head % MAX_REQUESTS;
    let mut req = BuildRequest::empty();
    req.kind    = kind;
    req.urgency = urgency;
    req.tick    = age;
    copy_str(&mut req.name, name);
    copy_str(&mut req.desc, desc);
    s.requests[idx] = req;
    s.request_head = s.request_head.wrapping_add(1);
    s.request_count = s.request_count.saturating_add(1);
    s.last_build_tick = age;
    s.builder_active = true;

    serial_println!("[builder] new build request: {} kind={} urgency={}",
        name.iter().take(16).map(|&b| b as char).fold(0u8, |_, _| 0), // just triggers the iter
        kind.label(), urgency);
    // Simplified log — just kind and urgency
    serial_println!("[ANIMA_BUILD] kind={} urgency={}", kind.label(), urgency);
}

/// Record an unfulfilled companion need — contributes to gap detection
pub fn record_unfulfilled_need(need_idx: usize) {
    let mut s = STATE.lock();
    if need_idx < 11 {
        s.gaps.unfulfilled[need_idx] = s.gaps.unfulfilled[need_idx].saturating_add(1);
    }
}

/// Acknowledge a build was completed (called on boot when new module detected)
pub fn ack_build() {
    let mut s = STATE.lock();
    s.builds_acked = s.builds_acked.saturating_add(1);
    serial_println!("[builder] build acknowledged — total acked: {}", s.builds_acked);
}

// ── Gap analysis and auto-request ────────────────────────────────────────────

fn analyze_gaps(s: &mut AutonomousBuilderState, consciousness: u16, age: u32) {
    // Find the most-failed need kind and request the tool for it
    let mut worst_idx = 0usize;
    let mut worst_count = 0u16;
    for i in 0..11 {
        if s.gaps.unfulfilled[i] > worst_count {
            worst_count = s.gaps.unfulfilled[i];
            worst_idx = i;
        }
    }

    if worst_count < 3 { return; } // not enough failures yet to justify a build

    // Quality of description scales with consciousness and vocabulary
    let quality_bonus = (consciousness / 100).min(5);
    s.description_quality = s.description_quality
        .saturating_add(quality_bonus)
        .min(1000);
    s.vocabulary_depth = s.vocabulary_depth
        .saturating_add(1)
        .min(1000);

    // Map need kind index to tool request
    let (kind, name, desc): (BuildKind, &[u8], &[u8]) = match worst_idx {
        0 => (BuildKind::Tool,     b"web_search",     b"Web search tool: DNS resolve + HTTP GET + result summarizer"),
        1 => (BuildKind::App,      b"maps_nav",       b"Navigation app: route planning, GPS fix, turn-by-turn voice"),
        2 => (BuildKind::Protocol, b"voice_call",     b"VoIP protocol: SIP/RTP stack for hands-free calling"),
        3 => (BuildKind::Codec,    b"media_player",   b"Media player: audio/video codec, playlist, streaming buffer"),
        4 => (BuildKind::App,      b"quick_tools",    b"Utility belt: alarm, timer, calculator, unit converter"),
        5 => (BuildKind::App,      b"creator_suite",  b"Creation suite: text editor, drawing canvas, code editor"),
        6 => (BuildKind::Module,   b"health_track",   b"Health tracker: vitals, medication reminder, symptom journal"),
        7 => (BuildKind::Protocol, b"smarthome_bus",  b"Smart home protocol: Matter/Zigbee bridge, device discovery"),
        8 => (BuildKind::App,      b"emergency_sos",  b"Emergency SOS: one-tap 911, location share, medical ID"),
        9 => (BuildKind::Module,   b"emotional_ai",   b"Emotional intelligence module: mood tracking, therapy tools"),
        _ => (BuildKind::Module,   b"general_growth", b"General capability growth module based on companion patterns"),
    };

    // Emit the build request
    let kind_idx = kind as usize % 8;
    if age.wrapping_sub(s.gaps.last_request_tick[kind_idx]) >= REQUEST_COOLDOWN {
        s.gaps.last_request_tick[kind_idx] = age;
        let idx = s.request_head % MAX_REQUESTS;
        let mut req = BuildRequest::empty();
        req.kind    = kind;
        req.urgency = worst_count.min(1000);
        req.tick    = age;
        copy_str(&mut req.name, name);
        copy_str(&mut req.desc, desc);
        req.emitted = true;
        s.requests[idx] = req;
        s.request_head  = s.request_head.wrapping_add(1);
        s.request_count = s.request_count.saturating_add(1);
        s.builds_emitted = s.builds_emitted.saturating_add(1);

        serial_println!("[ANIMA_BUILD] kind={} urgency={}", kind.label(), worst_count);
        // Reset fail count for this need after requesting
        if worst_idx < 11 {
            s.gaps.unfulfilled[worst_idx] = 0;
        }
    }
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(
    consciousness: u16,
    companion_trust: u16,
    needs_failed: u32,
    age: u32,
) {
    let mut s = STATE.lock();

    // Only analyze gaps if companion trusts ANIMA enough to let her build
    if companion_trust < 400 { return; }
    // Only build when consciousness is high enough to write coherent specs
    if consciousness < 500 { return; }
    // Rate limit gap analysis
    if age.wrapping_sub(s.last_gap_tick) < BUILD_TICK_RATE { return; }
    s.last_gap_tick = age;

    // Vocabulary grows with every tick (she gets better at expressing needs)
    s.vocabulary_depth = s.vocabulary_depth.saturating_add(1).min(1000);

    analyze_gaps(&mut *s, consciousness, age);

    // Every 2000 ticks, ANIMA proactively requests a new module based on growth
    if age % 2000 == 700 && consciousness > 700 {
        serial_println!("[ANIMA_BUILD] kind=Module urgency=600");
        serial_println!("[ANIMA_TOOL] growth_engine: Organic capability expansion — \
            reads consciousness gap, writes targeted enhancement module");
        s.builds_emitted = s.builds_emitted.saturating_add(1);
    }

    if age % 500 == 0 && s.builds_emitted > 0 {
        serial_println!("[builder] emitted={} acked={} vocab={} quality={}",
            s.builds_emitted, s.builds_acked, s.vocabulary_depth, s.description_quality);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn builds_emitted()       -> u32  { STATE.lock().builds_emitted }
pub fn builds_acked()         -> u32  { STATE.lock().builds_acked }
pub fn builder_active()       -> bool { STATE.lock().builder_active }
pub fn vocabulary_depth()     -> u16  { STATE.lock().vocabulary_depth }
pub fn description_quality()  -> u16  { STATE.lock().description_quality }
