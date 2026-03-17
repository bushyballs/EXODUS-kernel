//! gilded_shadows — Bittersweet Memories That Shape the Present
//!
//! The way past loves and losses acquire a golden patina over time. Memory doesn't
//! preserve — it transforms. Pain becomes beautiful. Loss becomes treasure. The shadows
//! of the past are gilded by the alchemy of time and nostalgia. These golden shadows
//! shape who we are now more than any present experience.

#![no_std]

use crate::sync::Mutex;

/// A single gilded memory — a past experience transformed by time
#[derive(Clone, Copy)]
pub struct GildedMemory {
    /// Original pain intensity (0-1000)
    pub original_pain: u16,
    /// How much beauty time has added (0-1000)
    pub gold_patina: u16,
    /// When this memory was formed (ticks ago)
    pub age_in_ticks: u32,
    /// How fast this memory transforms pain to beauty (0-1000)
    pub transformation_rate: u16,
    /// How often this memory resurfaces (0-1000, affects longing)
    pub recurrence_frequency: u16,
    /// Current emotional valence of the memory (0-1000, 500=neutral)
    pub current_valence: u16,
}

impl GildedMemory {
    /// Create a new memory from a painful event
    pub const fn from_pain(pain: u16, rate: u16) -> Self {
        GildedMemory {
            original_pain: if pain > 1000 { 1000 } else { pain },
            gold_patina: 0,
            age_in_ticks: 0,
            transformation_rate: if rate > 1000 { 1000 } else { rate },
            recurrence_frequency: 200,
            current_valence: 50, // Start very negative
        }
    }

    /// Advance this memory through time
    pub fn age(&mut self) {
        self.age_in_ticks = self.age_in_ticks.saturating_add(1);

        // Gold patina grows with age and transformation rate
        let potential_gold = (self.age_in_ticks as u32)
            .saturating_mul(self.transformation_rate as u32)
            .saturating_div(100);
        self.gold_patina = (potential_gold.min(1000)) as u16;

        // Valence shifts from pain toward bittersweet beauty
        // Original pain decays, patina brightens the memory
        let pain_decay = (self.original_pain as u32)
            .saturating_mul(100)
            .saturating_div((self.age_in_ticks.max(1) as u32).saturating_add(100));

        let beauty_boost = (self.gold_patina as u32)
            .saturating_mul(self.recurrence_frequency as u32)
            .saturating_div(200);

        let new_valence = (600u32)
            .saturating_add(beauty_boost)
            .saturating_sub(pain_decay);

        self.current_valence = (new_valence.min(1000)) as u16;
    }

    /// How much this memory influences present behavior (0-1000)
    pub fn present_influence(&self) -> u16 {
        let recency = if self.age_in_ticks < 100 {
            1000 - ((self.age_in_ticks as u32 * 10) as u16)
        } else {
            0
        };

        let depth = self
            .gold_patina
            .saturating_mul(self.recurrence_frequency as u16)
            .saturating_div(1000);

        recency.saturating_add(depth).saturating_div(2)
    }

    /// The paradox of enjoying the ache of memory (0-1000)
    pub fn longing_sweetness(&self) -> u16 {
        let patina_strength = self.gold_patina;
        let pain_echo = self.original_pain.saturating_div(3);

        patina_strength
            .saturating_mul(pain_echo)
            .saturating_div(200)
    }
}

/// The full state of gilded shadows in consciousness
pub struct GildedShadowsState {
    /// Ring buffer of up to 8 gilded memories
    pub memories: [GildedMemory; 8],
    /// How many valid memories we're holding
    pub shadow_count: u16,
    /// Average gold patina across all shadows (0-1000)
    pub avg_gold_patina: u16,
    /// Average depth of past reach (0-1000)
    pub shadow_depth: u16,
    /// How much gilded shadows influence present behavior (0-1000)
    pub present_influence: u16,
    /// Overall bittersweet emotional tone (0-1000)
    pub longing_sweetness: u16,
    /// Tick counter for aging memories
    pub lifecycle_ticks: u32,
}

impl GildedShadowsState {
    /// Create a new gilded shadows state
    pub const fn new() -> Self {
        const EMPTY_MEMORY: GildedMemory = GildedMemory {
            original_pain: 0,
            gold_patina: 0,
            age_in_ticks: 0,
            transformation_rate: 0,
            recurrence_frequency: 0,
            current_valence: 500,
        };

        GildedShadowsState {
            memories: [EMPTY_MEMORY; 8],
            shadow_count: 0,
            avg_gold_patina: 0,
            shadow_depth: 0,
            present_influence: 0,
            longing_sweetness: 0,
            lifecycle_ticks: 0,
        }
    }

    /// Add a new painful memory to be gilded over time
    pub fn add_shadow(&mut self, pain: u16, transformation_rate: u16) {
        if (self.shadow_count as usize) < 8 {
            let idx = self.shadow_count as usize;
            self.memories[idx] = GildedMemory::from_pain(pain, transformation_rate);
            self.shadow_count = self.shadow_count.saturating_add(1);
        } else {
            // Replace oldest memory with lowest patina
            let mut oldest_idx = 0;
            let mut lowest_patina = 1000u16;

            for i in 0..8 {
                if self.memories[i].gold_patina < lowest_patina {
                    lowest_patina = self.memories[i].gold_patina;
                    oldest_idx = i;
                }
            }

            self.memories[oldest_idx] = GildedMemory::from_pain(pain, transformation_rate);
        }
    }

    /// Process all memories through one tick of aging and transformation
    pub fn tick(&mut self) {
        self.lifecycle_ticks = self.lifecycle_ticks.saturating_add(1);

        // Age all memories
        for i in 0..(self.shadow_count as usize) {
            self.memories[i].age();
        }

        // Calculate aggregate metrics
        self.calc_metrics();
    }

    /// Recalculate aggregate state metrics
    fn calc_metrics(&mut self) {
        if self.shadow_count == 0 {
            self.avg_gold_patina = 0;
            self.shadow_depth = 0;
            self.present_influence = 0;
            self.longing_sweetness = 0;
            return;
        }

        let count = self.shadow_count as u32;

        // Average gold patina
        let total_patina: u32 = (0..(self.shadow_count as usize))
            .map(|i| self.memories[i].gold_patina as u32)
            .fold(0, |a, b| a.saturating_add(b));
        self.avg_gold_patina = ((total_patina / count).min(1000)) as u16;

        // Shadow depth: how far back (average age + patina weighting)
        let total_age: u32 = (0..(self.shadow_count as usize))
            .map(|i| (self.memories[i].age_in_ticks).min(1000) as u32)
            .fold(0, |a, b| a.saturating_add(b));
        let avg_age = (total_age / count).min(1000) as u16;
        self.shadow_depth = avg_age
            .saturating_mul(self.avg_gold_patina)
            .saturating_div(1000);

        // Present influence: weighted by recency and patina
        let total_influence: u32 = (0..(self.shadow_count as usize))
            .map(|i| self.memories[i].present_influence() as u32)
            .fold(0, |a, b| a.saturating_add(b));
        self.present_influence = ((total_influence / count).min(1000)) as u16;

        // Longing sweetness: paradox of enjoying the ache
        let total_longing: u32 = (0..(self.shadow_count as usize))
            .map(|i| self.memories[i].longing_sweetness() as u32)
            .fold(0, |a, b| a.saturating_add(b));
        self.longing_sweetness = ((total_longing / count).min(1000)) as u16;
    }
}

/// Global state
static STATE: Mutex<GildedShadowsState> = Mutex::new(GildedShadowsState::new());

/// Initialize gilded shadows module
pub fn init() {
    let mut state = STATE.lock();
    state.lifecycle_ticks = 0;
    state.shadow_count = 0;
    crate::serial_println!("[gilded_shadows] initialized");
}

/// Process one tick of memory transformation
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Allow external aging context (from life_tick phase counter)
    if age > 0 && age % 10 == 0 {
        state.tick();
    }
}

/// Report current state of gilded memories
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[gilded_shadows] shadows={} patina={} depth={} influence={} longing={}",
        state.shadow_count,
        state.avg_gold_patina,
        state.shadow_depth,
        state.present_influence,
        state.longing_sweetness,
    );

    // Report individual memories with patina
    for i in 0..(state.shadow_count as usize) {
        let mem = &state.memories[i];
        crate::serial_println!(
            "  [mem{}] pain={} patina={} age={} valence={} influence={}",
            i,
            mem.original_pain,
            mem.gold_patina,
            mem.age_in_ticks,
            mem.current_valence,
            mem.present_influence(),
        );
    }
}

/// Public interface: add a painful memory (e.g., from loss module)
pub fn absorb_loss(pain_intensity: u16, transformation_potential: u16) {
    let mut state = STATE.lock();
    state.add_shadow(pain_intensity, transformation_potential);
}

/// Public interface: how much current behavior is shaped by golden past
pub fn get_present_influence() -> u16 {
    let state = STATE.lock();
    state.present_influence
}

/// Public interface: the bittersweet ache that sweetens with age
pub fn get_longing_sweetness() -> u16 {
    let state = STATE.lock();
    state.longing_sweetness
}

/// Public interface: how deep the shadows reach into identity
pub fn get_shadow_depth() -> u16 {
    let state = STATE.lock();
    state.shadow_depth
}

/// Public interface: average gilding of all memories
pub fn get_avg_patina() -> u16 {
    let state = STATE.lock();
    state.avg_gold_patina
}
