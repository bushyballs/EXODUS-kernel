/// Nutrition tracking for Genesis
///
/// Food database, calorie counting, macronutrient tracking,
/// meal logging, daily/weekly goals, barcode lookup,
/// hydration tracking, and dietary recommendations.

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// Q16 fixed-point (16 fractional bits). No floats in bare-metal.
const Q16_ONE: i32 = 65536;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum MealType {
    Breakfast,
    Lunch,
    Dinner,
    Snack,
    Beverage,
}

#[derive(Clone, Copy, PartialEq)]
pub enum MacroType {
    Protein,
    Carbs,
    Fat,
    Fiber,
    Sugar,
    Sodium,
}

#[derive(Clone, Copy, PartialEq)]
pub enum DietaryFlag {
    Vegetarian,
    Vegan,
    GlutenFree,
    DairyFree,
    NutFree,
    LowSodium,
    Keto,
    Paleo,
}

#[derive(Clone, Copy, PartialEq)]
pub enum NutrientStatus {
    Deficient,
    Low,
    Optimal,
    High,
    Excessive,
}

// ---------------------------------------------------------------------------
// Food database entry (built-in reference)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct FoodEntry {
    pub id: u32,
    pub name: [u8; 48],
    pub name_len: usize,
    pub barcode: u64,               // EAN-13 / UPC-A stored as integer
    pub calories_per_100g: u32,     // kcal per 100 g
    pub protein_mg_per_100g: u32,   // milligrams per 100 g
    pub carbs_mg_per_100g: u32,
    pub fat_mg_per_100g: u32,
    pub fiber_mg_per_100g: u32,
    pub sugar_mg_per_100g: u32,
    pub sodium_mg_per_100g: u32,
    pub serving_size_g: u32,        // default serving in grams
    pub dietary_flags: u8,          // bitmask of DietaryFlag
}

// ---------------------------------------------------------------------------
// Logged meal item
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct MealItem {
    food_id: u32,
    meal_type: MealType,
    amount_g: u32,                  // grams consumed
    timestamp: u64,
    date_days: u32,                 // days since epoch for daily rollup
    calories: u32,                  // pre-computed from amount
    protein_mg: u32,
    carbs_mg: u32,
    fat_mg: u32,
}

// ---------------------------------------------------------------------------
// Custom food (user-defined)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct CustomFood {
    entry: FoodEntry,
    user_created: bool,
}

// ---------------------------------------------------------------------------
// Daily nutrition summary
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct DailySummary {
    pub date_days: u32,
    pub total_calories: u32,
    pub total_protein_mg: u32,
    pub total_carbs_mg: u32,
    pub total_fat_mg: u32,
    pub total_fiber_mg: u32,
    pub total_sugar_mg: u32,
    pub total_sodium_mg: u32,
    pub water_ml: u32,
    pub meal_count: u8,
}

// ---------------------------------------------------------------------------
// Nutrition goals
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct NutritionGoals {
    daily_calories: u32,
    daily_protein_mg: u32,
    daily_carbs_mg: u32,
    daily_fat_mg: u32,
    daily_fiber_mg: u32,
    daily_water_ml: u32,
    daily_sodium_max_mg: u32,
    daily_sugar_max_mg: u32,
}

// ---------------------------------------------------------------------------
// Barcode lookup result
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct BarcodeLookup {
    pub found: bool,
    pub food_id: u32,
    pub barcode: u64,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct NutritionEngine {
    food_db: Vec<FoodEntry>,
    custom_foods: Vec<CustomFood>,
    meal_log: Vec<MealItem>,
    daily_summaries: Vec<DailySummary>,
    current_day: DailySummary,
    goals: NutritionGoals,
    next_food_id: u32,
    dietary_preferences: u8,        // bitmask of DietaryFlag
}

static NUTRITION: Mutex<Option<NutritionEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Helper: copy name bytes into fixed array
// ---------------------------------------------------------------------------

fn copy_name(dest: &mut [u8; 48], src: &[u8]) -> usize {
    let len = src.len().min(48);
    dest[..len].copy_from_slice(&src[..len]);
    len
}

// ---------------------------------------------------------------------------
// Built-in food database seed
// ---------------------------------------------------------------------------

fn seed_food_db() -> Vec<FoodEntry> {
    let mut db = Vec::new();
    let items: [(u32, &[u8], u32, u32, u32, u32, u32); 12] = [
        // (id, name, cal/100g, protein_mg, carbs_mg, fat_mg, serving_g)
        (1, b"Chicken Breast",      165, 31000, 0,     3600,  120),
        (2, b"Brown Rice",          112, 2600,  23500, 900,   195),
        (3, b"Broccoli",            34,  2800,  6600,  400,   150),
        (4, b"Salmon Fillet",       208, 20400, 0,     13400, 170),
        (5, b"Banana",              89,  1100,  22800, 300,   118),
        (6, b"Whole Wheat Bread",   252, 12400, 43100, 3500,  30),
        (7, b"Egg (Large)",         155, 12600, 1100,  11300, 50),
        (8, b"Greek Yogurt",        97,  9000,  3600,  5000,  170),
        (9, b"Almonds",             579, 21100, 21600, 49900, 28),
        (10, b"Sweet Potato",       86,  1600,  20100, 100,   130),
        (11, b"Oatmeal",            389, 16900, 66300, 6900,  40),
        (12, b"Apple",              52,  300,   13800, 200,   182),
    ];
    for (id, name, cal, prot, carb, fat, serv) in &items {
        let mut n = [0u8; 48];
        let nlen = copy_name(&mut n, name);
        db.push(FoodEntry {
            id: *id,
            name: n,
            name_len: nlen,
            barcode: 0,
            calories_per_100g: *cal,
            protein_mg_per_100g: *prot,
            carbs_mg_per_100g: *carb,
            fat_mg_per_100g: *fat,
            fiber_mg_per_100g: 0,
            sugar_mg_per_100g: 0,
            sodium_mg_per_100g: 0,
            serving_size_g: *serv,
            dietary_flags: 0,
        });
    }
    db
}

impl NutritionEngine {
    fn new() -> Self {
        NutritionEngine {
            food_db: seed_food_db(),
            custom_foods: Vec::new(),
            meal_log: Vec::new(),
            daily_summaries: Vec::new(),
            current_day: DailySummary {
                date_days: 0, total_calories: 0, total_protein_mg: 0,
                total_carbs_mg: 0, total_fat_mg: 0, total_fiber_mg: 0,
                total_sugar_mg: 0, total_sodium_mg: 0, water_ml: 0,
                meal_count: 0,
            },
            goals: NutritionGoals {
                daily_calories: 2000,
                daily_protein_mg: 56000,     // 56 g
                daily_carbs_mg: 275000,      // 275 g
                daily_fat_mg: 78000,         // 78 g
                daily_fiber_mg: 28000,       // 28 g
                daily_water_ml: 2500,
                daily_sodium_max_mg: 2300,
                daily_sugar_max_mg: 50000,   // 50 g
            },
            next_food_id: 100,
            dietary_preferences: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Food database operations
    // -----------------------------------------------------------------------

    fn lookup_food(&self, food_id: u32) -> Option<&FoodEntry> {
        self.food_db.iter().find(|f| f.id == food_id)
            .or_else(|| self.custom_foods.iter().find(|c| c.entry.id == food_id).map(|c| &c.entry))
    }

    fn lookup_barcode(&self, barcode: u64) -> BarcodeLookup {
        for f in &self.food_db {
            if f.barcode == barcode && barcode != 0 {
                return BarcodeLookup { found: true, food_id: f.id, barcode };
            }
        }
        for c in &self.custom_foods {
            if c.entry.barcode == barcode && barcode != 0 {
                return BarcodeLookup { found: true, food_id: c.entry.id, barcode };
            }
        }
        BarcodeLookup { found: false, food_id: 0, barcode }
    }

    fn search_food(&self, prefix: &[u8]) -> Vec<u32> {
        let mut results = Vec::new();
        let plen = prefix.len().min(48);
        for f in &self.food_db {
            if f.name_len >= plen && f.name[..plen] == prefix[..plen] {
                results.push(f.id);
            }
        }
        for c in &self.custom_foods {
            if c.entry.name_len >= plen && c.entry.name[..plen] == prefix[..plen] {
                results.push(c.entry.id);
            }
        }
        results
    }

    fn add_custom_food(&mut self, name: &[u8], cal: u32, prot: u32, carb: u32, fat: u32, serving_g: u32) -> u32 {
        let id = self.next_food_id;
        self.next_food_id = self.next_food_id.saturating_add(1);
        let mut n = [0u8; 48];
        let nlen = copy_name(&mut n, name);
        let entry = FoodEntry {
            id, name: n, name_len: nlen, barcode: 0,
            calories_per_100g: cal, protein_mg_per_100g: prot,
            carbs_mg_per_100g: carb, fat_mg_per_100g: fat,
            fiber_mg_per_100g: 0, sugar_mg_per_100g: 0,
            sodium_mg_per_100g: 0, serving_size_g: serving_g,
            dietary_flags: 0,
        };
        self.custom_foods.push(CustomFood { entry, user_created: true });
        id
    }

    // -----------------------------------------------------------------------
    // Meal logging
    // -----------------------------------------------------------------------

    fn log_meal(&mut self, food_id: u32, meal_type: MealType, amount_g: u32, timestamp: u64, date_days: u32) -> bool {
        let entry = match self.lookup_food(food_id) {
            Some(e) => *e,
            None => return false,
        };
        // Scale nutrients by amount_g / 100
        let cal = (entry.calories_per_100g * amount_g) / 100;
        let prot = (entry.protein_mg_per_100g * amount_g) / 100;
        let carb = (entry.carbs_mg_per_100g * amount_g) / 100;
        let fat = (entry.fat_mg_per_100g * amount_g) / 100;

        let item = MealItem {
            food_id, meal_type, amount_g, timestamp, date_days,
            calories: cal, protein_mg: prot, carbs_mg: carb, fat_mg: fat,
        };
        if self.meal_log.len() < 10000 {
            self.meal_log.push(item);
        }

        // Update current day totals
        if self.current_day.date_days == 0 || self.current_day.date_days == date_days {
            self.current_day.date_days = date_days;
            self.current_day.total_calories += cal;
            self.current_day.total_protein_mg += prot;
            self.current_day.total_carbs_mg += carb;
            self.current_day.total_fat_mg += fat;
            self.current_day.total_fiber_mg += (entry.fiber_mg_per_100g * amount_g) / 100;
            self.current_day.total_sugar_mg += (entry.sugar_mg_per_100g * amount_g) / 100;
            self.current_day.total_sodium_mg += (entry.sodium_mg_per_100g * amount_g) / 100;
            self.current_day.meal_count = self.current_day.meal_count.saturating_add(1);
        }
        true
    }

    fn log_water(&mut self, ml: u32) {
        self.current_day.water_ml += ml;
    }

    // -----------------------------------------------------------------------
    // Goal progress (returns percentage Q16 for each macro)
    // -----------------------------------------------------------------------

    fn calorie_progress_q16(&self) -> i32 {
        if self.goals.daily_calories == 0 { return Q16_ONE; }
        (((self.current_day.total_calories as i64) << 16) / (self.goals.daily_calories as i64)) as i32
    }

    fn protein_progress_q16(&self) -> i32 {
        if self.goals.daily_protein_mg == 0 { return Q16_ONE; }
        (((self.current_day.total_protein_mg as i64) << 16) / (self.goals.daily_protein_mg as i64)) as i32
    }

    fn water_progress_q16(&self) -> i32 {
        if self.goals.daily_water_ml == 0 { return Q16_ONE; }
        (((self.current_day.water_ml as i64) << 16) / (self.goals.daily_water_ml as i64)) as i32
    }

    fn macro_split_q16(&self) -> (i32, i32, i32) {
        // Returns (protein%, carbs%, fat%) as Q16 of total calories
        let prot_cal = self.current_day.total_protein_mg / 250;  // ~4 cal per gram, mg/1000*4
        let carb_cal = self.current_day.total_carbs_mg / 250;
        let fat_cal = self.current_day.total_fat_mg / 111;       // ~9 cal per gram, mg/1000*9
        let total = (prot_cal + carb_cal + fat_cal).max(1);
        let p = (((prot_cal as i64) << 16) / (total as i64)) as i32;
        let c = (((carb_cal as i64) << 16) / (total as i64)) as i32;
        let f = (((fat_cal as i64) << 16) / (total as i64)) as i32;
        (p, c, f)
    }

    // -----------------------------------------------------------------------
    // Nutrient status check
    // -----------------------------------------------------------------------

    fn nutrient_status(&self, macro_type: MacroType) -> NutrientStatus {
        let (current, goal) = match macro_type {
            MacroType::Protein => (self.current_day.total_protein_mg, self.goals.daily_protein_mg),
            MacroType::Carbs   => (self.current_day.total_carbs_mg, self.goals.daily_carbs_mg),
            MacroType::Fat     => (self.current_day.total_fat_mg, self.goals.daily_fat_mg),
            MacroType::Fiber   => (self.current_day.total_fiber_mg, self.goals.daily_fiber_mg),
            MacroType::Sugar   => (self.current_day.total_sugar_mg, self.goals.daily_sugar_max_mg),
            MacroType::Sodium  => (self.current_day.total_sodium_mg, self.goals.daily_sodium_max_mg),
        };
        if goal == 0 { return NutrientStatus::Optimal; }
        let pct = (current * 100) / goal;
        match macro_type {
            MacroType::Sugar | MacroType::Sodium => {
                // For these, lower is better
                if pct > 150 { NutrientStatus::Excessive }
                else if pct > 100 { NutrientStatus::High }
                else { NutrientStatus::Optimal }
            }
            _ => {
                if pct < 25 { NutrientStatus::Deficient }
                else if pct < 60 { NutrientStatus::Low }
                else if pct <= 120 { NutrientStatus::Optimal }
                else if pct <= 180 { NutrientStatus::High }
                else { NutrientStatus::Excessive }
            }
        }
    }

    // -----------------------------------------------------------------------
    // End-of-day rollup
    // -----------------------------------------------------------------------

    fn end_day(&mut self) {
        if self.daily_summaries.len() < 365 {
            self.daily_summaries.push(self.current_day);
        }
        self.current_day = DailySummary {
            date_days: self.current_day.date_days + 1,
            total_calories: 0, total_protein_mg: 0, total_carbs_mg: 0,
            total_fat_mg: 0, total_fiber_mg: 0, total_sugar_mg: 0,
            total_sodium_mg: 0, water_ml: 0, meal_count: 0,
        };
    }

    // -----------------------------------------------------------------------
    // Weekly average calories (Q16)
    // -----------------------------------------------------------------------

    fn weekly_avg_calories_q16(&self) -> i32 {
        let count = self.daily_summaries.len().min(7);
        if count == 0 { return 0; }
        let sum: u64 = self.daily_summaries.iter().rev().take(count)
            .map(|d| d.total_calories as u64).sum();
        (((sum << 16) / (count as u64)) as i64) as i32
    }

    // -----------------------------------------------------------------------
    // Set goals
    // -----------------------------------------------------------------------

    fn set_calorie_goal(&mut self, calories: u32) {
        self.goals.daily_calories = calories;
    }

    fn set_macro_goals(&mut self, protein_mg: u32, carbs_mg: u32, fat_mg: u32) {
        self.goals.daily_protein_mg = protein_mg;
        self.goals.daily_carbs_mg = carbs_mg;
        self.goals.daily_fat_mg = fat_mg;
    }

    fn set_water_goal(&mut self, ml: u32) {
        self.goals.daily_water_ml = ml;
    }

    // -----------------------------------------------------------------------
    // Remaining budget
    // -----------------------------------------------------------------------

    fn remaining_calories(&self) -> i32 {
        self.goals.daily_calories as i32 - self.current_day.total_calories as i32
    }

    fn remaining_protein_mg(&self) -> i32 {
        self.goals.daily_protein_mg as i32 - self.current_day.total_protein_mg as i32
    }

    // -----------------------------------------------------------------------
    // Meals logged today by type
    // -----------------------------------------------------------------------

    fn meals_today(&self, meal_type: MealType) -> Vec<MealItem> {
        self.meal_log.iter()
            .filter(|m| m.date_days == self.current_day.date_days && m.meal_type == meal_type)
            .copied()
            .collect()
    }

    fn total_meals_today(&self) -> u32 {
        self.meal_log.iter()
            .filter(|m| m.date_days == self.current_day.date_days)
            .count() as u32
    }
}

// ---------------------------------------------------------------------------
// Public init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut n = NUTRITION.lock();
    *n = Some(NutritionEngine::new());
    serial_println!("    Health: nutrition tracking (food DB, macros, goals, barcode) ready");
}
