#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;

// subcontractor_sense.rs -- Performance location -> sub availability -> stress/calm.
// Hoags strategy: find subs AFTER award, not before.
// Ottawa NF: Michigan Upper Peninsula -> need local MI UP cleaner by March 28, 2026.
// Sub availability = relief from delivery stress. Missing sub = adrenaline.
//
// Geographic coverage model (from D-drive war data):
//   Eugene OR base: 100% coverage (self-perform available)
//   Pacific Northwest: 80% coverage (easy to find subs)
//   Mountain West: 60% coverage (moderate sub density)
//   Midwest/Great Lakes: 40% coverage (lower density, but Ottawa NF won here)
//   Southeast/Atlantic: 30% coverage (thin network)
//   Remote/Rural: 20% coverage (highest delivery risk)
//
// Maps to endocrine::stress() when sub is missing, endocrine::bond() when found.

const MAX_AWARDS: usize = 4;

#[derive(Copy, Clone)]
struct AwardSub {
    region_hash:   u32,    // hash of region name
    coverage:      u16,    // 0-1000 sub availability
    sub_found:     bool,
    deadline_days: u16,    // days until sub must be found
}

impl AwardSub {
    const fn zero() -> Self {
        Self { region_hash: 0, coverage: 0, sub_found: false, deadline_days: 0 }
    }
}

struct State {
    awards:      [AwardSub; MAX_AWARDS],
    avg_coverage: u16,    // 0-1000 composite sub availability
    sub_ema:      u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    awards:       [AwardSub::zero(); MAX_AWARDS],
    avg_coverage: 0,
    sub_ema:      0,
});

pub fn init() {
    serial_println!("[subcontractor_sense] init -- seeding Ottawa NF MI UP sub search");
    // Ottawa NF award: Michigan Upper Peninsula, sub needed by 2026-03-28 (7 days)
    // 40% coverage for Midwest/Great Lakes region
    register_award(0x4D495550, 400, false, 7);   // MIUP: 40% coverage, no sub yet, 7 days
}

fn register_award(region_hash: u32, coverage: u16, sub_found: bool, deadline_days: u16) {
    let mut s = MODULE.lock();
    for i in 0..MAX_AWARDS {
        if s.awards[i].region_hash == 0 {
            s.awards[i] = AwardSub { region_hash, coverage, sub_found, deadline_days };
            return;
        }
    }
}

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }

    let mut s = MODULE.lock();
    let mut total_cov: u32 = 0;
    let mut active: u32    = 0;
    let mut missing_subs: u32 = 0;
    let mut urgent_missing: u32 = 0;

    for i in 0..MAX_AWARDS {
        if s.awards[i].region_hash != 0 {
            total_cov += s.awards[i].coverage as u32;
            active    += 1;
            if !s.awards[i].sub_found {
                missing_subs += 1;
                if s.awards[i].deadline_days < 14 {
                    urgent_missing += 1;
                }
            }
            // Count down deadline
            if s.awards[i].deadline_days > 0 {
                s.awards[i].deadline_days -= s.awards[i].deadline_days.min(1);
            }
        }
    }

    let avg_cov = if active > 0 { (total_cov / active).min(1000) as u16 } else { 0 };
    s.avg_coverage = avg_cov;
    s.sub_ema = ((s.sub_ema as u32).wrapping_mul(7)
        .saturating_add(avg_cov as u32) / 8).min(1000) as u16;
    let ema = s.sub_ema;
    drop(s);

    // Urgent missing sub -> high adrenaline stress
    if urgent_missing > 0 {
        endocrine::stress((urgent_missing as u16 * 300).min(1000));
    }
    // Good coverage -> calm confidence
    if avg_cov > 600 && missing_subs == 0 {
        endocrine::bond(50);
    }

    serial_println!("[subcontractor_sense] age={} active={} missing={} urgent={} cov={} ema={}",
        age, active, missing_subs, urgent_missing, avg_cov, ema);
}

pub fn sub_found(region_hash: u32) {
    let mut s = MODULE.lock();
    for i in 0..MAX_AWARDS {
        if s.awards[i].region_hash == region_hash {
            s.awards[i].sub_found = true;
            drop(s);
            // Finding a sub -> oxytocin bond (partnership)
            endocrine::bond(200);
            serial_println!("[subcontractor_sense] SUB FOUND for region {:08x}", region_hash);
            return;
        }
    }
}

pub fn add_award(region_hash: u32, coverage: u16, deadline_days: u16) {
    register_award(region_hash, coverage, false, deadline_days);
}

pub fn get_avg_coverage() -> u16 { MODULE.lock().avg_coverage }
pub fn get_sub_ema()      -> u16 { MODULE.lock().sub_ema }
