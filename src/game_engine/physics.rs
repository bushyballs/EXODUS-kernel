use crate::sync::Mutex;
/// 2D Physics Engine for Genesis
///
/// Rigid body dynamics with AABB and circle colliders, gravity,
/// force/impulse application, collision detection and resolution,
/// and raycasting. All values use i32 Q16 fixed-point arithmetic
/// (16 fractional bits, 65536 = 1.0).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constants
const Q16_ONE: i32 = 65536; // 1.0
const Q16_HALF: i32 = 32768; // 0.5
const Q16_ZERO: i32 = 0; // 0.0

/// Default gravity: ~9.8 m/s^2 in Q16 (9 * 65536 + 52429 ~ 9.8)
const DEFAULT_GRAVITY_Y: i32 = 642252;

/// Maximum number of rigid bodies
const MAX_BODIES: usize = 256;

/// Maximum velocity magnitude (prevents tunneling)
const MAX_VELOCITY: i32 = 655360; // 10.0 in Q16

/// Integer square root using Newton's method. Works on non-negative i32.
fn isqrt(value: i32) -> i32 {
    if value <= 0 {
        return 0;
    }
    let val = value as u32;
    let mut guess = val;
    let mut last = 0u32;

    loop {
        last = guess;
        // Newton step: guess = (guess + val / guess) / 2
        if guess == 0 {
            return 0;
        }
        guess = (guess + val / guess) / 2;
        if guess >= last {
            break;
        }
    }
    last as i32
}

/// Q16 multiply: (a * b) >> 16, using i64 to prevent overflow.
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide: (a << 16) / b, using i64 to prevent overflow.
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

/// Absolute value helper
fn q16_abs(v: i32) -> i32 {
    if v < 0 {
        -v
    } else {
        v
    }
}

/// Clamp a value to [-limit, limit]
fn q16_clamp(v: i32, limit: i32) -> i32 {
    if v > limit {
        limit
    } else if v < -limit {
        -limit
    } else {
        v
    }
}

/// Collider shape for a rigid body.
#[derive(Clone, Copy, PartialEq)]
pub enum Collider {
    /// Axis-aligned bounding box with Q16 half-width and half-height.
    AABB { w: i32, h: i32 },
    /// Circle with Q16 radius.
    Circle { radius: i32 },
    /// No collider attached.
    None,
}

/// A 2D rigid body with position, velocity, mass, and physical properties.
#[derive(Clone, Copy)]
pub struct RigidBody {
    pub id: u32,
    pub x: i32,           // Q16 position X
    pub y: i32,           // Q16 position Y
    pub vx: i32,          // Q16 velocity X
    pub vy: i32,          // Q16 velocity Y
    pub ax: i32,          // Q16 accumulated force X (reset each step)
    pub ay: i32,          // Q16 accumulated force Y (reset each step)
    pub mass: i32,        // Q16 mass (0 = infinite / static)
    pub inv_mass: i32,    // Q16 inverse mass (precomputed)
    pub restitution: i32, // Q16 bounciness [0..Q16_ONE]
    pub friction: i32,    // Q16 friction coefficient [0..Q16_ONE]
    pub is_static: bool,  // Static bodies do not move
    pub collider: Collider,
    pub active: bool,
}

/// Result of a raycast query.
#[derive(Clone, Copy)]
pub struct RaycastHit {
    pub body_id: u32,
    pub hit_x: i32,    // Q16 intersection point X
    pub hit_y: i32,    // Q16 intersection point Y
    pub distance: i32, // Q16 distance from ray origin
}

/// Collision pair detected during broadphase/narrowphase.
#[derive(Clone, Copy)]
struct CollisionPair {
    index_a: usize,
    index_b: usize,
    normal_x: i32,    // Q16 collision normal X
    normal_y: i32,    // Q16 collision normal Y
    penetration: i32, // Q16 penetration depth
}

/// The physics world manages all rigid bodies and simulation.
struct PhysicsWorld {
    bodies: Vec<RigidBody>,
    next_id: u32,
    gravity_x: i32, // Q16 gravity vector X
    gravity_y: i32, // Q16 gravity vector Y
    collision_pairs: Vec<CollisionPair>,
}

static PHYSICS: Mutex<Option<PhysicsWorld>> = Mutex::new(None);

impl RigidBody {
    fn new(id: u32, x: i32, y: i32, mass: i32, collider: Collider, is_static: bool) -> Self {
        let inv_mass = if is_static || mass <= 0 {
            Q16_ZERO
        } else {
            q16_div(Q16_ONE, mass)
        };

        RigidBody {
            id,
            x,
            y,
            vx: Q16_ZERO,
            vy: Q16_ZERO,
            ax: Q16_ZERO,
            ay: Q16_ZERO,
            mass,
            inv_mass,
            restitution: Q16_HALF, // 0.5 bounciness default
            friction: 19661,       // ~0.3 friction default
            is_static,
            collider,
            active: true,
        }
    }
}

impl PhysicsWorld {
    fn new() -> Self {
        PhysicsWorld {
            bodies: Vec::new(),
            next_id: 1,
            gravity_x: Q16_ZERO,
            gravity_y: DEFAULT_GRAVITY_Y,
            collision_pairs: Vec::new(),
        }
    }

    /// Create a new rigid body and return its id.
    fn create_body(
        &mut self,
        x: i32,
        y: i32,
        mass: i32,
        collider: Collider,
        is_static: bool,
    ) -> u32 {
        if self.bodies.len() >= MAX_BODIES {
            serial_println!("    Physics: max bodies reached ({})", MAX_BODIES);
            return 0;
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let body = RigidBody::new(id, x, y, mass, collider, is_static);
        self.bodies.push(body);
        id
    }

    /// Remove a rigid body by id.
    fn destroy_body(&mut self, id: u32) -> bool {
        if let Some(pos) = self.bodies.iter().position(|b| b.id == id) {
            self.bodies.swap_remove(pos);
            return true;
        }
        false
    }

    /// Set global gravity vector (Q16).
    fn set_gravity(&mut self, gx: i32, gy: i32) {
        self.gravity_x = gx;
        self.gravity_y = gy;
    }

    /// Apply a force to a body (accumulated over the frame, then reset).
    fn apply_force(&mut self, id: u32, fx: i32, fy: i32) -> bool {
        for body in self.bodies.iter_mut() {
            if body.id == id && body.active && !body.is_static {
                body.ax += fx;
                body.ay += fy;
                return true;
            }
        }
        false
    }

    /// Apply an instantaneous impulse to a body's velocity.
    fn apply_impulse(&mut self, id: u32, ix: i32, iy: i32) -> bool {
        for body in self.bodies.iter_mut() {
            if body.id == id && body.active && !body.is_static {
                body.vx += q16_mul(ix, body.inv_mass);
                body.vy += q16_mul(iy, body.inv_mass);
                return true;
            }
        }
        false
    }

    /// Set physical properties on a body.
    fn set_properties(&mut self, id: u32, restitution: i32, friction: i32) -> bool {
        for body in self.bodies.iter_mut() {
            if body.id == id && body.active {
                body.restitution = restitution;
                body.friction = friction;
                return true;
            }
        }
        false
    }

    /// Check AABB vs AABB collision. Returns (colliding, normal_x, normal_y, penetration).
    fn check_collision_aabb(
        ax: i32,
        ay: i32,
        aw: i32,
        ah: i32,
        bx: i32,
        by: i32,
        bw: i32,
        bh: i32,
    ) -> (bool, i32, i32, i32) {
        // Half-widths are already stored as w, h in the AABB collider
        let dx = bx - ax;
        let dy = by - ay;

        let overlap_x = aw + bw - q16_abs(dx);
        let overlap_y = ah + bh - q16_abs(dy);

        if overlap_x <= 0 || overlap_y <= 0 {
            return (false, 0, 0, 0);
        }

        // Pick the axis with the smallest overlap (least penetration)
        if overlap_x < overlap_y {
            let nx = if dx < 0 { -Q16_ONE } else { Q16_ONE };
            (true, nx, 0, overlap_x)
        } else {
            let ny = if dy < 0 { -Q16_ONE } else { Q16_ONE };
            (true, 0, ny, overlap_y)
        }
    }

    /// Check circle vs circle collision. Returns (colliding, normal_x, normal_y, penetration).
    fn check_collision_circle(
        ax: i32,
        ay: i32,
        ar: i32,
        bx: i32,
        by: i32,
        br: i32,
    ) -> (bool, i32, i32, i32) {
        let dx = bx - ax;
        let dy = by - ay;

        // Distance squared in Q16: need (dx*dx + dy*dy) but these are Q16 values
        // so dx*dx is Q32 — shift right 16 to get Q16
        let dist_sq = q16_mul(dx, dx) + q16_mul(dy, dy);
        let min_dist = ar + br;
        let min_dist_sq = q16_mul(min_dist, min_dist);

        if dist_sq >= min_dist_sq {
            return (false, 0, 0, 0);
        }

        // Compute actual distance using integer sqrt
        // dist_sq is in Q16, so sqrt(dist_sq << 16) gives Q16 distance
        let _dist_raw = isqrt(dist_sq);
        // dist_raw is sqrt of Q16 value, which is sqrt(val * 65536) / 256 scale
        // Actually: dist_sq is already Q16, so we need sqrt in Q16 domain
        // sqrt_q16(x) = isqrt(x << 16)
        let dist_shifted = if dist_sq < 0x7FFF {
            // prevent overflow
            isqrt(dist_sq << 16)
        } else {
            // For large values, use: isqrt(dist_sq) << 8 (approximate)
            isqrt(dist_sq) << 8
        };

        if dist_shifted == 0 {
            // Bodies at same position, push apart on X
            return (true, Q16_ONE, 0, min_dist);
        }

        let nx = q16_div(dx, dist_shifted);
        let ny = q16_div(dy, dist_shifted);
        let penetration = min_dist - dist_shifted;

        (true, nx, ny, penetration)
    }

    /// Check AABB vs Circle collision. Returns (colliding, normal_x, normal_y, penetration).
    fn check_collision_aabb_circle(
        box_x: i32,
        box_y: i32,
        box_w: i32,
        box_h: i32,
        circle_x: i32,
        circle_y: i32,
        circle_r: i32,
    ) -> (bool, i32, i32, i32) {
        // Find closest point on AABB to circle center
        let left = box_x - box_w;
        let right = box_x + box_w;
        let top = box_y - box_h;
        let bottom = box_y + box_h;

        let closest_x = if circle_x < left {
            left
        } else if circle_x > right {
            right
        } else {
            circle_x
        };
        let closest_y = if circle_y < top {
            top
        } else if circle_y > bottom {
            bottom
        } else {
            circle_y
        };

        let dx = circle_x - closest_x;
        let dy = circle_y - closest_y;
        let dist_sq = q16_mul(dx, dx) + q16_mul(dy, dy);
        let r_sq = q16_mul(circle_r, circle_r);

        if dist_sq >= r_sq {
            return (false, 0, 0, 0);
        }

        let dist = if dist_sq < 0x7FFF {
            isqrt(dist_sq << 16)
        } else {
            isqrt(dist_sq) << 8
        };

        if dist == 0 {
            // Circle center is inside AABB
            let overlap_x_left = circle_x - left + circle_r;
            let overlap_x_right = right - circle_x + circle_r;
            let overlap_y_top = circle_y - top + circle_r;
            let overlap_y_bottom = bottom - circle_y + circle_r;

            let min_overlap = overlap_x_left
                .min(overlap_x_right)
                .min(overlap_y_top)
                .min(overlap_y_bottom);

            if min_overlap == overlap_x_left {
                return (true, -Q16_ONE, 0, min_overlap);
            } else if min_overlap == overlap_x_right {
                return (true, Q16_ONE, 0, min_overlap);
            } else if min_overlap == overlap_y_top {
                return (true, 0, -Q16_ONE, min_overlap);
            } else {
                return (true, 0, Q16_ONE, min_overlap);
            }
        }

        let nx = q16_div(dx, dist);
        let ny = q16_div(dy, dist);
        let penetration = circle_r - dist;

        (true, nx, ny, penetration)
    }

    /// Detect all collisions between active bodies.
    fn detect_collisions(&mut self) {
        self.collision_pairs.clear();
        let count = self.bodies.len();

        for i in 0..count {
            if !self.bodies[i].active {
                continue;
            }
            for j in (i + 1)..count {
                if !self.bodies[j].active {
                    continue;
                }
                // Skip static-static pairs
                if self.bodies[i].is_static && self.bodies[j].is_static {
                    continue;
                }

                let (colliding, nx, ny, pen) =
                    match (self.bodies[i].collider, self.bodies[j].collider) {
                        (Collider::AABB { w: aw, h: ah }, Collider::AABB { w: bw, h: bh }) => {
                            Self::check_collision_aabb(
                                self.bodies[i].x,
                                self.bodies[i].y,
                                aw,
                                ah,
                                self.bodies[j].x,
                                self.bodies[j].y,
                                bw,
                                bh,
                            )
                        }
                        (Collider::Circle { radius: ar }, Collider::Circle { radius: br }) => {
                            Self::check_collision_circle(
                                self.bodies[i].x,
                                self.bodies[i].y,
                                ar,
                                self.bodies[j].x,
                                self.bodies[j].y,
                                br,
                            )
                        }
                        (Collider::AABB { w, h }, Collider::Circle { radius }) => {
                            Self::check_collision_aabb_circle(
                                self.bodies[i].x,
                                self.bodies[i].y,
                                w,
                                h,
                                self.bodies[j].x,
                                self.bodies[j].y,
                                radius,
                            )
                        }
                        (Collider::Circle { radius }, Collider::AABB { w, h }) => {
                            let (c, nx, ny, p) = Self::check_collision_aabb_circle(
                                self.bodies[j].x,
                                self.bodies[j].y,
                                w,
                                h,
                                self.bodies[i].x,
                                self.bodies[i].y,
                                radius,
                            );
                            // Flip normal since we swapped order
                            (c, -nx, -ny, p)
                        }
                        _ => (false, 0, 0, 0),
                    };

                if colliding {
                    self.collision_pairs.push(CollisionPair {
                        index_a: i,
                        index_b: j,
                        normal_x: nx,
                        normal_y: ny,
                        penetration: pen,
                    });
                }
            }
        }
    }

    /// Resolve all detected collisions using impulse-based resolution.
    fn resolve_collisions(&mut self) {
        // Work on a copy of pairs since we need mutable access to bodies
        let pairs: Vec<CollisionPair> = self.collision_pairs.clone();

        for pair in pairs.iter() {
            let a_inv = self.bodies[pair.index_a].inv_mass;
            let b_inv = self.bodies[pair.index_b].inv_mass;
            let total_inv = a_inv + b_inv;

            if total_inv == 0 {
                continue;
            }

            // Positional correction (push apart to resolve penetration)
            let correction_scale = q16_mul(pair.penetration, 52429); // 0.8 slop correction
            let corr_x = q16_mul(q16_div(correction_scale, total_inv), pair.normal_x);
            let corr_y = q16_mul(q16_div(correction_scale, total_inv), pair.normal_y);

            if !self.bodies[pair.index_a].is_static {
                self.bodies[pair.index_a].x -= q16_mul(corr_x, a_inv);
                self.bodies[pair.index_a].y -= q16_mul(corr_y, a_inv);
            }
            if !self.bodies[pair.index_b].is_static {
                self.bodies[pair.index_b].x += q16_mul(corr_x, b_inv);
                self.bodies[pair.index_b].y += q16_mul(corr_y, b_inv);
            }

            // Relative velocity
            let rel_vx = self.bodies[pair.index_b].vx - self.bodies[pair.index_a].vx;
            let rel_vy = self.bodies[pair.index_b].vy - self.bodies[pair.index_a].vy;

            // Relative velocity along collision normal
            let vel_along_normal = q16_mul(rel_vx, pair.normal_x) + q16_mul(rel_vy, pair.normal_y);

            // Do not resolve if velocities are separating
            if vel_along_normal > 0 {
                continue;
            }

            // Restitution: use the minimum of both bodies
            let e = self.bodies[pair.index_a]
                .restitution
                .min(self.bodies[pair.index_b].restitution);

            // Impulse magnitude: j = -(1 + e) * vel_along_normal / (inv_mass_a + inv_mass_b)
            let j_num = -q16_mul(Q16_ONE + e, vel_along_normal);
            let j = q16_div(j_num, total_inv);

            // Apply impulse
            let impulse_x = q16_mul(j, pair.normal_x);
            let impulse_y = q16_mul(j, pair.normal_y);

            if !self.bodies[pair.index_a].is_static {
                self.bodies[pair.index_a].vx -= q16_mul(impulse_x, a_inv);
                self.bodies[pair.index_a].vy -= q16_mul(impulse_y, a_inv);
            }
            if !self.bodies[pair.index_b].is_static {
                self.bodies[pair.index_b].vx += q16_mul(impulse_x, b_inv);
                self.bodies[pair.index_b].vy += q16_mul(impulse_y, b_inv);
            }

            // Friction impulse along tangent
            let tangent_x = rel_vx - q16_mul(vel_along_normal, pair.normal_x);
            let tangent_y = rel_vy - q16_mul(vel_along_normal, pair.normal_y);
            let tang_len_sq = q16_mul(tangent_x, tangent_x) + q16_mul(tangent_y, tangent_y);

            if tang_len_sq > 256 {
                // small threshold to avoid division by near-zero
                let tang_len = if tang_len_sq < 0x7FFF {
                    isqrt(tang_len_sq << 16)
                } else {
                    isqrt(tang_len_sq) << 8
                };
                if tang_len > 0 {
                    let tnx = q16_div(tangent_x, tang_len);
                    let tny = q16_div(tangent_y, tang_len);

                    let mu = self.bodies[pair.index_a]
                        .friction
                        .min(self.bodies[pair.index_b].friction);
                    let friction_j = q16_mul(j, mu);

                    if !self.bodies[pair.index_a].is_static {
                        self.bodies[pair.index_a].vx += q16_mul(q16_mul(friction_j, tnx), a_inv);
                        self.bodies[pair.index_a].vy += q16_mul(q16_mul(friction_j, tny), a_inv);
                    }
                    if !self.bodies[pair.index_b].is_static {
                        self.bodies[pair.index_b].vx -= q16_mul(q16_mul(friction_j, tnx), b_inv);
                        self.bodies[pair.index_b].vy -= q16_mul(q16_mul(friction_j, tny), b_inv);
                    }
                }
            }
        }
    }

    /// Step the physics simulation forward by one fixed timestep.
    /// dt is in Q16 (e.g., 1092 = ~1/60s).
    fn step(&mut self, dt: i32) {
        // Integrate forces and velocities using semi-implicit Euler
        for body in self.bodies.iter_mut() {
            if !body.active || body.is_static {
                continue;
            }

            // Apply gravity
            body.ax += q16_mul(self.gravity_x, body.mass);
            body.ay += q16_mul(self.gravity_y, body.mass);

            // Acceleration from force: a = F * inv_mass
            let accel_x = q16_mul(body.ax, body.inv_mass);
            let accel_y = q16_mul(body.ay, body.inv_mass);

            // Update velocity: v += a * dt
            body.vx += q16_mul(accel_x, dt);
            body.vy += q16_mul(accel_y, dt);

            // Clamp velocity to prevent tunneling
            body.vx = q16_clamp(body.vx, MAX_VELOCITY);
            body.vy = q16_clamp(body.vy, MAX_VELOCITY);

            // Update position: x += v * dt
            body.x += q16_mul(body.vx, dt);
            body.y += q16_mul(body.vy, dt);

            // Reset accumulated forces for next frame
            body.ax = Q16_ZERO;
            body.ay = Q16_ZERO;
        }

        // Collision detection and resolution
        self.detect_collisions();
        self.resolve_collisions();
    }

    /// Cast a ray from (ox, oy) in direction (dx, dy) and find the nearest hit.
    /// Direction should be normalized (unit length in Q16). max_dist is Q16.
    fn raycast(&self, ox: i32, oy: i32, dx: i32, dy: i32, max_dist: i32) -> Option<RaycastHit> {
        let mut closest: Option<RaycastHit> = None;
        let mut closest_dist = max_dist;

        for body in self.bodies.iter() {
            if !body.active {
                continue;
            }

            let hit = match body.collider {
                Collider::AABB { w, h } => {
                    // Ray vs AABB using slab method
                    self.ray_vs_aabb(ox, oy, dx, dy, body.x, body.y, w, h, closest_dist)
                }
                Collider::Circle { radius } => {
                    // Ray vs Circle
                    self.ray_vs_circle(ox, oy, dx, dy, body.x, body.y, radius, closest_dist)
                }
                Collider::None => None,
            };

            if let Some((hx, hy, dist)) = hit {
                if dist < closest_dist {
                    closest_dist = dist;
                    closest = Some(RaycastHit {
                        body_id: body.id,
                        hit_x: hx,
                        hit_y: hy,
                        distance: dist,
                    });
                }
            }
        }

        closest
    }

    /// Ray vs AABB intersection. Returns Some((hit_x, hit_y, distance)) or None.
    fn ray_vs_aabb(
        &self,
        ox: i32,
        oy: i32,
        dx: i32,
        dy: i32,
        bx: i32,
        by: i32,
        bw: i32,
        bh: i32,
        max_dist: i32,
    ) -> Option<(i32, i32, i32)> {
        let left = bx - bw;
        let right = bx + bw;
        let top = by - bh;
        let bottom = by + bh;

        // For each axis, compute entry and exit distances
        let (t_near_x, t_far_x) = if dx == 0 {
            if ox < left || ox > right {
                return None;
            }
            (-max_dist, max_dist)
        } else {
            let t1 = q16_div(left - ox, dx);
            let t2 = q16_div(right - ox, dx);
            if t1 < t2 {
                (t1, t2)
            } else {
                (t2, t1)
            }
        };

        let (t_near_y, t_far_y) = if dy == 0 {
            if oy < top || oy > bottom {
                return None;
            }
            (-max_dist, max_dist)
        } else {
            let t1 = q16_div(top - oy, dy);
            let t2 = q16_div(bottom - oy, dy);
            if t1 < t2 {
                (t1, t2)
            } else {
                (t2, t1)
            }
        };

        let t_near = t_near_x.max(t_near_y);
        let t_far = t_far_x.min(t_far_y);

        if t_near > t_far || t_far < 0 || t_near > max_dist {
            return None;
        }

        let t = if t_near > 0 { t_near } else { t_far };
        if t < 0 || t > max_dist {
            return None;
        }

        let hx = ox + q16_mul(dx, t);
        let hy = oy + q16_mul(dy, t);
        Some((hx, hy, t))
    }

    /// Ray vs Circle intersection. Returns Some((hit_x, hit_y, distance)) or None.
    fn ray_vs_circle(
        &self,
        ox: i32,
        oy: i32,
        dx: i32,
        dy: i32,
        cx: i32,
        cy: i32,
        radius: i32,
        max_dist: i32,
    ) -> Option<(i32, i32, i32)> {
        let fx = ox - cx;
        let fy = oy - cy;

        // a = dot(d, d), b = 2*dot(f, d), c = dot(f, f) - r^2
        let a = q16_mul(dx, dx) + q16_mul(dy, dy);
        let b = 2 * (q16_mul(fx, dx) + q16_mul(fy, dy));
        let c = q16_mul(fx, fx) + q16_mul(fy, fy) - q16_mul(radius, radius);

        // discriminant = b^2 - 4ac (all in Q16, careful with overflow)
        let disc = q16_mul(b, b) - 4 * q16_mul(a, c);
        if disc < 0 {
            return None;
        }

        let sqrt_disc = if disc < 0x7FFF {
            isqrt(disc << 16)
        } else {
            isqrt(disc) << 8
        };

        let two_a = 2 * a;
        if two_a == 0 {
            return None;
        }

        let t1 = q16_div(-b - sqrt_disc, two_a);
        let t2 = q16_div(-b + sqrt_disc, two_a);

        let t = if t1 >= 0 && t1 <= max_dist {
            t1
        } else if t2 >= 0 && t2 <= max_dist {
            t2
        } else {
            return None;
        };

        let hx = ox + q16_mul(dx, t);
        let hy = oy + q16_mul(dy, t);
        Some((hx, hy, t))
    }

    /// Get a body by id.
    fn get_body(&self, id: u32) -> Option<&RigidBody> {
        self.bodies.iter().find(|b| b.id == id && b.active)
    }

    /// Get the number of active bodies.
    fn active_count(&self) -> usize {
        self.bodies.iter().filter(|b| b.active).count()
    }
}

// --- Public API ---

/// Create a new rigid body. Returns its id.
pub fn create_body(x: i32, y: i32, mass: i32, collider: Collider, is_static: bool) -> u32 {
    let mut world = PHYSICS.lock();
    if let Some(ref mut w) = *world {
        w.create_body(x, y, mass, collider, is_static)
    } else {
        0
    }
}

/// Destroy a rigid body.
pub fn destroy_body(id: u32) -> bool {
    let mut world = PHYSICS.lock();
    if let Some(ref mut w) = *world {
        w.destroy_body(id)
    } else {
        false
    }
}

/// Step the simulation forward by dt (Q16 seconds).
pub fn step(dt: i32) {
    let mut world = PHYSICS.lock();
    if let Some(ref mut w) = *world {
        w.step(dt);
    }
}

/// Apply a force to a body (accumulated over the frame).
pub fn apply_force(id: u32, fx: i32, fy: i32) -> bool {
    let mut world = PHYSICS.lock();
    if let Some(ref mut w) = *world {
        w.apply_force(id, fx, fy)
    } else {
        false
    }
}

/// Apply an instantaneous impulse to a body.
pub fn apply_impulse(id: u32, ix: i32, iy: i32) -> bool {
    let mut world = PHYSICS.lock();
    if let Some(ref mut w) = *world {
        w.apply_impulse(id, ix, iy)
    } else {
        false
    }
}

/// Set global gravity (Q16).
pub fn set_gravity(gx: i32, gy: i32) {
    let mut world = PHYSICS.lock();
    if let Some(ref mut w) = *world {
        w.set_gravity(gx, gy);
    }
}

/// Cast a ray and find the nearest body hit.
pub fn raycast(ox: i32, oy: i32, dx: i32, dy: i32, max_dist: i32) -> Option<RaycastHit> {
    let world = PHYSICS.lock();
    if let Some(ref w) = *world {
        w.raycast(ox, oy, dx, dy, max_dist)
    } else {
        None
    }
}

pub fn init() {
    let mut world = PHYSICS.lock();
    *world = Some(PhysicsWorld::new());
    serial_println!(
        "    Physics: Q16 rigid body, AABB+circle colliders, impulse resolution, raycast"
    );
}
