use crate::sync::Mutex;
/// Quick settings tiles for Genesis
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum TileState {
    Active,
    Inactive,
    Unavailable,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TileType {
    Wifi,
    Bluetooth,
    Cellular,
    Airplane,
    Flashlight,
    AutoRotate,
    Battery,
    DarkMode,
    Hotspot,
    Nfc,
    Location,
    DoNotDisturb,
    ScreenCast,
    Vpn,
    NightLight,
}

#[derive(Clone, Copy)]
struct QuickTile {
    id: u32,
    tile_type: TileType,
    state: TileState,
    label_hash: u64,
    position: u8,
    visible: bool,
}

struct TileManager {
    tiles: Vec<QuickTile>,
    max_tiles: u8,
    edit_mode: bool,
    next_id: u32,
}

static TILE_MGR: Mutex<Option<TileManager>> = Mutex::new(None);

impl TileManager {
    fn new() -> Self {
        TileManager {
            tiles: Vec::new(),
            max_tiles: 15,
            edit_mode: false,
            next_id: 1,
        }
    }

    fn add_default_tiles(&mut self) {
        let defaults = [
            TileType::Wifi,
            TileType::Bluetooth,
            TileType::Cellular,
            TileType::Airplane,
            TileType::Flashlight,
            TileType::AutoRotate,
            TileType::DarkMode,
            TileType::DoNotDisturb,
            TileType::Location,
            TileType::Hotspot,
            TileType::Nfc,
            TileType::NightLight,
        ];
        for (i, &tt) in defaults.iter().enumerate() {
            self.tiles.push(QuickTile {
                id: self.next_id,
                tile_type: tt,
                state: TileState::Inactive,
                label_hash: 0,
                position: i as u8,
                visible: true,
            });
            self.next_id = self.next_id.saturating_add(1);
        }
    }

    fn toggle_tile(&mut self, tile_id: u32) {
        if let Some(t) = self.tiles.iter_mut().find(|t| t.id == tile_id) {
            t.state = match t.state {
                TileState::Active => TileState::Inactive,
                TileState::Inactive => TileState::Active,
                TileState::Unavailable => TileState::Unavailable,
            };
        }
    }

    fn get_visible(&self) -> Vec<u32> {
        self.tiles
            .iter()
            .filter(|t| t.visible)
            .map(|t| t.id)
            .collect()
    }

    fn rearrange(&mut self, tile_id: u32, new_pos: u8) {
        if let Some(t) = self.tiles.iter_mut().find(|t| t.id == tile_id) {
            t.position = new_pos;
        }
    }
}

pub fn init() {
    let mut tm = TILE_MGR.lock();
    let mut mgr = TileManager::new();
    mgr.add_default_tiles();
    *tm = Some(mgr);
    serial_println!("    Quick tiles: 12 default tiles ready");
}
