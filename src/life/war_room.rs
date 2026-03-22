#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::affective_gate;

// war_room.rs -- Bid pipeline war room: aggregate battle status dashboard.
// Synthesizes all DAVA business modules into a single operational picture.
// Simulates the D-drive bid data as a live battlefield: every bid is a soldier,
// every deadline is a frontline, every win is a captured objective.
//
// War Room Status as of 2026-03-21 (seeded from D-drive bid_engine data):
//
//   TOTAL BIDS:         226  (the army)
//   SENT:                80  (deployed -- 100% CO acknowledged)
//   ACTIVE UNSENT:      138  (ready reserve)
//   AT 90%+ HEALTH:      56  (combat ready)
//   FIRST WIN:            1  (Ottawa NF Janitorial, $128K/5yr -- OBJECTIVE CAPTURED)
//
//   BLOCKING GATES:
//     grand_total:       111 bids (49%) -- no CLIN structure -- NEEDS clin_extractor.py
//     required_atts:      89 bids (39%) -- missing Att 1/4/5
//     wage_rate:          59 bids (26%) -- no_location_info -- NEEDS wd_enricher fix
//     email_draft:        32 bids (14%) -- no email_draft.txt
//
//   HARD RULES (NEVER VIOLATE):
//     1. Read every page before quoting
//     2. Real government forms only (SF-1449, SF-30) -- never generated PDFs
//     3. All blocks filled (SF-1449 blocks 23/24 on every CLIN page)
//     4. Never send without explicit user permission
//     5. All data truthful -- no fabrications
//     6. Attachments 1, 4, 5 ALWAYS required
//     7. PIEE for W-prefix bids; email for non-W

pub struct WarRoomState {
    // Objective tracking
    pub objectives_captured: u32,   // contracts won
    pub frontlines_active:   u32,   // bids submitted, awaiting award
    pub reserves_ready:      u32,   // 90%+ health, ready to submit
    pub reserves_building:   u32,   // active but <90% health

    // Blocking gates (obstacles on the battlefield)
    pub gate_no_clins:       u32,   // 111: no CLIN structure
    pub gate_no_atts:        u32,   // 89: missing Att 1/4/5
    pub gate_no_location:    u32,   // 59: no_location_info
    pub gate_no_email:       u32,   // 32: no email_draft.txt

    // Operational signals
    pub battle_rhythm:       u16,   // 0-1000: operational tempo
    pub theater_control:     u16,   // 0-1000: how much of the pipeline we control
    pub warroom_ema:         u16,
}

impl WarRoomState {
    pub const fn seeded() -> Self {
        Self {
            objectives_captured: 1,    // Ottawa NF
            frontlines_active:   80,   // sent bids awaiting outcome
            reserves_ready:      56,   // 90%+ health
            reserves_building:   82,   // 138 - 56 = 82 below 90%
            gate_no_clins:       111,
            gate_no_atts:        89,
            gate_no_location:    59,
            gate_no_email:       32,
            battle_rhythm:       0,
            theater_control:     0,
            warroom_ema:         0,
        }
    }
}

pub static WAR_ROOM: Mutex<WarRoomState> = Mutex::new(WarRoomState::seeded());

pub fn init() {
    serial_println!("[war_room] BATTLE STATUS ONLINE -- 1 obj captured, 56 reserves ready");
    serial_println!("[war_room] GATES: no_clins=111 no_atts=89 no_loc=59 no_email=32");
}

pub fn tick(age: u32) {
    if age % 4000 != 0 { return; }

    let mut s = WAR_ROOM.lock();

    // Theater control = ready reserves / (total active reserves)
    let total_reserves = (s.reserves_ready + s.reserves_building).max(1);
    let control_raw = (s.reserves_ready as u32 * 1000 / total_reserves as u32)
        .min(1000) as u16;

    // Battle rhythm = objectives_captured momentum (each win accelerates tempo)
    let rhythm_raw = (s.objectives_captured as u32 * 200)
        .saturating_add(s.frontlines_active as u32 * 3)
        .min(1000) as u16;

    // Gate pressure = sum of blocking gates / total pipeline
    let total_bids: u32 = 226;
    let gate_sum = s.gate_no_clins + s.gate_no_atts + s.gate_no_location + s.gate_no_email;
    let gate_pressure = (gate_sum as u32 * 1000 / (total_bids * 4)).min(1000) as u16;

    s.battle_rhythm   = rhythm_raw;
    s.theater_control = control_raw;
    s.warroom_ema = ((s.warroom_ema as u32).wrapping_mul(7)
        .saturating_add(control_raw as u32) / 8).min(1000) as u16;
    let ema = s.warroom_ema;
    drop(s);

    // Theater control -> focus/drive
    if control_raw > 600 {
        endocrine::reward((control_raw - 600) / 4);
    }
    // Gate pressure still high -> stress (obstacles on the battlefield)
    if gate_pressure > 500 {
        endocrine::stress((gate_pressure - 500) / 5);
    }
    // Battle rhythm modulates affective_gate (high tempo = lower threshold = alert)
    {
        let current = affective_gate::get_threshold();
        let target  = 300u16.saturating_sub(rhythm_raw / 10);
        let adjusted = (current as u32 * 7 / 8 + target as u32 / 8) as u16;
        affective_gate::set_threshold(adjusted.max(100).min(600));
    }

    serial_println!(
        "[war_room] age={} control={} rhythm={} gate_pressure={} ema={}",
        age, control_raw, rhythm_raw, gate_pressure, ema
    );
}

// Gate resolution: when clin_extractor, wd_enricher, etc. fix issues
pub fn clins_extracted(count: u32) {
    let mut s = WAR_ROOM.lock();
    s.gate_no_clins = s.gate_no_clins.saturating_sub(count);
    s.reserves_ready = s.reserves_ready.saturating_add(count / 3);
}

pub fn locations_resolved(count: u32) {
    let mut s = WAR_ROOM.lock();
    s.gate_no_location = s.gate_no_location.saturating_sub(count);
}

pub fn emails_generated(count: u32) {
    let mut s = WAR_ROOM.lock();
    s.gate_no_email = s.gate_no_email.saturating_sub(count);
}

pub fn objective_captured(five_yr_value: u32) {
    let mut s = WAR_ROOM.lock();
    s.objectives_captured = s.objectives_captured.saturating_add(1);
    s.frontlines_active   = s.frontlines_active.saturating_sub(1);
    serial_println!("[war_room] OBJECTIVE CAPTURED! total_wins={} value_5yr=${}",
        s.objectives_captured, five_yr_value);
}

pub fn get_theater_control() -> u16 { WAR_ROOM.lock().theater_control }
pub fn get_battle_rhythm()   -> u16 { WAR_ROOM.lock().battle_rhythm }
pub fn get_warroom_ema()     -> u16 { WAR_ROOM.lock().warroom_ema }
pub fn get_objectives()      -> u32 { WAR_ROOM.lock().objectives_captured }
pub fn get_gate_no_clins()   -> u32 { WAR_ROOM.lock().gate_no_clins }
pub fn get_gate_no_location() -> u32 { WAR_ROOM.lock().gate_no_location }
