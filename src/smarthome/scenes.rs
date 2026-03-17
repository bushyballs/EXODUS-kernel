use crate::sync::Mutex;
/// Home scenes for Genesis
///
/// Multi-device scene presets, quick actions.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

struct SceneAction {
    device_id: u32,
    power: Option<bool>,
    brightness: Option<u8>,
    temperature: Option<u16>,
}

struct Scene {
    id: u32,
    name: [u8; 24],
    name_len: usize,
    actions: Vec<SceneAction>,
    icon: u8,
    activations: u32,
}

struct SceneManager {
    scenes: Vec<Scene>,
    next_id: u32,
}

static SCENES: Mutex<Option<SceneManager>> = Mutex::new(None);

impl SceneManager {
    fn new() -> Self {
        SceneManager {
            scenes: Vec::new(),
            next_id: 1,
        }
    }

    fn create_scene(&mut self, name: &[u8]) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut n = [0u8; 24];
        let nlen = name.len().min(24);
        n[..nlen].copy_from_slice(&name[..nlen]);
        self.scenes.push(Scene {
            id,
            name: n,
            name_len: nlen,
            actions: Vec::new(),
            icon: 0,
            activations: 0,
        });
        id
    }

    fn add_action_to_scene(
        &mut self,
        scene_id: u32,
        device_id: u32,
        power: Option<bool>,
        brightness: Option<u8>,
        temp: Option<u16>,
    ) {
        if let Some(scene) = self.scenes.iter_mut().find(|s| s.id == scene_id) {
            scene.actions.push(SceneAction {
                device_id,
                power,
                brightness,
                temperature: temp,
            });
        }
    }

    fn activate_scene(&mut self, scene_id: u32) -> Vec<SceneAction> {
        if let Some(scene) = self.scenes.iter_mut().find(|s| s.id == scene_id) {
            scene.activations = scene.activations.saturating_add(1);
            return scene
                .actions
                .iter()
                .map(|a| SceneAction {
                    device_id: a.device_id,
                    power: a.power,
                    brightness: a.brightness,
                    temperature: a.temperature,
                })
                .collect();
        }
        Vec::new()
    }
}

pub fn init() {
    let mut s = SCENES.lock();
    *s = Some(SceneManager::new());
    serial_println!("    Smart home: scenes ready");
}
