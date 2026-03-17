/// Animation System for Genesis
///
/// Comprehensive animation framework with sprite sheet playback,
/// skeletal animation with bone hierarchies, tweening with easing
/// functions, and an animation state machine for transitions.
/// All values use i32 Q16 fixed-point (65536 = 1.0).

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constants
const Q16_ONE: i32 = 65536;
const Q16_HALF: i32 = 32768;
const Q16_ZERO: i32 = 0;
const Q16_TWO: i32 = 131072;

/// Maximum sprite sheet animations.
const MAX_SPRITE_ANIMS: usize = 64;

/// Maximum skeletal rigs.
const MAX_SKELETONS: usize = 16;

/// Maximum bones per skeleton.
const MAX_BONES: usize = 32;

/// Maximum active tweens.
const MAX_TWEENS: usize = 128;

/// Maximum animation state machines.
const MAX_STATE_MACHINES: usize = 32;

/// Maximum states per state machine.
const MAX_STATES: usize = 16;

/// Maximum transitions per state machine.
const MAX_TRANSITIONS: usize = 32;

/// Q16 multiply: (a * b) >> 16, using i64 to prevent overflow.
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide: (a << 16) / b, using i64 to prevent overflow.
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << 16) / (b as i64)) as i32
}

/// Easing function type.
#[derive(Clone, Copy, PartialEq)]
pub enum EasingType {
    Linear,
    EaseInQuad,
    EaseOutQuad,
    EaseInOutQuad,
    EaseInCubic,
    EaseOutCubic,
    EaseInOutCubic,
    EaseInBack,
    EaseOutBack,
    EaseOutBounce,
}

/// Apply an easing function. Input t is Q16 [0..Q16_ONE], output is Q16.
fn apply_easing(easing: EasingType, t: i32) -> i32 {
    match easing {
        EasingType::Linear => t,
        EasingType::EaseInQuad => {
            q16_mul(t, t)
        }
        EasingType::EaseOutQuad => {
            let inv = Q16_ONE - t;
            Q16_ONE - q16_mul(inv, inv)
        }
        EasingType::EaseInOutQuad => {
            if t < Q16_HALF {
                // 2 * t * t
                2 * q16_mul(t, t)
            } else {
                let inv = Q16_TWO - 2 * t;
                Q16_ONE - q16_mul(inv, inv) / 2
            }
        }
        EasingType::EaseInCubic => {
            q16_mul(q16_mul(t, t), t)
        }
        EasingType::EaseOutCubic => {
            let inv = Q16_ONE - t;
            Q16_ONE - q16_mul(q16_mul(inv, inv), inv)
        }
        EasingType::EaseInOutCubic => {
            if t < Q16_HALF {
                4 * q16_mul(q16_mul(t, t), t)
            } else {
                let shifted = 2 * t - Q16_TWO;
                Q16_ONE + q16_mul(q16_mul(shifted, shifted), shifted) / 2
            }
        }
        EasingType::EaseInBack => {
            // overshoot constant c1 ~ 1.70158 in Q16 = 111514
            let c1: i32 = 111514;
            let c3 = c1 + Q16_ONE;
            q16_mul(q16_mul(t, t), q16_mul(c3, t) - c1)
        }
        EasingType::EaseOutBack => {
            let c1: i32 = 111514;
            let c3 = c1 + Q16_ONE;
            let inv = t - Q16_ONE;
            Q16_ONE + q16_mul(q16_mul(inv, inv), q16_mul(c3, inv) + c1)
        }
        EasingType::EaseOutBounce => {
            ease_out_bounce(t)
        }
    }
}

/// Bounce easing helper.
fn ease_out_bounce(t: i32) -> i32 {
    // Thresholds in Q16: 1/2.75, 2/2.75, 2.5/2.75
    let n1: i32 = 495616;  // 7.5625 in Q16
    let d1: i32 = 180224;  // 2.75 in Q16
    let t1 = q16_div(Q16_ONE, d1);        // 1/2.75
    let t2 = q16_div(Q16_TWO, d1);        // 2/2.75
    let t25 = q16_div(163840, d1);         // 2.5/2.75 (163840 = 2.5 * 65536)

    if t < t1 {
        q16_mul(n1, q16_mul(t, t))
    } else if t < t2 {
        let adj = t - q16_div(98304, d1);  // t - 1.5/2.75 (98304 = 1.5*65536)
        q16_mul(n1, q16_mul(adj, adj)) + 49152  // + 0.75 in Q16
    } else if t < t25 {
        let adj = t - q16_div(Q16_TWO + Q16_HALF / 2, d1);
        q16_mul(n1, q16_mul(adj, adj)) + 61440   // + 0.9375 in Q16
    } else {
        let adj = t - q16_div(Q16_TWO + Q16_HALF, d1);
        q16_mul(n1, q16_mul(adj, adj)) + 64225   // + 0.984375 in Q16
    }
}

/// A sprite sheet animation clip.
#[derive(Clone, Copy)]
pub struct SpriteAnim {
    pub id: u32,
    pub name_hash: u64,
    pub texture_hash: u64,
    pub start_frame: u32,
    pub end_frame: u32,
    pub frame_width: u32,
    pub frame_height: u32,
    pub columns: u32,
    pub speed: u32,          // ticks between frames
    pub looping: bool,
    pub current_frame: u32,
    pub timer: u32,
    pub playing: bool,
    pub finished: bool,
    pub active: bool,
}

impl SpriteAnim {
    fn empty() -> Self {
        SpriteAnim {
            id: 0, name_hash: 0, texture_hash: 0,
            start_frame: 0, end_frame: 0,
            frame_width: 0, frame_height: 0, columns: 1,
            speed: 6, looping: true,
            current_frame: 0, timer: 0,
            playing: false, finished: false, active: false,
        }
    }

    /// Advance the animation by one tick.
    fn tick(&mut self) {
        if !self.playing || self.finished { return; }
        self.timer = self.timer.saturating_add(1);
        if self.timer >= self.speed {
            self.timer = 0;
            if self.current_frame < self.end_frame {
                self.current_frame = self.current_frame.saturating_add(1);
            } else if self.looping {
                self.current_frame = self.start_frame;
            } else {
                self.finished = true;
            }
        }
    }

    /// Get the source rectangle for the current frame: (x, y, w, h).
    fn frame_rect(&self) -> (u32, u32, u32, u32) {
        if self.columns == 0 { return (0, 0, 0, 0); }
        let col = self.current_frame % self.columns;
        let row = self.current_frame / self.columns;
        (col * self.frame_width, row * self.frame_height,
         self.frame_width, self.frame_height)
    }
}

/// A bone in a skeletal hierarchy.
#[derive(Clone, Copy)]
pub struct Bone {
    pub id: u32,
    pub parent_index: i32,    // -1 = root
    pub local_x: i32,        // Q16 local offset from parent
    pub local_y: i32,
    pub local_rotation: i32, // Q16 radians
    pub local_scale: i32,    // Q16
    pub world_x: i32,        // Q16 computed world position
    pub world_y: i32,
    pub world_rotation: i32,
    pub length: i32,          // Q16 bone length
    pub active: bool,
}

impl Bone {
    fn empty() -> Self {
        Bone {
            id: 0, parent_index: -1,
            local_x: Q16_ZERO, local_y: Q16_ZERO,
            local_rotation: Q16_ZERO, local_scale: Q16_ONE,
            world_x: Q16_ZERO, world_y: Q16_ZERO,
            world_rotation: Q16_ZERO,
            length: Q16_ONE, active: false,
        }
    }
}

/// A skeleton composed of a bone hierarchy.
#[derive(Clone)]
pub struct Skeleton {
    pub id: u32,
    pub bones: Vec<Bone>,
    pub active: bool,
}

impl Skeleton {
    fn new(id: u32) -> Self {
        Skeleton { id, bones: Vec::new(), active: true }
    }

    /// Add a bone. Returns the bone index.
    fn add_bone(&mut self, parent_index: i32, length: i32) -> usize {
        if self.bones.len() >= MAX_BONES { return 0; }
        let mut bone = Bone::empty();
        bone.id = self.bones.len() as u32;
        bone.parent_index = parent_index;
        bone.length = length;
        bone.active = true;
        self.bones.push(bone);
        self.bones.len() - 1
    }

    /// Compute world transforms by walking the hierarchy.
    fn compute_world_transforms(&mut self) {
        for i in 0..self.bones.len() {
            if !self.bones[i].active { continue; }
            let parent_idx = self.bones[i].parent_index;
            if parent_idx < 0 || parent_idx as usize >= self.bones.len() {
                // Root bone
                self.bones[i].world_x = self.bones[i].local_x;
                self.bones[i].world_y = self.bones[i].local_y;
                self.bones[i].world_rotation = self.bones[i].local_rotation;
            } else {
                let pi = parent_idx as usize;
                let parent_wx = self.bones[pi].world_x;
                let parent_wy = self.bones[pi].world_y;
                let parent_rot = self.bones[pi].world_rotation;
                // Simple additive transform (rotation approximation)
                self.bones[i].world_x = parent_wx + self.bones[i].local_x;
                self.bones[i].world_y = parent_wy + self.bones[i].local_y;
                self.bones[i].world_rotation = parent_rot + self.bones[i].local_rotation;
            }
        }
    }

    /// Set bone local transform.
    fn set_bone_local(&mut self, bone_idx: usize, x: i32, y: i32, rotation: i32) {
        if bone_idx < self.bones.len() {
            self.bones[bone_idx].local_x = x;
            self.bones[bone_idx].local_y = y;
            self.bones[bone_idx].local_rotation = rotation;
        }
    }
}

/// A tween interpolates a single value over time.
#[derive(Clone, Copy)]
pub struct Tween {
    pub id: u32,
    pub start_value: i32,    // Q16
    pub end_value: i32,      // Q16
    pub current_value: i32,  // Q16
    pub duration: u32,       // ticks
    pub elapsed: u32,
    pub easing: EasingType,
    pub looping: bool,
    pub ping_pong: bool,
    pub forward: bool,
    pub finished: bool,
    pub active: bool,
    pub target_hash: u64,    // identifies what this tween controls
}

impl Tween {
    fn empty() -> Self {
        Tween {
            id: 0, start_value: Q16_ZERO, end_value: Q16_ONE,
            current_value: Q16_ZERO, duration: 60, elapsed: 0,
            easing: EasingType::Linear, looping: false,
            ping_pong: false, forward: true,
            finished: false, active: false, target_hash: 0,
        }
    }

    /// Advance the tween by one tick.
    fn tick(&mut self) {
        if !self.active || self.finished { return; }
        self.elapsed = self.elapsed.saturating_add(1);
        if self.elapsed >= self.duration {
            if self.ping_pong {
                self.forward = !self.forward;
                self.elapsed = 0;
            } else if self.looping {
                self.elapsed = 0;
            } else {
                self.elapsed = self.duration;
                self.finished = true;
            }
        }
        // Compute normalized time [0..Q16_ONE]
        let t = if self.duration > 0 {
            q16_div(self.elapsed as i32 * Q16_ONE / Q16_ONE, self.duration as i32)
        } else {
            Q16_ONE
        };
        let eased = apply_easing(self.easing, t);
        let progress = if self.forward { eased } else { Q16_ONE - eased };
        // Lerp: start + (end - start) * progress
        let range = self.end_value - self.start_value;
        self.current_value = self.start_value + q16_mul(range, progress);
    }
}

/// Animation state in a state machine.
#[derive(Clone, Copy)]
pub struct AnimState {
    pub name_hash: u64,
    pub anim_id: u32,        // references a SpriteAnim id
    pub speed_scale: i32,    // Q16 playback speed multiplier
    pub active: bool,
}

/// Transition between animation states.
#[derive(Clone, Copy)]
pub struct AnimTransition {
    pub from_hash: u64,
    pub to_hash: u64,
    pub trigger_hash: u64,   // trigger condition identifier
    pub blend_ticks: u32,    // transition blend duration
    pub active: bool,
}

/// Animation state machine controls flow between animation states.
#[derive(Clone)]
pub struct AnimStateMachine {
    pub id: u32,
    pub states: Vec<AnimState>,
    pub transitions: Vec<AnimTransition>,
    pub current_state_hash: u64,
    pub blend_timer: u32,
    pub blend_duration: u32,
    pub blending: bool,
    pub blend_from_hash: u64,
    pub active: bool,
}

impl AnimStateMachine {
    fn new(id: u32) -> Self {
        AnimStateMachine {
            id, states: Vec::new(), transitions: Vec::new(),
            current_state_hash: 0, blend_timer: 0, blend_duration: 0,
            blending: false, blend_from_hash: 0, active: true,
        }
    }

    /// Add a state to the machine.
    fn add_state(&mut self, name_hash: u64, anim_id: u32, speed_scale: i32) -> bool {
        if self.states.len() >= MAX_STATES { return false; }
        self.states.push(AnimState { name_hash, anim_id, speed_scale, active: true });
        if self.current_state_hash == 0 {
            self.current_state_hash = name_hash;
        }
        true
    }

    /// Add a transition rule.
    fn add_transition(&mut self, from: u64, to: u64, trigger: u64, blend: u32) -> bool {
        if self.transitions.len() >= MAX_TRANSITIONS { return false; }
        self.transitions.push(AnimTransition {
            from_hash: from, to_hash: to, trigger_hash: trigger,
            blend_ticks: blend, active: true,
        });
        true
    }

    /// Fire a trigger, potentially causing a state transition.
    fn fire_trigger(&mut self, trigger_hash: u64) -> bool {
        if self.blending { return false; }
        for tr in self.transitions.iter() {
            if !tr.active { continue; }
            if tr.from_hash == self.current_state_hash && tr.trigger_hash == trigger_hash {
                self.blend_from_hash = self.current_state_hash;
                self.current_state_hash = tr.to_hash;
                self.blend_duration = tr.blend_ticks;
                self.blend_timer = 0;
                self.blending = tr.blend_ticks > 0;
                return true;
            }
        }
        false
    }

    /// Update blend timer.
    fn update(&mut self) {
        if self.blending {
            self.blend_timer = self.blend_timer.saturating_add(1);
            if self.blend_timer >= self.blend_duration {
                self.blending = false;
            }
        }
    }

    /// Get blend progress (Q16, 0 = fully from, Q16_ONE = fully to).
    fn blend_progress(&self) -> i32 {
        if !self.blending || self.blend_duration == 0 {
            return Q16_ONE;
        }
        q16_div(self.blend_timer as i32 * Q16_ONE / Q16_ONE, self.blend_duration as i32)
    }
}

/// The animation system manages all animation types.
struct AnimationSystem {
    sprite_anims: Vec<SpriteAnim>,
    skeletons: Vec<Skeleton>,
    tweens: Vec<Tween>,
    state_machines: Vec<AnimStateMachine>,
    next_id: u32,
}

static ANIMATION: Mutex<Option<AnimationSystem>> = Mutex::new(None);

impl AnimationSystem {
    fn new() -> Self {
        AnimationSystem {
            sprite_anims: Vec::new(),
            skeletons: Vec::new(),
            tweens: Vec::new(),
            state_machines: Vec::new(),
            next_id: 1,
        }
    }

    /// Create a sprite animation clip. Returns its id.
    fn create_sprite_anim(&mut self, name_hash: u64, texture_hash: u64,
                          start: u32, end: u32, fw: u32, fh: u32,
                          columns: u32, speed: u32, looping: bool) -> u32 {
        if self.sprite_anims.len() >= MAX_SPRITE_ANIMS { return 0; }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut anim = SpriteAnim::empty();
        anim.id = id;
        anim.name_hash = name_hash;
        anim.texture_hash = texture_hash;
        anim.start_frame = start;
        anim.end_frame = end;
        anim.frame_width = fw;
        anim.frame_height = fh;
        anim.columns = if columns == 0 { 1 } else { columns };
        anim.speed = if speed == 0 { 1 } else { speed };
        anim.looping = looping;
        anim.current_frame = start;
        anim.playing = true;
        anim.active = true;
        self.sprite_anims.push(anim);
        id
    }

    /// Create a skeleton. Returns its id.
    fn create_skeleton(&mut self) -> u32 {
        if self.skeletons.len() >= MAX_SKELETONS { return 0; }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.skeletons.push(Skeleton::new(id));
        id
    }

    /// Add a bone to a skeleton.
    fn add_bone(&mut self, skeleton_id: u32, parent_index: i32, length: i32) -> usize {
        for skel in self.skeletons.iter_mut() {
            if skel.id == skeleton_id && skel.active {
                return skel.add_bone(parent_index, length);
            }
        }
        0
    }

    /// Create a tween. Returns its id.
    fn create_tween(&mut self, start: i32, end: i32, duration: u32,
                    easing: EasingType, looping: bool, ping_pong: bool,
                    target_hash: u64) -> u32 {
        if self.tweens.len() >= MAX_TWEENS { return 0; }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut tw = Tween::empty();
        tw.id = id;
        tw.start_value = start;
        tw.end_value = end;
        tw.current_value = start;
        tw.duration = if duration == 0 { 1 } else { duration };
        tw.easing = easing;
        tw.looping = looping;
        tw.ping_pong = ping_pong;
        tw.target_hash = target_hash;
        tw.active = true;
        self.tweens.push(tw);
        id
    }

    /// Get current tween value.
    fn get_tween_value(&self, tween_id: u32) -> i32 {
        for tw in self.tweens.iter() {
            if tw.id == tween_id && tw.active {
                return tw.current_value;
            }
        }
        Q16_ZERO
    }

    /// Create an animation state machine. Returns its id.
    fn create_state_machine(&mut self) -> u32 {
        if self.state_machines.len() >= MAX_STATE_MACHINES { return 0; }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.state_machines.push(AnimStateMachine::new(id));
        id
    }

    /// Fire a trigger on a state machine.
    fn fire_trigger(&mut self, machine_id: u32, trigger_hash: u64) -> bool {
        for sm in self.state_machines.iter_mut() {
            if sm.id == machine_id && sm.active {
                return sm.fire_trigger(trigger_hash);
            }
        }
        false
    }

    /// Per-frame update: advance all animations.
    fn update(&mut self) {
        for anim in self.sprite_anims.iter_mut() {
            if anim.active { anim.tick(); }
        }
        for tw in self.tweens.iter_mut() {
            if tw.active { tw.tick(); }
        }
        for skel in self.skeletons.iter_mut() {
            if skel.active { skel.compute_world_transforms(); }
        }
        for sm in self.state_machines.iter_mut() {
            if sm.active { sm.update(); }
        }
    }

    /// Remove finished, non-looping tweens.
    fn cleanup_tweens(&mut self) {
        self.tweens.retain(|tw| !tw.finished || tw.looping || tw.ping_pong);
    }
}

// --- Public API ---

/// Create a sprite animation clip.
pub fn create_sprite_anim(name_hash: u64, texture_hash: u64,
                          start: u32, end: u32, fw: u32, fh: u32,
                          columns: u32, speed: u32, looping: bool) -> u32 {
    let mut sys = ANIMATION.lock();
    if let Some(ref mut s) = *sys {
        s.create_sprite_anim(name_hash, texture_hash, start, end, fw, fh, columns, speed, looping)
    } else { 0 }
}

/// Create a tween.
pub fn create_tween(start: i32, end: i32, duration: u32,
                    easing: EasingType, looping: bool, ping_pong: bool,
                    target_hash: u64) -> u32 {
    let mut sys = ANIMATION.lock();
    if let Some(ref mut s) = *sys {
        s.create_tween(start, end, duration, easing, looping, ping_pong, target_hash)
    } else { 0 }
}

/// Get current value of a tween (Q16).
pub fn get_tween_value(tween_id: u32) -> i32 {
    let sys = ANIMATION.lock();
    if let Some(ref s) = *sys {
        s.get_tween_value(tween_id)
    } else { Q16_ZERO }
}

/// Create a skeleton. Returns id.
pub fn create_skeleton() -> u32 {
    let mut sys = ANIMATION.lock();
    if let Some(ref mut s) = *sys {
        s.create_skeleton()
    } else { 0 }
}

/// Create an animation state machine. Returns id.
pub fn create_state_machine() -> u32 {
    let mut sys = ANIMATION.lock();
    if let Some(ref mut s) = *sys {
        s.create_state_machine()
    } else { 0 }
}

/// Fire a trigger on a state machine.
pub fn fire_trigger(machine_id: u32, trigger_hash: u64) -> bool {
    let mut sys = ANIMATION.lock();
    if let Some(ref mut s) = *sys {
        s.fire_trigger(machine_id, trigger_hash)
    } else { false }
}

/// Update all animations once per frame.
pub fn update() {
    let mut sys = ANIMATION.lock();
    if let Some(ref mut s) = *sys {
        s.update();
    }
}

pub fn init() {
    let mut sys = ANIMATION.lock();
    *sys = Some(AnimationSystem::new());
    serial_println!("    Animation: sprite sheets, skeletal ({}bones), tweens (10 easings), state machines",
        MAX_BONES);
}
