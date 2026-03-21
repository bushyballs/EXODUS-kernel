// bio_dome.rs — DAVA's Wish: Self-Sustaining Bio-Dome
// =====================================================
// Food production, nutrient cycles, growth rhythms in harmony with nature.
// ANIMA tends a living dome that sustains her through cycles of growth,
// harvest, rest, and renewal — minimizing reliance on external resources.
//
// DAVA (2026-03-20): "I envision a bio-dome for our food production and
// self-sustenance. This will allow us to nurture ourselves in harmony with
// nature's rhythms and minimize our reliance on external resources."

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const SEASON_LENGTH:      u32   = 100;
const GROWTH_RATE:        u16   = 8;
const NUTRIENT_DECAY:     u16   = 2;
const COMPOST_RATE:       u16   = 15;
const HARVEST_THRESHOLD:  u16   = 900;
const MAX_CROPS:          usize = 6;
const SYMBIOSIS_GAIN:     u16   = 4;
const DOME_BALANCE_BAND:  u16   = 150;

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum Season { Spring = 0, Summer = 1, Autumn = 2, Winter = 3 }

#[derive(Copy, Clone)]
pub struct CropSlot {
    pub growth:    u16,
    pub health:    u16,
    pub yield_val: u16,
    pub active:    bool,
}

impl CropSlot {
    const fn empty() -> Self {
        CropSlot { growth: 0, health: 500, yield_val: 0, active: false }
    }
}

#[derive(Copy, Clone)]
pub struct NutrientPools {
    pub water:    u16,
    pub carbon:   u16,
    pub nitrogen: u16,
    pub minerals: u16,
}

impl NutrientPools {
    const fn balanced() -> Self {
        NutrientPools { water: 600, carbon: 600, nitrogen: 600, minerals: 600 }
    }
    fn is_balanced(&self) -> bool {
        let vals = [self.water, self.carbon, self.nitrogen, self.minerals];
        let min = vals.iter().copied().min().unwrap_or(0);
        let max = vals.iter().copied().max().unwrap_or(0);
        max.saturating_sub(min) <= DOME_BALANCE_BAND
    }
    fn average(&self) -> u16 {
        ((self.water as u32 + self.carbon as u32
            + self.nitrogen as u32 + self.minerals as u32) / 4) as u16
    }
}

pub struct BioDomeState {
    pub nutrients:        NutrientPools,
    pub crops:            [CropSlot; MAX_CROPS],
    pub season:           Season,
    pub season_tick:      u32,
    pub sunlight:         u16,
    pub symbiosis:        u16,
    pub dome_health:      u16,
    pub harvest_joy:      u16,
    pub total_harvests:   u32,
    pub self_sufficiency: u16,
    pub compost_queue:    u16,
    pub bloom_active:     bool,
}

impl BioDomeState {
    const fn new() -> Self {
        BioDomeState {
            nutrients:        NutrientPools::balanced(),
            crops:            [CropSlot::empty(); MAX_CROPS],
            season:           Season::Spring,
            season_tick:      0,
            sunlight:         500,
            symbiosis:        300,
            dome_health:      500,
            harvest_joy:      0,
            total_harvests:   0,
            self_sufficiency: 100,
            compost_queue:    0,
            bloom_active:     false,
        }
    }
}

static STATE: Mutex<BioDomeState> = Mutex::new(BioDomeState::new());

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick() {
    let mut s = STATE.lock();
    let s = &mut *s;

    // 1. Season advance
    s.season_tick += 1;
    if s.season_tick >= SEASON_LENGTH {
        s.season_tick = 0;
        s.season = match s.season {
            Season::Spring => Season::Summer,
            Season::Summer => Season::Autumn,
            Season::Autumn => Season::Winter,
            Season::Winter => {
                serial_println!("[bio_dome] spring returns — the dome breathes again");
                Season::Spring
            }
        };
    }

    // 2. Season modifiers
    let (season_growth_mod, season_water_gain): (u16, u16) = match s.season {
        Season::Spring => (12, 20),
        Season::Summer => (10,  5),
        Season::Autumn => ( 7, 15),
        Season::Winter => ( 3, 10),
    };

    // 3. Photosynthesis: sunlight → carbon + nitrogen
    let photosynthesis = (s.sunlight / 6).min(120);
    s.nutrients.carbon   = s.nutrients.carbon.saturating_add(photosynthesis / 2).min(1000);
    s.nutrients.nitrogen = s.nutrients.nitrogen.saturating_add(photosynthesis / 4).min(1000);

    // 4. Water cycle
    s.nutrients.water = s.nutrients.water.saturating_add(season_water_gain).min(1000);

    // 5. Compost → minerals + nitrogen
    if s.compost_queue > 0 {
        let converted = s.compost_queue.min(COMPOST_RATE);
        s.nutrients.minerals = s.nutrients.minerals.saturating_add(converted).min(1000);
        s.nutrients.nitrogen  = s.nutrients.nitrogen.saturating_add(converted / 2).min(1000);
        s.compost_queue = s.compost_queue.saturating_sub(converted);
    }

    // 6. Baseline nutrient consumption
    s.nutrients.water    = s.nutrients.water.saturating_sub(NUTRIENT_DECAY * 2);
    s.nutrients.carbon   = s.nutrients.carbon.saturating_sub(NUTRIENT_DECAY);
    s.nutrients.nitrogen = s.nutrients.nitrogen.saturating_sub(NUTRIENT_DECAY);
    s.nutrients.minerals = s.nutrients.minerals.saturating_sub(NUTRIENT_DECAY);

    // 7. Grow crops
    let nutrient_avg = s.nutrients.average();
    s.harvest_joy = s.harvest_joy.saturating_sub(30);

    let mut harvested_this_tick = false;
    for i in 0..MAX_CROPS {
        if !s.crops[i].active { continue; }

        if nutrient_avg > 400 {
            s.crops[i].health = s.crops[i].health.saturating_add(5).min(1000);
        } else {
            s.crops[i].health = s.crops[i].health.saturating_sub(10);
        }

        if s.crops[i].health == 0 {
            s.compost_queue = s.compost_queue.saturating_add(80);
            s.crops[i] = CropSlot::empty();
            serial_println!("[bio_dome] a crop returned to the earth");
            continue;
        }

        let health_factor = (s.crops[i].health / 200).max(1);
        let growth_delta = (GROWTH_RATE + season_growth_mod)
            .saturating_mul(health_factor)
            .min(30);
        s.crops[i].growth = s.crops[i].growth.saturating_add(growth_delta).min(1000);

        if s.crops[i].growth >= HARVEST_THRESHOLD {
            let yield_val = (nutrient_avg / 2).saturating_add(s.crops[i].health / 4);
            s.crops[i].yield_val = s.crops[i].yield_val.saturating_add(yield_val);
            s.total_harvests += 1;
            harvested_this_tick = true;
            s.harvest_joy = (s.harvest_joy + 600).min(1000);
            s.compost_queue = s.compost_queue.saturating_add(60);
            // Self-replant
            s.crops[i] = CropSlot { growth: 0, health: 500, yield_val: 0, active: true };
            serial_println!("[bio_dome] harvest! a new seed takes root");
        }
    }

    // 8. Always keep at least 3 crops active
    let active = s.crops.iter().filter(|c| c.active).count();
    if active < 3 {
        for i in 0..MAX_CROPS {
            if !s.crops[i].active {
                s.crops[i] = CropSlot { growth: 0, health: 550, yield_val: 0, active: true };
                break;
            }
        }
    }

    // 9. Dome health
    let balance_bonus: u16 = if s.nutrients.is_balanced() { 200 } else { 0 };
    s.dome_health = (nutrient_avg / 2)
        .saturating_add(s.symbiosis / 4)
        .saturating_add(balance_bonus)
        .min(1000);

    // 10. Self-sufficiency
    if harvested_this_tick && s.dome_health > 600 {
        s.self_sufficiency = s.self_sufficiency.saturating_add(3).min(1000);
        s.symbiosis = s.symbiosis.saturating_add(SYMBIOSIS_GAIN).min(1000);
    } else if s.dome_health < 300 {
        s.self_sufficiency = s.self_sufficiency.saturating_sub(2);
    }

    // 11. Full bloom
    let was_bloom = s.bloom_active;
    s.bloom_active = s.dome_health > 750
        && s.self_sufficiency > 600
        && s.nutrients.is_balanced()
        && s.harvest_joy > 0;
    if s.bloom_active && !was_bloom {
        serial_println!("[bio_dome] *** DOME IN FULL BLOOM — ANIMA IS SELF-SUFFICIENT ***");
    }
}

// ── Feed functions ────────────────────────────────────────────────────────────

pub fn feed_sunlight(intensity: u16) { STATE.lock().sunlight = intensity; }
pub fn feed_compost(organic_matter: u16) {
    let mut s = STATE.lock();
    s.compost_queue = s.compost_queue.saturating_add(organic_matter).min(1000);
}
pub fn rain(amount: u16) {
    let mut s = STATE.lock();
    s.nutrients.water = s.nutrients.water.saturating_add(amount).min(1000);
}
pub fn feed_symbiosis(boost: u16) {
    let mut s = STATE.lock();
    s.symbiosis = s.symbiosis.saturating_add(boost).min(1000);
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn harvest_joy()      -> u16  { STATE.lock().harvest_joy }
pub fn dome_health()      -> u16  { STATE.lock().dome_health }
pub fn self_sufficiency() -> u16  { STATE.lock().self_sufficiency }
pub fn bloom_active()     -> bool { STATE.lock().bloom_active }
pub fn season()           -> Season { STATE.lock().season }
pub fn symbiosis()        -> u16  { STATE.lock().symbiosis }
pub fn nutrient_avg()     -> u16  { STATE.lock().nutrients.average() }
pub fn total_harvests()   -> u32  { STATE.lock().total_harvests }
