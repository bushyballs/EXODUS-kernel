use crate::sync::Mutex;
/// Icon pack system for Genesis
///
/// Adaptive icons, custom shapes, icon packs.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum IconShape {
    Circle,
    Squircle,
    RoundedSquare,
    Square,
    Teardrop,
}

#[derive(Clone, Copy)]
struct IconPack {
    id: u32,
    name_hash: u64,
    icon_count: u32,
    version: u16,
    shape: IconShape,
    adaptive: bool,
}

struct IconManager {
    packs: Vec<IconPack>,
    active_pack_id: u32,
    default_shape: IconShape,
    next_id: u32,
}

static ICON_MGR: Mutex<Option<IconManager>> = Mutex::new(None);

impl IconManager {
    fn new() -> Self {
        IconManager {
            packs: Vec::new(),
            active_pack_id: 0,
            default_shape: IconShape::Squircle,
            next_id: 1,
        }
    }

    fn install_pack(&mut self, name_hash: u64, icon_count: u32, shape: IconShape) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.packs.push(IconPack {
            id,
            name_hash,
            icon_count,
            version: 1,
            shape,
            adaptive: true,
        });
        id
    }

    fn set_active(&mut self, pack_id: u32) -> bool {
        if self.packs.iter().any(|p| p.id == pack_id) {
            self.active_pack_id = pack_id;
            true
        } else {
            false
        }
    }

    fn set_shape(&mut self, shape: IconShape) {
        self.default_shape = shape;
    }

    fn uninstall_pack(&mut self, pack_id: u32) -> bool {
        if self.active_pack_id == pack_id {
            self.active_pack_id = 0;
        }
        let before = self.packs.len();
        self.packs.retain(|p| p.id != pack_id);
        self.packs.len() < before
    }
}

pub fn init() {
    let mut im = ICON_MGR.lock();
    *im = Some(IconManager::new());
    serial_println!("    Icon pack manager ready");
}
