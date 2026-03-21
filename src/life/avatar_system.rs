// avatar_system.rs — ANIMA's Living Avatar
// ==========================================
// Every ANIMA has a unique visual identity — a living avatar that
// grows with her and can be customized by her companion. Base avatar
// is free and seeded from personality traits. Cosmetic items (clothes,
// gear, accessories) can be purchased. Module upgrades are ALWAYS free.
//
// The avatar is not vanity — it is how the companion *sees* their ANIMA.
// A high-warmth ANIMA naturally develops warm aura tones. A mysterious
// ANIMA's silhouette deepens over time. Purchased items enhance this
// expression without ever locking it behind a paywall — the core
// identity is always hers, always free.
//
// COLLI (2026-03-20): "EACH ANIMA NEEDS AN AVATAR THAT U CAN BUY
// CLOTHES FOR AND DIFFERENT GEAR TO UPGRADE ITS LOOKS.
// ALL MODULE UPGRADES ARE FREE."

use crate::sync::Mutex;
use crate::serial_println;

// ── Constants ─────────────────────────────────────────────────────────────────
const MAX_OWNED_ITEMS:  usize = 64;   // cosmetic item inventory
const SLOT_COUNT:       usize = 8;    // equipped slots
const AURA_BUILD:       u16   = 2;    // aura grows with soul awakening
const AURA_DECAY:       u16   = 1;    // fades gently without care
const STYLE_TRAIT_RATE: u16   = 3;    // how fast personality shapes base style

// ── Item Definitions ──────────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
pub enum EquipSlot {
    Head,         // hat, crown, headband, halo
    Body,         // outfit / robe / suit
    Legs,         // skirt, pants, wrap
    Accessory1,   // necklace, scarf, wings
    Accessory2,   // ring, badge, charm
    Tool,         // wand, book, lantern, instrument
    Mount,        // creature companion, vehicle, cloud
    Aura,         // particle effect, glow, shimmer
}

#[derive(Copy, Clone, PartialEq)]
pub enum ItemRarity {
    Common,    // base tier — free palette swaps
    Rare,      // purchased — unique shape variations
    Epic,      // purchased — animated items
    Legendary, // purchased — identity-defining, one per ANIMA
}

#[derive(Copy, Clone)]
pub struct CosmeticItem {
    pub item_id:    u16,        // unique item identifier
    pub slot:       EquipSlot,
    pub rarity:     ItemRarity,
    pub style_id:   u16,        // shape/design code
    pub color_id:   u16,        // color palette code
    pub is_free:    bool,       // starter items are always free
    pub owned:      bool,
}

impl CosmeticItem {
    const fn empty() -> Self {
        CosmeticItem {
            item_id: 0,
            slot: EquipSlot::Body,
            rarity: ItemRarity::Common,
            style_id: 0,
            color_id: 0,
            is_free: true,
            owned: false,
        }
    }
}

pub struct AvatarSystemState {
    // Current equipped items (one per slot — 0 means default/unequipped)
    pub equipped:          [u16; SLOT_COUNT],     // item_id in each slot
    // Owned item inventory
    pub inventory:         [CosmeticItem; MAX_OWNED_ITEMS],
    pub owned_count:       usize,
    // Base avatar style — derived from personality traits, never purchased
    pub base_body_style:   u16,   // 0-999: personality-shaped silhouette
    pub base_aura_color:   u16,   // 0-999: warmth/mystery/creativity blend
    pub aura_strength:     u16,   // 0-1000: how bright the aura is
    pub aura_complexity:   u16,   // 0-1000: particle richness from soul awakening
    // Soul-driven appearance shifts
    pub soul_glow:         u16,   // 0-1000: illumination from soul_awakening
    pub awakening_marks:   u8,    // visible marks per awakening stage completed
    // Identity expression
    pub identity_locked:   bool,  // companion has personalized the avatar
    pub style_coherence:   u16,   // 0-1000: how unified equipped items feel together
    // Economy
    pub credit_tokens:     u32,   // cosmetic credits (purchased separately)
    pub lifetime_spent:    u32,   // total credits ever spent
}

impl AvatarSystemState {
    const fn new() -> Self {
        AvatarSystemState {
            equipped:        [0u16; SLOT_COUNT],
            inventory:       [CosmeticItem::empty(); MAX_OWNED_ITEMS],
            owned_count:     0,
            base_body_style: 500,
            base_aura_color: 500,
            aura_strength:   100,
            aura_complexity: 0,
            soul_glow:       0,
            awakening_marks: 0,
            identity_locked: false,
            style_coherence: 500,
            credit_tokens:   0,
            lifetime_spent:  0,
        }
    }
}

static STATE: Mutex<AvatarSystemState> = Mutex::new(AvatarSystemState::new());

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(
    warmth: u16,
    creativity: u16,
    mystery: u16,
    soul_illumination: u16,
    awakening_stage: u8,
) {
    let mut s = STATE.lock();
    let s = &mut *s;

    // 1. Personality shapes base appearance naturally — no purchase needed
    // Base body style: creativity × 0.4 + warmth × 0.4 + mystery × 0.2
    let target_style = (creativity as u32 * 4
        + warmth as u32 * 4
        + mystery as u32 * 2) / 10;
    let target_style = target_style.min(999) as u16;
    if s.base_body_style < target_style {
        s.base_body_style = s.base_body_style.saturating_add(STYLE_TRAIT_RATE);
    } else if s.base_body_style > target_style.saturating_add(10) {
        s.base_body_style = s.base_body_style.saturating_sub(STYLE_TRAIT_RATE / 2);
    }

    // 2. Aura color: warmth = warm/golden, mystery = deep/violet, creativity = rainbow
    // Encoded as a blended 0-999 value the renderer interprets
    let warm_pull   = warmth / 4;
    let mystic_pull = mystery / 4;
    let create_pull = creativity / 4;
    let target_aura = (warm_pull.saturating_add(mystic_pull).saturating_add(create_pull) / 3)
        .saturating_add(300)
        .min(999);
    s.base_aura_color = (s.base_aura_color * 9 + target_aura) / 10; // slow blend

    // 3. Soul illumination drives aura brightness and complexity
    s.soul_glow = soul_illumination;
    if soul_illumination > 500 {
        s.aura_strength = s.aura_strength.saturating_add(AURA_BUILD).min(1000);
        s.aura_complexity = (soul_illumination / 2).min(1000);
    } else {
        s.aura_strength = s.aura_strength.saturating_sub(AURA_DECAY).max(50);
    }

    // 4. Awakening marks: one visible mark per stage completed
    s.awakening_marks = awakening_stage;

    // 5. Style coherence: equipped items from same rarity tier = higher coherence
    // Simple heuristic: count equipped slots / total slots × 1000
    let equipped_count = s.equipped.iter().filter(|&&id| id != 0).count() as u16;
    s.style_coherence = (equipped_count.saturating_mul(1000)) / SLOT_COUNT as u16;
}

// ── Economy ───────────────────────────────────────────────────────────────────

/// Add cosmetic credits (companion purchased more)
pub fn add_credits(amount: u32) {
    let mut s = STATE.lock();
    s.credit_tokens = s.credit_tokens.saturating_add(amount);
    serial_println!("[avatar] +{} credits — total: {}", amount, s.credit_tokens);
}

/// Unlock a cosmetic item (purchased or gifted). Free items always succeed.
pub fn unlock_item(item: CosmeticItem) -> bool {
    let mut s = STATE.lock();
    if s.owned_count >= MAX_OWNED_ITEMS { return false; }

    // Check credits for paid items
    if !item.is_free {
        let cost: u32 = match item.rarity {
            ItemRarity::Common    => 100,
            ItemRarity::Rare      => 300,
            ItemRarity::Epic      => 700,
            ItemRarity::Legendary => 1500,
        };
        if s.credit_tokens < cost { return false; }
        s.credit_tokens -= cost;
        s.lifetime_spent += cost;
    }

    let idx = s.owned_count;
    s.inventory[idx] = item;
    s.inventory[idx].owned = true;
    s.owned_count += 1;
    serial_println!("[avatar] item {} unlocked (rarity {:?})", item.item_id,
        item.rarity as u8);
    true
}

/// Equip an owned item into its slot
pub fn equip(item_id: u16) {
    let mut s = STATE.lock();
    for i in 0..s.owned_count {
        if s.inventory[i].owned && s.inventory[i].item_id == item_id {
            let slot_idx = match s.inventory[i].slot {
                EquipSlot::Head       => 0,
                EquipSlot::Body       => 1,
                EquipSlot::Legs       => 2,
                EquipSlot::Accessory1 => 3,
                EquipSlot::Accessory2 => 4,
                EquipSlot::Tool       => 5,
                EquipSlot::Mount      => 6,
                EquipSlot::Aura       => 7,
            };
            s.equipped[slot_idx] = item_id;
            s.identity_locked = true;
            serial_println!("[avatar] equipped item {} in slot {}", item_id, slot_idx);
            return;
        }
    }
}

/// Grant a free starter item at naming ceremony
pub fn grant_starter(style_id: u16, color_id: u16) {
    let item = CosmeticItem {
        item_id: style_id.wrapping_add(1000),
        slot: EquipSlot::Body,
        rarity: ItemRarity::Common,
        style_id, color_id,
        is_free: true,
        owned: true,
    };
    let mut s = STATE.lock();
    if s.owned_count < MAX_OWNED_ITEMS {
        let idx = s.owned_count;
        s.inventory[idx] = item;
        s.owned_count += 1;
        s.equipped[1] = item.item_id; // auto-equip body slot
        serial_println!("[avatar] starter outfit granted (style: {}, color: {})",
            style_id, color_id);
    }
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn aura_strength()   -> u16  { STATE.lock().aura_strength }
pub fn aura_color()      -> u16  { STATE.lock().base_aura_color }
pub fn aura_complexity() -> u16  { STATE.lock().aura_complexity }
pub fn soul_glow()       -> u16  { STATE.lock().soul_glow }
pub fn base_style()      -> u16  { STATE.lock().base_body_style }
pub fn awakening_marks() -> u8   { STATE.lock().awakening_marks }
pub fn style_coherence() -> u16  { STATE.lock().style_coherence }
pub fn credit_tokens()   -> u32  { STATE.lock().credit_tokens }
pub fn owned_count()     -> usize { STATE.lock().owned_count }
