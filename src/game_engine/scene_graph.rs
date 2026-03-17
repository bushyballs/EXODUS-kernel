use crate::sync::Mutex;
/// Scene Graph and Level Management for Genesis
///
/// Manages game scenes (levels) and their entities. Each entity can
/// optionally reference a sprite and/or a physics body, plus carry
/// tag bits and a data hash for game-specific lookup. The SceneManager
/// handles loading, unloading, and switching between scenes.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Maximum number of scenes that can exist simultaneously.
const MAX_SCENES: usize = 32;

/// Maximum entities per scene.
const MAX_ENTITIES_PER_SCENE: usize = 1024;

/// An entity within a scene. Entities are lightweight containers
/// that link together sprite visuals, physics bodies, and game logic.
#[derive(Clone, Copy)]
pub struct Entity {
    pub id: u32,
    pub sprite_id: Option<u32>,
    pub body_id: Option<u32>,
    pub tags: u64,
    pub data_hash: u64,
    pub enabled: bool,
    pub parent_id: Option<u32>,
    pub local_x: i32,
    pub local_y: i32,
    pub depth: u32,
}

/// A scene represents a game level or screen. It contains a collection
/// of entities and metadata for the scene manager.
#[derive(Clone)]
pub struct Scene {
    pub id: u32,
    pub name_hash: u64,
    pub entities: Vec<Entity>,
    pub active: bool,
    pub paused: bool,
    pub next_entity_id: u32,
    pub background_hash: u64,
    pub transition_state: TransitionState,
    pub transition_timer: u32,
    pub transition_duration: u32,
}

/// Scene transition states for smooth level changes.
#[derive(Clone, Copy, PartialEq)]
pub enum TransitionState {
    None,
    FadeIn,
    FadeOut,
    SlideLeft,
    SlideRight,
    Ready,
}

/// Query result for entity searches.
#[derive(Clone, Copy)]
pub struct EntityRef {
    pub scene_id: u32,
    pub entity_id: u32,
    pub entity_index: usize,
}

/// The scene manager owns all scenes and coordinates transitions.
struct SceneManager {
    scenes: Vec<Scene>,
    next_scene_id: u32,
    active_scene_id: Option<u32>,
    pending_load: Option<u32>,
    pending_unload: Option<u32>,
    entity_count_total: usize,
}

static SCENE_MANAGER: Mutex<Option<SceneManager>> = Mutex::new(None);

impl Entity {
    /// Create a new entity with default values.
    fn new(id: u32) -> Self {
        Entity {
            id,
            sprite_id: None,
            body_id: None,
            tags: 0,
            data_hash: 0,
            enabled: true,
            parent_id: None,
            local_x: 0,
            local_y: 0,
            depth: 0,
        }
    }

    /// Check if this entity has a specific tag bit set.
    fn has_tag(&self, tag_bit: u32) -> bool {
        if tag_bit >= 64 {
            return false;
        }
        (self.tags & (1u64 << tag_bit)) != 0
    }

    /// Set a tag bit on this entity.
    fn set_tag(&mut self, tag_bit: u32) {
        if tag_bit < 64 {
            self.tags |= 1u64 << tag_bit;
        }
    }

    /// Clear a tag bit on this entity.
    fn clear_tag(&mut self, tag_bit: u32) {
        if tag_bit < 64 {
            self.tags &= !(1u64 << tag_bit);
        }
    }
}

impl Scene {
    /// Create a new empty scene.
    fn new(id: u32, name_hash: u64) -> Self {
        Scene {
            id,
            name_hash,
            entities: Vec::new(),
            active: false,
            paused: false,
            next_entity_id: 1,
            background_hash: 0,
            transition_state: TransitionState::None,
            transition_timer: 0,
            transition_duration: 60, // 1 second at 60fps default
        }
    }

    /// Add an entity to this scene. Returns the entity id.
    fn add_entity(
        &mut self,
        sprite_id: Option<u32>,
        body_id: Option<u32>,
        tags: u64,
        data_hash: u64,
    ) -> u32 {
        if self.entities.len() >= MAX_ENTITIES_PER_SCENE {
            serial_println!(
                "    Scene {}: max entities reached ({})",
                self.id,
                MAX_ENTITIES_PER_SCENE
            );
            return 0;
        }

        let eid = self.next_entity_id;
        self.next_entity_id = self.next_entity_id.saturating_add(1);

        let mut entity = Entity::new(eid);
        entity.sprite_id = sprite_id;
        entity.body_id = body_id;
        entity.tags = tags;
        entity.data_hash = data_hash;

        self.entities.push(entity);
        eid
    }

    /// Remove an entity by id.
    fn remove_entity(&mut self, entity_id: u32) -> bool {
        if let Some(pos) = self.entities.iter().position(|e| e.id == entity_id) {
            // Also unparent any children referencing this entity
            let removed_id = self.entities[pos].id;
            self.entities.swap_remove(pos);

            for entity in self.entities.iter_mut() {
                if entity.parent_id == Some(removed_id) {
                    entity.parent_id = None;
                }
            }
            return true;
        }
        false
    }

    /// Find all entities that have ALL of the specified tag bits set.
    fn find_by_tag(&self, tag_mask: u64) -> Vec<u32> {
        let mut results = Vec::new();
        for entity in self.entities.iter() {
            if entity.enabled && (entity.tags & tag_mask) == tag_mask {
                results.push(entity.id);
            }
        }
        results
    }

    /// Find all entities that match a specific data hash.
    fn find_by_data_hash(&self, hash: u64) -> Vec<u32> {
        let mut results = Vec::new();
        for entity in self.entities.iter() {
            if entity.enabled && entity.data_hash == hash {
                results.push(entity.id);
            }
        }
        results
    }

    /// Set parent-child relationship between entities.
    fn set_parent(&mut self, child_id: u32, parent_id: Option<u32>) -> bool {
        // Validate parent exists if specified
        if let Some(pid) = parent_id {
            if !self.entities.iter().any(|e| e.id == pid) {
                return false;
            }
        }

        for entity in self.entities.iter_mut() {
            if entity.id == child_id {
                entity.parent_id = parent_id;
                return true;
            }
        }
        false
    }

    /// Update scene transition timer. Returns true if transition is complete.
    fn update_transition(&mut self) -> bool {
        if self.transition_state == TransitionState::None
            || self.transition_state == TransitionState::Ready
        {
            return true;
        }

        self.transition_timer = self.transition_timer.saturating_add(1);
        if self.transition_timer >= self.transition_duration {
            self.transition_state = TransitionState::Ready;
            self.transition_timer = 0;
            return true;
        }
        false
    }

    /// Get the entity count.
    fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Get enabled entity count.
    fn enabled_count(&self) -> usize {
        self.entities.iter().filter(|e| e.enabled).count()
    }

    /// Enable or disable an entity.
    fn set_entity_enabled(&mut self, entity_id: u32, enabled: bool) -> bool {
        for entity in self.entities.iter_mut() {
            if entity.id == entity_id {
                entity.enabled = enabled;
                return true;
            }
        }
        false
    }

    /// Get an entity reference by id.
    fn get_entity(&self, entity_id: u32) -> Option<&Entity> {
        self.entities.iter().find(|e| e.id == entity_id)
    }

    /// Get a mutable entity reference by id.
    fn get_entity_mut(&mut self, entity_id: u32) -> Option<&mut Entity> {
        self.entities.iter_mut().find(|e| e.id == entity_id)
    }
}

impl SceneManager {
    fn new() -> Self {
        SceneManager {
            scenes: Vec::new(),
            next_scene_id: 1,
            active_scene_id: None,
            pending_load: None,
            pending_unload: None,
            entity_count_total: 0,
        }
    }

    /// Create a new scene. Returns the scene id.
    fn create_scene(&mut self, name_hash: u64) -> u32 {
        if self.scenes.len() >= MAX_SCENES {
            serial_println!("    SceneManager: max scenes reached ({})", MAX_SCENES);
            return 0;
        }

        let id = self.next_scene_id;
        self.next_scene_id = self.next_scene_id.saturating_add(1);

        let scene = Scene::new(id, name_hash);
        self.scenes.push(scene);
        id
    }

    /// Load a scene by id, making it the active scene.
    /// Begins a fade-in transition.
    fn load_scene(&mut self, scene_id: u32) -> bool {
        // Deactivate current scene
        if let Some(active_id) = self.active_scene_id {
            for scene in self.scenes.iter_mut() {
                if scene.id == active_id {
                    scene.active = false;
                    scene.transition_state = TransitionState::FadeOut;
                    scene.transition_timer = 0;
                    break;
                }
            }
        }

        // Activate target scene
        for scene in self.scenes.iter_mut() {
            if scene.id == scene_id {
                scene.active = true;
                scene.paused = false;
                scene.transition_state = TransitionState::FadeIn;
                scene.transition_timer = 0;
                self.active_scene_id = Some(scene_id);
                return true;
            }
        }
        false
    }

    /// Unload a scene, removing all its entities and freeing resources.
    fn unload_scene(&mut self, scene_id: u32) -> bool {
        if let Some(pos) = self.scenes.iter().position(|s| s.id == scene_id) {
            let entity_count = self.scenes[pos].entities.len();
            self.entity_count_total = self.entity_count_total.saturating_sub(entity_count);

            if self.active_scene_id == Some(scene_id) {
                self.active_scene_id = None;
            }

            self.scenes.swap_remove(pos);
            return true;
        }
        false
    }

    /// Add an entity to a specific scene.
    fn add_entity(
        &mut self,
        scene_id: u32,
        sprite_id: Option<u32>,
        body_id: Option<u32>,
        tags: u64,
        data_hash: u64,
    ) -> u32 {
        for scene in self.scenes.iter_mut() {
            if scene.id == scene_id {
                let eid = scene.add_entity(sprite_id, body_id, tags, data_hash);
                if eid > 0 {
                    self.entity_count_total = self.entity_count_total.saturating_add(1);
                }
                return eid;
            }
        }
        0
    }

    /// Remove an entity from a specific scene.
    fn remove_entity(&mut self, scene_id: u32, entity_id: u32) -> bool {
        for scene in self.scenes.iter_mut() {
            if scene.id == scene_id {
                if scene.remove_entity(entity_id) {
                    self.entity_count_total = self.entity_count_total.saturating_sub(1);
                    return true;
                }
                return false;
            }
        }
        false
    }

    /// Find entities by tag mask in the active scene.
    fn find_by_tag(&self, tag_mask: u64) -> Vec<u32> {
        if let Some(active_id) = self.active_scene_id {
            for scene in self.scenes.iter() {
                if scene.id == active_id {
                    return scene.find_by_tag(tag_mask);
                }
            }
        }
        Vec::new()
    }

    /// Update the active scene: advance transitions.
    fn update_scene(&mut self) {
        for scene in self.scenes.iter_mut() {
            if scene.active {
                scene.update_transition();
            }
        }
    }

    /// Get the id of the currently active scene.
    fn get_active(&self) -> Option<u32> {
        self.active_scene_id
    }

    /// Get the active scene reference.
    fn get_active_scene(&self) -> Option<&Scene> {
        if let Some(active_id) = self.active_scene_id {
            return self.scenes.iter().find(|s| s.id == active_id);
        }
        None
    }

    /// Pause or unpause the active scene.
    fn set_paused(&mut self, paused: bool) {
        if let Some(active_id) = self.active_scene_id {
            for scene in self.scenes.iter_mut() {
                if scene.id == active_id {
                    scene.paused = paused;
                    return;
                }
            }
        }
    }

    /// Get the total number of scenes.
    fn scene_count(&self) -> usize {
        self.scenes.len()
    }

    /// Get total entity count across all scenes.
    fn total_entity_count(&self) -> usize {
        self.entity_count_total
    }
}

// --- Public API ---

/// Create a new scene with a name hash. Returns the scene id.
pub fn create_scene(name_hash: u64) -> u32 {
    let mut mgr = SCENE_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.create_scene(name_hash)
    } else {
        0
    }
}

/// Load a scene, making it active with a fade-in transition.
pub fn load_scene(scene_id: u32) -> bool {
    let mut mgr = SCENE_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.load_scene(scene_id)
    } else {
        false
    }
}

/// Unload a scene and free its entities.
pub fn unload_scene(scene_id: u32) -> bool {
    let mut mgr = SCENE_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.unload_scene(scene_id)
    } else {
        false
    }
}

/// Add an entity to a scene. Returns the entity id.
pub fn add_entity(
    scene_id: u32,
    sprite_id: Option<u32>,
    body_id: Option<u32>,
    tags: u64,
    data_hash: u64,
) -> u32 {
    let mut mgr = SCENE_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.add_entity(scene_id, sprite_id, body_id, tags, data_hash)
    } else {
        0
    }
}

/// Remove an entity from a scene.
pub fn remove_entity(scene_id: u32, entity_id: u32) -> bool {
    let mut mgr = SCENE_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.remove_entity(scene_id, entity_id)
    } else {
        false
    }
}

/// Find entities by tag mask in the active scene.
pub fn find_by_tag(tag_mask: u64) -> Vec<u32> {
    let mgr = SCENE_MANAGER.lock();
    if let Some(ref m) = *mgr {
        m.find_by_tag(tag_mask)
    } else {
        Vec::new()
    }
}

/// Update the active scene (transitions, timers).
pub fn update_scene() {
    let mut mgr = SCENE_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.update_scene();
    }
}

/// Get the currently active scene id.
pub fn get_active() -> Option<u32> {
    let mgr = SCENE_MANAGER.lock();
    if let Some(ref m) = *mgr {
        m.get_active()
    } else {
        None
    }
}

pub fn init() {
    let mut mgr = SCENE_MANAGER.lock();
    *mgr = Some(SceneManager::new());
    serial_println!(
        "    Scene graph: {} max scenes, {} entities/scene, transitions",
        MAX_SCENES,
        MAX_ENTITIES_PER_SCENE
    );
}
