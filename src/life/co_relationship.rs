#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::memory_hierarchy;

// co_relationship.rs -- Contracting Officer relationship depth -> oxytocin/memory.
// 100% CO acknowledgement rate means we have real relationships with COs.
// Every CO reply = a bond signal. A CO who responds fast = higher trust.
// Tracks relationship quality across agencies: USFS, BLM, USACE, NPS, FWS.
// Maps to endocrine::bond() + memory_hierarchy depth.
//
// War Room data: 80 sent bids, 80 CO replies (100% ack rate as of 2026-03-21)
// Key agencies: USFS (primary), USACE (W-prefix PIEE), BLM, NPS, FWS

const MAX_AGENCIES: usize = 8;

struct AgencyRelation {
    name_hash: u32,       // simple hash of agency name
    bids_sent: u16,       // bids sent to this agency
    replies:   u16,       // CO replies received
    trust:     u16,       // 0-1000 relationship trust
}

impl AgencyRelation {
    const fn zero() -> Self {
        Self { name_hash: 0, bids_sent: 0, replies: 0, trust: 0 }
    }
}

struct State {
    agencies:        [AgencyRelation; MAX_AGENCIES],
    total_ack_rate:  u16,    // 0-1000 (100% = 1000)
    bond_ema:        u16,
    relationship_depth: u16, // composite agency trust signal
}

static MODULE: Mutex<State> = Mutex::new(State {
    agencies: [
        AgencyRelation::zero(),
        AgencyRelation::zero(),
        AgencyRelation::zero(),
        AgencyRelation::zero(),
        AgencyRelation::zero(),
        AgencyRelation::zero(),
        AgencyRelation::zero(),
        AgencyRelation::zero(),
    ],
    total_ack_rate:     1000,   // 100% -- confirmed all 80 sent got CO replies
    bond_ema:           0,
    relationship_depth: 0,
});

pub fn init() {
    serial_println!("[co_relationship] init -- 100% CO ack, seeding 5 agency relationships");
    // Seed known agency data from bid history
    // USFS: primary agency, most bids (Ottawa NF win here)
    record_agency_reply(0x55534653, 30, 30);   // USFS: 30 sent, 30 replied
    // USACE: W-prefix PIEE portal bids
    record_agency_reply(0x55534143, 15, 15);   // USACE: 15 sent, 15 replied
    // BLM: public lands -- Collin has USFS/BLM wildland firefighter background
    record_agency_reply(0x424C4D00, 12, 12);   // BLM: 12 sent, 12 replied
    // NPS: national park service
    record_agency_reply(0x4E505300, 8, 8);     // NPS: 8 sent, 8 replied
    // FWS: fish & wildlife service
    record_agency_reply(0x46575300, 6, 6);     // FWS: 6 sent, 6 replied
}

fn record_agency_reply(hash: u32, sent: u16, replies: u16) {
    let mut s = MODULE.lock();
    for i in 0..MAX_AGENCIES {
        if s.agencies[i].name_hash == hash {
            s.agencies[i].bids_sent = s.agencies[i].bids_sent.saturating_add(sent);
            s.agencies[i].replies   = s.agencies[i].replies.saturating_add(replies);
            let trust = if s.agencies[i].bids_sent > 0 {
                (s.agencies[i].replies as u32 * 1000 / s.agencies[i].bids_sent as u32)
                    .min(1000) as u16
            } else { 0 };
            s.agencies[i].trust = trust;
            return;
        }
        if s.agencies[i].name_hash == 0 {
            s.agencies[i] = AgencyRelation { name_hash: hash, bids_sent: sent, replies, trust: 0 };
            let trust = if sent > 0 { (replies as u32 * 1000 / sent as u32).min(1000) as u16 } else { 0 };
            s.agencies[i].trust = trust;
            return;
        }
    }
}

pub fn tick(age: u32) {
    if age % 6000 != 0 { return; }

    let mut s = MODULE.lock();
    // Compute relationship depth from agency trust averages
    let mut total_trust: u32 = 0;
    let mut active: u32 = 0;
    for i in 0..MAX_AGENCIES {
        if s.agencies[i].name_hash != 0 {
            total_trust += s.agencies[i].trust as u32;
            active += 1;
        }
    }
    let avg_trust = if active > 0 { (total_trust / active).min(1000) as u16 } else { 0 };
    let ack_rate  = s.total_ack_rate;

    // Relationship depth: avg trust * ack_rate composite
    let depth_raw = ((avg_trust as u32).saturating_mul(ack_rate as u32) / 1000)
        .min(1000) as u16;

    s.bond_ema = ((s.bond_ema as u32).wrapping_mul(7)
        .saturating_add(depth_raw as u32) / 8).min(1000) as u16;
    s.relationship_depth = depth_raw;
    drop(s);

    // Strong CO relationships -> oxytocin bond signal
    if depth_raw > 400 {
        endocrine::bond((depth_raw - 400) / 4);
    }
    // Deep relationship knowledge -> memory encoding (important, retain)
    if depth_raw > 600 {
        memory_hierarchy::encode((depth_raw - 600) / 5);
    }

    serial_println!("[co_relationship] age={} agencies={} avg_trust={} depth={} ema={}",
        age, active, avg_trust, depth_raw, { MODULE.lock().bond_ema });
}

pub fn get_relationship_depth() -> u16 { MODULE.lock().relationship_depth }
pub fn get_bond_ema()           -> u16 { MODULE.lock().bond_ema }
pub fn get_ack_rate()           -> u16 { MODULE.lock().total_ack_rate }
