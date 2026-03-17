pub mod navigation;
pub mod offline_maps;
pub mod route_engine;
/// Hoags Maps — maps and navigation subsystem for Genesis
///
/// Provides a complete offline-first mapping, routing, and turn-by-turn
/// navigation stack built from scratch. No external crates.
///
/// Subsystems:
///   - tile_render: Slippy map tile rendering with LRU cache
///   - route_engine: Dijkstra + A* pathfinding on road graphs
///   - navigation: Turn-by-turn navigation with off-route detection
///   - offline_maps: Offline map region storage and management
///
/// All coordinates use Q16 fixed-point (i32 * 65536) — no floating point.
/// Tile coordinates follow the standard z/x/y slippy map convention.
pub mod tile_render;

use crate::{serial_print, serial_println};

pub fn init() {
    serial_println!("[MAPS] Initializing maps subsystem...");

    tile_render::init();
    route_engine::init();
    navigation::init();
    offline_maps::init();

    serial_println!("[MAPS] Maps subsystem initialized");
}
