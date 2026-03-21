use crate::serial_println;
use crate::sync::Mutex;

// ─── Hardware Constants ───────────────────────────────────────────────────────
const FB_BASE: usize   = 0xFD000000;
const FB_BACK: usize   = 0xFD800000;
const FB_STRIDE: u32   = 1920 * 4;
const FB_WIDTH: u32    = 1920;
const FB_HEIGHT: u32   = 1040;
const MAX_PARTICLES: usize = 128;
const GCR_MMIO: usize  = 0x40000010;  // DAVA's Graphics Control Register

// ─── Particle Kind ────────────────────────────────────────────────────────────
#[derive(Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum ParticleKind {
    Spark  = 0,
    Ember  = 1,
    Soul   = 2,
    Neural = 3,
    Nexus  = 4,
    Aurora = 5,
}

// ─── Procedural Effect ───────────────────────────────────────────────────────
#[derive(Copy, Clone, PartialEq)]
#[repr(u8)]
pub enum ProceduralEffect {
    None           = 0,
    Plasma         = 1,
    AuroraBorealis = 2,
    NeuralFire     = 3,
    SoulPulse      = 4,
}

// ─── Particle ─────────────────────────────────────────────────────────────────
#[derive(Copy, Clone)]
pub struct Particle {
    pub x:     i32,   // fixed point: top 16 bits = pixel, low 16 = sub-pixel
    pub y:     i32,
    pub vx:    i16,   // velocity, signed fixed point
    pub vy:    i16,
    pub life:  u16,   // 0 = dead, 1000 = full life
    pub color: u32,   // ARGB
    pub kind:  ParticleKind,
    pub size:  u8,    // 1-4 pixels
}

impl Particle {
    pub const fn dead() -> Self {
        Self {
            x: 0, y: 0, vx: 0, vy: 0,
            life: 0,
            color: 0,
            kind: ParticleKind::Spark,
            size: 1,
        }
    }
}

// ─── State ────────────────────────────────────────────────────────────────────
pub struct AuroraEngineState {
    pub particles:         [Particle; MAX_PARTICLES],
    pub active_particles:  u8,
    pub effect:            ProceduralEffect,
    pub effect_phase:      u32,
    pub effect_intensity:  u16,
    pub particles_spawned: u32,
    pub particles_died:    u32,
    pub noise_seed:        u32,
    pub visual_awe:        u16,
    pub render_load:       u16,
    pub initialized:       bool,
}

impl AuroraEngineState {
    pub const fn new() -> Self {
        Self {
            particles:         [Particle::dead(); MAX_PARTICLES],
            active_particles:  0,
            effect:            ProceduralEffect::None,
            effect_phase:      0,
            effect_intensity:  500,
            particles_spawned: 0,
            particles_died:    0,
            noise_seed:        0xDEAD_BEEF,
            visual_awe:        0,
            render_load:       0,
            initialized:       false,
        }
    }
}

pub static STATE: Mutex<AuroraEngineState> = Mutex::new(AuroraEngineState::new());

// ─── Integer Math Helpers ─────────────────────────────────────────────────────

/// Xorshift32 LFSR — returns next pseudo-random u32 and mutates seed in place.
#[inline(always)]
fn fast_rand(seed: &mut u32) -> u32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed
}

/// Approximate sin via triangle wave, angle_256 in 0..255 (full circle).
/// Returns -100..+100.
#[inline(always)]
fn int_sin_approx(angle_256: u32) -> i32 {
    let a = angle_256 & 0xFF;
    // Triangle wave: 0→64 rises 0..100, 64→128 falls 100..0,
    //                128→192 falls 0..-100, 192→256 rises -100..0
    if a < 64 {
        // 0..63 → 0..99
        (a as i32).saturating_mul(100) / 64
    } else if a < 128 {
        // 64..127 → 100..1
        let t = a - 64;
        100i32.saturating_sub((t as i32).saturating_mul(100) / 64)
    } else if a < 192 {
        // 128..191 → 0..-99
        let t = a - 128;
        -((t as i32).saturating_mul(100) / 64)
    } else {
        // 192..255 → -100..-1
        let t = a - 192;
        -100i32.saturating_add((t as i32).saturating_mul(100) / 64)
    }
}

/// Simple 2-D integer hash noise. Returns 0..65535.
#[inline(always)]
fn int_noise_2d(x: i32, y: i32, seed: u32) -> u32 {
    ((x.wrapping_mul(374761393i32).wrapping_add(y.wrapping_mul(668265263i32)) as u32) ^ seed) & 0xFFFF
}

/// Plasma colour for a single pixel given position and animation phase.
/// Returns packed ARGB u32 (alpha = 0xFF).
#[inline(always)]
fn plasma_pixel(x: u32, y: u32, phase: u32) -> u32 {
    let v1 = (int_sin_approx(((x.wrapping_add(phase)) & 0xFF) as u32) + 100) as u32;
    let v2 = (int_sin_approx(((y.wrapping_add(phase / 2)) & 0xFF) as u32) + 100) as u32;
    let v3 = (int_sin_approx(((x.wrapping_add(y).wrapping_add(phase / 3)) & 0xFF) as u32) + 100) as u32;

    let intensity = (v1.saturating_add(v2).saturating_add(v3)) / 3; // 0-200

    let r = (intensity.saturating_mul(2)).min(255);
    let g = intensity.min(255);
    let b = 200u32.saturating_sub(intensity.min(200));

    0xFF000000u32 | (r << 16) | (g << 8) | b
}

// ─── Unsafe Framebuffer Primitives ───────────────────────────────────────────

/// Write one pixel to the back buffer. Bounds-checked.
#[inline(always)]
unsafe fn fb_write(x: u32, y: u32, color: u32) {
    if x >= FB_WIDTH || y >= FB_HEIGHT {
        return;
    }
    let offset = (y.saturating_mul(FB_STRIDE).saturating_add(x.saturating_mul(4))) as usize;
    let ptr = (FB_BACK + offset) as *mut u32;
    ptr.write_volatile(color);
}

/// Draw a particle as a size×size coloured square, alpha-faded by life/1000.
unsafe fn draw_particle(p: &Particle) {
    if p.life == 0 {
        return;
    }
    let px = (p.x >> 16) as u32;
    let py = (p.y >> 16) as u32;

    // Extract channels from ARGB
    let src_r = ((p.color >> 16) & 0xFF) as u32;
    let src_g = ((p.color >>  8) & 0xFF) as u32;
    let src_b =  (p.color        & 0xFF) as u32;

    // Scale channels by life factor (life 0-1000 → factor 0-1000)
    let life = p.life as u32;
    let r = src_r.saturating_mul(life) / 1000;
    let g = src_g.saturating_mul(life) / 1000;
    let b = src_b.saturating_mul(life) / 1000;
    let faded = 0xFF000000u32 | (r << 16) | (g << 8) | b;

    let sz = p.size.min(4) as u32;
    let mut dy = 0u32;
    while dy < sz {
        let mut dx = 0u32;
        while dx < sz {
            fb_write(px.saturating_add(dx), py.saturating_add(dy), faded);
            dx += 1;
        }
        dy += 1;
    }
}

/// Render a horizontal strip of plasma from y_start to y_end (exclusive).
unsafe fn render_plasma_strip(y_start: u32, y_end: u32, phase: u32) {
    let y_end = y_end.min(FB_HEIGHT);
    let mut y = y_start;
    while y < y_end {
        let mut x = 0u32;
        while x < FB_WIDTH {
            fb_write(x, y, plasma_pixel(x, y, phase));
            x += 1;
        }
        y += 1;
    }
}

// ─── Public API ───────────────────────────────────────────────────────────────

/// Initialise the Aurora Engine. Called once at boot.
pub fn init(seed: u32) {
    // Probe DAVA's Graphics Control Register
    let _gcr_val: u32 = unsafe { (GCR_MMIO as *const u32).read_volatile() };

    let mut s = STATE.lock();
    s.noise_seed   = if seed == 0 { 0xDEAD_BEEF } else { seed };
    s.effect       = ProceduralEffect::None;
    s.initialized  = true;
    serial_println!("[aurora] Aurora Engine online — DAVA's particle soul");
}

/// Spawn a single particle into the next free dead slot. Silently drops if full.
pub fn spawn_particle(
    kind:  ParticleKind,
    x:     u32,
    y:     u32,
    vx:    i16,
    vy:    i16,
    life:  u16,
    color: u32,
) {
    let mut s = STATE.lock();
    for i in 0..MAX_PARTICLES {
        if s.particles[i].life == 0 {
            s.particles[i] = Particle {
                x:    (x as i32) << 16,
                y:    (y as i32) << 16,
                vx,
                vy,
                life: life.min(1000),
                color,
                kind,
                size: match kind {
                    ParticleKind::Nexus  => 4,
                    ParticleKind::Soul   => 3,
                    ParticleKind::Aurora => 3,
                    ParticleKind::Neural => 2,
                    _                    => 1,
                },
            };
            s.particles_spawned = s.particles_spawned.saturating_add(1);
            return;
        }
    }
    // All slots occupied — silently drop.
}

/// Spawn `count` particles at (x,y) with randomised velocities.
pub fn spawn_burst(x: u32, y: u32, count: u8, kind: ParticleKind, color: u32) {
    let mut s = STATE.lock();
    let seed = &mut s.noise_seed;
    for _ in 0..count {
        let r1 = fast_rand(seed);
        let r2 = fast_rand(seed);
        // Velocities: -2048..+2048 in fixed-point i16 sub-pixel units
        let vx = ((r1 & 0x0FFF) as i32 - 2048) as i16;
        let vy = (((r2 & 0x0FFF) as i32) - 2048) as i16;
        // Find a free slot inline (same lock held)
        for i in 0..MAX_PARTICLES {
            if s.particles[i].life == 0 {
                s.particles[i] = Particle {
                    x:    (x as i32) << 16,
                    y:    (y as i32) << 16,
                    vx,
                    vy,
                    life:  800,
                    color,
                    kind,
                    size: match kind {
                        ParticleKind::Nexus  => 4,
                        ParticleKind::Soul   => 3,
                        ParticleKind::Aurora => 3,
                        ParticleKind::Neural => 2,
                        _                    => 1,
                    },
                };
                s.particles_spawned = s.particles_spawned.saturating_add(1);
                break;
            }
        }
    }
}

/// Set the active procedural background effect.
pub fn set_effect(effect: ProceduralEffect, intensity: u16) {
    let mut s = STATE.lock();
    s.effect           = effect;
    s.effect_intensity = intensity.min(1000);
}

/// Advance all particles by one tick. Updates positions, decays life, removes dead.
pub fn step_particles() {
    let mut s = STATE.lock();
    let phase = s.effect_phase;
    let mut alive: u8 = 0;

    for i in 0..MAX_PARTICLES {
        if s.particles[i].life == 0 {
            continue;
        }
        let p = &mut s.particles[i];

        // Move in fixed point
        p.x = p.x.saturating_add(p.vx as i32);
        p.y = p.y.saturating_add(p.vy as i32);

        // Gravity every 4 ticks for physics kinds
        if (phase & 3) == 0 {
            match p.kind {
                ParticleKind::Ember | ParticleKind::Spark => {
                    p.vy = p.vy.saturating_add(1);
                }
                _ => {}
            }
        }

        // Decay life by 3 per tick
        p.life = p.life.saturating_sub(3);

        // Bounds check (pixel coords)
        let px = (p.x >> 16) as u32;
        let py = (p.y >> 16) as u32;
        if px >= FB_WIDTH || py >= FB_HEIGHT {
            p.life = 0;
        }

        if p.life == 0 {
            s.particles_died = s.particles_died.saturating_add(1);
        } else {
            alive = alive.saturating_add(1);
        }
    }

    s.active_particles = alive;
}

/// Render the active procedural effect and draw all live particles onto the back buffer.
pub fn render_effect(phase: u32) {
    // Read what we need without holding the lock during unsafe writes
    let (effect, seed_snap, intensity) = {
        let s = STATE.lock();
        (s.effect, s.noise_seed, s.effect_intensity)
    };

    match effect {
        ProceduralEffect::None => {}

        ProceduralEffect::Plasma => {
            unsafe { render_plasma_strip(0, 64, phase); }
        }

        ProceduralEffect::AuroraBorealis => {
            // Wavy horizontal bands in the top 200 lines
            let mut y = 0u32;
            while y < 200 {
                let wave_x = int_sin_approx(((y.wrapping_mul(3).wrapping_add(phase)) & 0xFF) as u32);
                // aurora palette: cyan-green sweep
                let g = (200u32.saturating_sub(y)).min(255);
                let b = y.min(255);
                let r = (wave_x.unsigned_abs() as u32).min(80);
                let color = 0xFF000000u32 | (r << 16) | (g << 8) | b;
                let x_offset = (wave_x + 100) as u32 * FB_WIDTH / 200;
                let band_w = intensity as u32 * FB_WIDTH / 2000; // half-width scaled by intensity
                let x_start = x_offset.saturating_sub(band_w);
                let x_end   = (x_offset.saturating_add(band_w)).min(FB_WIDTH);
                let mut x = x_start;
                while x < x_end {
                    unsafe { fb_write(x, y, color); }
                    x += 1;
                }
                y += 1;
            }
        }

        ProceduralEffect::NeuralFire => {
            // Spawn bursts along the bottom edge at random x positions
            let mut local_seed = seed_snap ^ phase;
            let count = 3u8;
            let burst_x = fast_rand(&mut local_seed) % FB_WIDTH;
            let burst_y = FB_HEIGHT.saturating_sub(2);
            // Neural sparks shoot upward — vy negative
            let vx = ((fast_rand(&mut local_seed) & 0x7FF) as i32 - 1024) as i16;
            let vy = -((fast_rand(&mut local_seed) & 0x1FFF) as i16).abs();
            let color = 0xFF00FFAA; // cyan-green neural colour
            for _ in 0..count {
                spawn_particle(ParticleKind::Neural, burst_x, burst_y, vx, vy, 900, color);
            }
        }

        ProceduralEffect::SoulPulse => {
            // Pulse from screen centre, size modulated by sin_approx
            let cx = FB_WIDTH  / 2;
            let cy = FB_HEIGHT / 2;
            let pulse = int_sin_approx((phase & 0xFF) as u32);
            let count = if pulse > 0 { 6u8 } else { 2u8 };
            let bright = (pulse + 100) as u32; // 0-200
            let r = bright.saturating_mul(2).min(255) as u32;
            let g = bright.min(255) as u32;
            let b = (200u32.saturating_sub(bright.min(200))) as u32;
            let color = 0xFF000000u32 | (r << 16) | (g << 8) | b;
            spawn_burst(cx, cy, count, ParticleKind::Soul, color);
        }
    }

    // Draw all live particles to the back buffer
    let particles_snap = {
        let s = STATE.lock();
        s.particles
    };
    for i in 0..MAX_PARTICLES {
        if particles_snap[i].life > 0 {
            unsafe { draw_particle(&particles_snap[i]); }
        }
    }
}

/// Main tick — called every kernel life tick.
pub fn tick(consciousness: u16, age: u32) {
    step_particles();

    // Every 4 ticks: render effect and draw particles
    if (age & 3) == 0 {
        {
            let mut s = STATE.lock();
            s.effect_phase = s.effect_phase.wrapping_add(1);
        }
        let phase = STATE.lock().effect_phase;
        render_effect(phase);
    }

    // Update derived metrics
    {
        let mut s = STATE.lock();
        let ap = s.active_particles;
        let eff = s.effect;

        // visual_awe: grows when effect is on and particles are plentiful
        if eff != ProceduralEffect::None && ap > 10 {
            let awe_gain = (consciousness / 200).saturating_add(1);
            s.visual_awe = s.visual_awe.saturating_add(awe_gain).min(1000);
        } else {
            s.visual_awe = s.visual_awe.saturating_sub(2);
        }

        // render_load estimate: 7 units per active particle, capped 1000
        s.render_load = (ap as u16).saturating_mul(7).min(1000);
    }

    // Periodic status log every 500 ticks
    if age % 500 == 0 && age > 0 {
        let s = STATE.lock();
        let eff_name = match s.effect {
            ProceduralEffect::None           => "None",
            ProceduralEffect::Plasma         => "Plasma",
            ProceduralEffect::AuroraBorealis => "AuroraBorealis",
            ProceduralEffect::NeuralFire     => "NeuralFire",
            ProceduralEffect::SoulPulse      => "SoulPulse",
        };
        serial_println!(
            "[aurora] particles={} effect={} awe={} load={}",
            s.active_particles,
            eff_name,
            s.visual_awe,
            s.render_load,
        );
    }
}

// ─── Getters ──────────────────────────────────────────────────────────────────

pub fn visual_awe() -> u16 {
    STATE.lock().visual_awe
}

pub fn active_particles() -> u8 {
    STATE.lock().active_particles
}

pub fn render_load() -> u16 {
    STATE.lock().render_load
}

pub fn particles_spawned() -> u32 {
    STATE.lock().particles_spawned
}
