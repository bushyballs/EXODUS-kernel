pub mod game_input;
pub mod physics;
pub mod scene_graph;
/// 2D Game Engine subsystem for Genesis
///
/// Provides a complete game development framework:
///   - sprite: 2D sprite rendering with animation and layering
///   - physics: Rigid body physics with AABB and circle colliders
///   - scene_graph: Scene/level management with entity system
///   - game_input: Action-based input mapping and binding
pub mod sprite;

use crate::{serial_print, serial_println};

pub fn init() {
    sprite::init();
    physics::init();
    scene_graph::init();
    game_input::init();
    serial_println!("  Game engine initialized (sprite, physics, scene_graph, game_input)");
}
