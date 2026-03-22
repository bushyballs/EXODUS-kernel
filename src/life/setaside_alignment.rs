#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::entropy;

// setaside_alignment.rs -- Set-aside eligibility awareness -> competitive alignment.
// Federal set-asides restrict who can bid: Small Business (SB), SDVOSB, 8(a),
// HUBZone, WOSB, or Unrestricted (open competition).
//
// Hoags Inc. eligibility (confirmed):
//   Small Business:     YES (confirmed, all bids qualify)
//   SDVOSB:             NO (Collin is not a service-disabled vet)
//   8(a):               NO (not enrolled in SBA 8a program)
//   HUBZone:            NO (Eugene OR 97402 is not a HUBZone as of 2026)
//   WOSB:               NO (Collin Hoag is male owner)
//   Unrestricted:       YES (all offerors eligible)
//
// Key insight: Unrestricted and Total Small Business are Hoags natural habitat.
// SB set-asides reduce competition from large firms -- good for Hoags.
// Unrestricted = open to all large firms -- harder competition.
//
// From real pipeline: ~70% unrestricted, ~25% total SB, ~5% other set-asides.
//
// Maps to: entropy::increase() (set-asides expand valid bid space within reach),
//          endocrine::reward() (eligible for a competitive advantage set-aside),
//          endocrine::stress() (bidding unrestricted against large corporations).

struct State {
    bids_unrestricted:  u32,    // most competitive, big firms allowed
    bids_small_biz:     u32,    // SB set-aside -- only SBs, Hoags qualifies
    bids_sdvosb:        u32,    // ineligible -- tracked to avoid wasted effort
    bids_8a:            u32,    // ineligible
    bids_hubzone:       u32,    // ineligible
    bids_wosb:          u32,    // ineligible
    alignment_signal:   u16,    // 0-1000: how well pipeline aligns to eligibility
    alignment_ema:      u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    bids_unrestricted:  200,    // ~70% of 281 active bids
    bids_small_biz:      72,    // ~25%
    bids_sdvosb:          5,    // few -- should avoid
    bids_8a:              2,
    bids_hubzone:         2,
    bids_wosb:            0,
    alignment_signal:     0,
    alignment_ema:      600,
});

pub fn init() {
    serial_println!("[setaside_alignment] init -- SB eligible, SDVOSB/8a/HUBZone ineligible");
}

pub fn tick(age: u32) {
    if age % 8000 != 0 { return; }

    let mut s = MODULE.lock();
    let total = (s.bids_unrestricted + s.bids_small_biz + s.bids_sdvosb
        + s.bids_8a + s.bids_hubzone + s.bids_wosb).max(1);

    let eligible  = s.bids_unrestricted + s.bids_small_biz;
    let ineligible = s.bids_sdvosb + s.bids_8a + s.bids_hubzone + s.bids_wosb;

    let align_raw     = ((eligible   as u32 * 1000) / total).min(1000) as u16;
    let inelig_penalty = ((ineligible as u32 * 500) / total).min(300) as u16;
    let sb_bonus      = ((s.bids_small_biz as u32 * 200) / total).min(200) as u16;

    let alignment = align_raw.saturating_sub(inelig_penalty)
        .saturating_add(sb_bonus / 2).min(1000);

    s.alignment_ema = ((s.alignment_ema as u32).wrapping_mul(7)
        .saturating_add(alignment as u32) / 8).min(1000) as u16;
    s.alignment_signal = alignment;
    let ema = s.alignment_ema;
    let sb  = s.bids_small_biz;
    let inelg = ineligible;
    drop(s);

    if alignment > 600 {
        entropy::increase((alignment - 600) / 15);
    }
    if sb > 20 {
        endocrine::reward((sb as u16 * 5).min(150));
    }
    if inelg > 3 {
        endocrine::stress((inelg as u16 * 15).min(150));
    }

    if age % 24000 == 0 {
        serial_println!("[setaside_alignment] age={} eligible={} sb={} inelg={} align={} ema={}",
            age, eligible, sb, inelg, alignment, ema);
    }
}

pub fn get_alignment()     -> u16 { MODULE.lock().alignment_signal }
pub fn get_alignment_ema() -> u16 { MODULE.lock().alignment_ema }
pub fn get_sb_count()      -> u32 { MODULE.lock().bids_small_biz }
pub fn get_ineligible()    -> u32 {
    let s = MODULE.lock();
    s.bids_sdvosb + s.bids_8a + s.bids_hubzone + s.bids_wosb
}
