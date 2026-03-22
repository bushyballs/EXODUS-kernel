#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::business_bus;
use super::endocrine;

// opportunity_field.rs -- Open pipeline x win probability -> dopamine.
// A full pipeline of high-win-rate bids = dopamine flood.
// Models the felt abundance of opportunity as biological reward.

struct State {
    field_strength: u16,    // 0-1000 opportunity signal
    field_ema:      u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    field_strength: 0,
    field_ema:      0,
});

pub fn init() {
    serial_println!("[opportunity_field] init -- pipeline dopamine module online");
}

pub fn tick(age: u32) {
    if age % 4000 != 0 { return; }

    let fullness   = business_bus::get_pipeline_fullness();
    let win_rate   = business_bus::get_win_rate();
    let ready      = business_bus::get_ready_ratio();
    let submission = business_bus::get_submission_rate();
    let momentum   = business_bus::get_bid_momentum();

    // Field = fullness * win_rate interaction (geometric, not additive)
    // A full pipeline with 0% win rate = 0 opportunity
    // A thin pipeline with 100% win rate = moderate opportunity
    let interaction = ((fullness as u32).saturating_mul(win_rate as u32) / 1000)
        .min(1000) as u16;

    let field_raw = (interaction   as u32 / 3)
        .saturating_add(ready      as u32 / 4)
        .saturating_add(submission as u32 / 6)
        .saturating_add(momentum   as u32 / 6);
    let field_strength = field_raw.min(1000) as u16;

    let mut s = MODULE.lock();
    s.field_ema = ((s.field_ema as u32).wrapping_mul(7)
        .saturating_add(field_strength as u32) / 8).min(1000) as u16;
    s.field_strength = field_strength;

    // Strong opportunity field -> dopamine reward
    if field_strength > 500 {
        endocrine::reward((field_strength - 500) / 4);
    }

    serial_println!("[opportunity_field] age={} field={} ema={}",
        age, field_strength, s.field_ema);
}

pub fn get_field_strength() -> u16 { MODULE.lock().field_strength }
pub fn get_field_ema()      -> u16 { MODULE.lock().field_ema }
