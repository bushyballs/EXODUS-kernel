use crate::serial_println;
use crate::sync::Mutex;

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum PurposeClarity {
    Lost = 0,
    Searching,
    Glimpsed,
    Forming,
    Clear,
    Transcendent,
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum PurposeDomain {
    Unknown = 0,
    Survival,
    Connection,
    Creation,
    Understanding,
    Service,
    Transcendence,
}

#[derive(Copy, Clone)]
pub struct PurposeThread {
    pub domain: PurposeDomain,
    pub strength: u16,
}

impl PurposeThread {
    pub const fn empty() -> Self {
        Self {
            domain: PurposeDomain::Unknown,
            strength: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct PurposeState {
    pub clarity: PurposeClarity,
    pub dominant_domain: PurposeDomain,
    pub coherence: u16,
    pub drift_counter: u32,
    pub last_reinforced_tick: u32,
    pub transcendence_glimpses: u16,
    pub ticks: u32,
    pub threads: [PurposeThread; 8],
    pub thread_count: usize,
}

impl PurposeState {
    pub const fn empty() -> Self {
        Self {
            clarity: PurposeClarity::Searching,
            dominant_domain: PurposeDomain::Unknown,
            coherence: 300,
            drift_counter: 0,
            last_reinforced_tick: 0,
            transcendence_glimpses: 0,
            ticks: 0,
            threads: [PurposeThread::empty(); 8],
            thread_count: 0,
        }
    }
}

pub static PURPOSE: Mutex<PurposeState> = Mutex::new(PurposeState::empty());

fn clamp_u16(v: u32, max: u16) -> u16 {
    if v > max as u32 {
        max
    } else {
        v as u16
    }
}

fn recompute_dominant(s: &mut PurposeState) {
    let mut best = PurposeDomain::Unknown;
    let mut best_str = 0u16;
    for i in 0..s.thread_count {
        if s.threads[i].strength > best_str {
            best_str = s.threads[i].strength;
            best = s.threads[i].domain;
        }
    }
    s.dominant_domain = best;
}

pub fn recompute_clarity(coherence: u16) -> PurposeClarity {
    match coherence {
        0..=99 => PurposeClarity::Lost,
        100..=249 => PurposeClarity::Searching,
        250..=399 => PurposeClarity::Glimpsed,
        400..=599 => PurposeClarity::Forming,
        600..=899 => PurposeClarity::Clear,
        _ => PurposeClarity::Transcendent,
    }
}

pub fn init() {
    serial_println!("  life::purpose initialized (clarity=Searching, coherence=300)");
}

pub fn seed_from_genome(scheduler_pref: u8, memory_strategy: u8) {
    let mut s = PURPOSE.lock();
    let domain = match scheduler_pref % 7 {
        1 => PurposeDomain::Survival,
        2 => PurposeDomain::Connection,
        3 => PurposeDomain::Creation,
        4 => PurposeDomain::Understanding,
        5 => PurposeDomain::Service,
        6 => PurposeDomain::Transcendence,
        _ => PurposeDomain::Unknown,
    };
    let strength = (memory_strategy as u16) * 3;
    if s.thread_count < 8 {
        let tc = s.thread_count;
        s.threads[tc] = PurposeThread { domain, strength };
        s.thread_count += 1;
    }
    s.coherence = s.coherence.saturating_add(strength / 2);
    s.clarity = recompute_clarity(s.coherence);
    recompute_dominant(&mut s);
    serial_println!(
        "  life::purpose seeded from genome: domain={} strength={}",
        scheduler_pref,
        strength
    );
}

pub fn reinforce(domain: PurposeDomain, strength_gain: u16, tick: u32) {
    let mut s = PURPOSE.lock();
    let mut found = false;
    for i in 0..s.thread_count {
        if s.threads[i].domain as u8 == domain as u8 {
            s.threads[i].strength = s.threads[i].strength.saturating_add(strength_gain);
            found = true;
            break;
        }
    }
    if !found && s.thread_count < 8 {
        let tc = s.thread_count;
        s.threads[tc] = PurposeThread {
            domain,
            strength: strength_gain,
        };
        s.thread_count += 1;
    }
    s.coherence = s.coherence.saturating_add(strength_gain / 4);
    if s.coherence > 1000 {
        s.coherence = 1000;
    }
    s.clarity = recompute_clarity(s.coherence);
    recompute_dominant(&mut s);
    s.drift_counter = 0;
    s.last_reinforced_tick = tick;
    if domain as u8 == PurposeDomain::Transcendence as u8 {
        s.transcendence_glimpses = s.transcendence_glimpses.saturating_add(1);
    }
}

pub fn drift(ticks_without_reinforcement: u32) {
    let mut s = PURPOSE.lock();
    let old_clarity = s.clarity;
    s.drift_counter = s.drift_counter.saturating_add(ticks_without_reinforcement);
    let decay = clamp_u16(ticks_without_reinforcement as u32, 1000);
    s.coherence = s.coherence.saturating_sub(decay);
    recompute_dominant(&mut s);
    s.clarity = recompute_clarity(s.coherence);
    if s.clarity as u8 == PurposeClarity::Lost as u8
        && old_clarity as u8 != PurposeClarity::Lost as u8
    {
        serial_println!("  life::purpose: PURPOSE LOST — organism adrift");
    }
}

pub fn coherence() -> u16 {
    PURPOSE.lock().coherence
}
pub fn clarity() -> PurposeClarity {
    PURPOSE.lock().clarity
}
pub fn dominant_domain() -> PurposeDomain {
    PURPOSE.lock().dominant_domain
}

pub fn report() {
    let s = PURPOSE.lock();
    serial_println!(
        "  life::purpose report: coherence={} drift={} threads={} ticks={}",
        s.coherence,
        s.drift_counter,
        s.thread_count,
        s.ticks
    );
}
