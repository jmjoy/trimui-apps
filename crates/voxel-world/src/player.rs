use crate::world::{BORDER, HEIGHT, World, terrain};
use glam::{IVec3, Vec2, Vec3};
use serde::{Deserialize, Serialize};

pub const EYE: f32 = 1.62;
pub const SNEAK_EYE: f32 = 1.32;
const WALK: f32 = 4.317;
const SPRINT: f32 = 5.612;
const SNEAK: f32 = 1.31;
const FLY: f32 = 8.0;
const FLY_SPRINT: f32 = 14.0;

fn default_eye() -> f32 {
    EYE
}

/// What the controls ask the player to do during one simulation step.
#[derive(Clone, Copy, Default, Debug)]
pub struct Intent {
    /// Left stick: x right, y down (forward is negative).
    pub axis: Vec2,
    pub jump: bool,
    /// Sneaking on foot; descending while flying.
    pub sneak: bool,
    pub sprint: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Player {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
    #[serde(skip)]
    pub velocity: f32,
    #[serde(skip)]
    pub grounded: bool,
    #[serde(skip)]
    pub sneaking: bool,
    #[serde(skip)]
    pub sprinting: bool,
    /// Smoothed camera height above the feet.
    #[serde(skip, default = "default_eye")]
    pub eye_height: f32,
    /// Downward speed at the moment of landing during the last tick, or 0.
    #[serde(skip)]
    pub landed: f32,
    /// Horizontal distance actually moved during the last tick.
    #[serde(skip)]
    pub moved: f32,
}
impl Player {
    pub fn new(world: &World) -> Self {
        Self {
            position: world.spawn(),
            yaw: 0.0,
            pitch: -0.32,
            flying: false,
            velocity: 0.0,
            grounded: false,
            sneaking: false,
            sprinting: false,
            eye_height: EYE,
            landed: 0.0,
            moved: 0.0,
        }
    }
    pub fn eye(&self) -> Vec3 {
        self.position + Vec3::Y * self.eye_height
    }
    pub fn direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        )
    }
    pub fn look(&mut self, axis: Vec2, dt: f32, sensitivity: f32) {
        self.yaw = (self.yaw + axis.x * dt * 2.4 * sensitivity).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch - axis.y * dt * 1.8 * sensitivity).clamp(-1.54, 1.54);
    }
    pub fn overlaps(&self, p: IVec3) -> bool {
        let min = self.position - Vec3::new(0.3, 0.0, 0.3);
        let max = self.position + Vec3::new(0.3, 1.8, 0.3);
        let b = p.as_vec3();
        min.cmplt(b + Vec3::ONE).all() && max.cmpgt(b).all()
    }
    pub fn collides(&self, world: &World) -> bool {
        let min = (self.position - Vec3::new(0.3, 0.0, 0.3) + Vec3::splat(0.0001))
            .floor()
            .as_ivec3();
        let max = (self.position + Vec3::new(0.3, 1.8, 0.3) - Vec3::splat(0.0001))
            .floor()
            .as_ivec3();
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    if world.solid(IVec3::new(x, y, z)) {
                        return true;
                    }
                }
            }
        }
        false
    }
    /// Whether a solid block lies directly below any part of the footprint.
    fn supported(&self, world: &World) -> bool {
        let y = (self.position.y - 0.05).floor() as i32;
        let min = (self.position - Vec3::new(0.3, 0.0, 0.3) + Vec3::splat(0.0001)).floor();
        let max = (self.position + Vec3::new(0.3, 0.0, 0.3) - Vec3::splat(0.0001)).floor();
        for z in min.z as i32..=max.z as i32 {
            for x in min.x as i32..=max.x as i32 {
                if world.solid(IVec3::new(x, y, z)) {
                    return true;
                }
            }
        }
        false
    }
    /// Resets values that cannot come from normal play (a damaged save).
    /// Collisions are resolved later with `unstuck`, once terrain is loaded.
    pub fn restore_if_invalid(&mut self, seed: u64) {
        let limit = BORDER as f32 - 1.0;
        if !self.position.is_finite()
            || !self.yaw.is_finite()
            || !self.pitch.is_finite()
            || self.position.x.abs() > limit
            || self.position.z.abs() > limit
            || self.position.y < 1.0
            || self.position.y > HEIGHT as f32 + 32.0
        {
            *self = Self::new(&World::new(seed));
            self.position = terrain::spawn(seed);
        }
        self.pitch = self.pitch.clamp(-1.54, 1.54);
        self.yaw = self.yaw.rem_euclid(std::f32::consts::TAU);
    }
    /// Moves up out of blocks (e.g. after loading inside new terrain).
    /// Returns whether the position changed.
    pub fn unstuck(&mut self, world: &World) -> bool {
        let start = self.position.y;
        while self.collides(world) && self.position.y < HEIGHT as f32 + 1.0 {
            self.position.y = self.position.y.floor() + 1.0;
        }
        self.position.y != start
    }
    /// One fixed simulation step; returns whether the player jumped.
    pub fn tick(&mut self, world: &World, intent: Intent, dt: f32) -> bool {
        self.sneaking = intent.sneak && !self.flying;
        self.sprinting = intent.sprint && !self.sneaking && intent.axis.y < -0.3;
        let forward = Vec3::new(self.yaw.sin(), 0.0, -self.yaw.cos());
        let right = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let wish = (right * intent.axis.x - forward * intent.axis.y).clamp_length_max(1.0);
        let speed = match (self.flying, self.sneaking, self.sprinting) {
            (true, _, true) => FLY_SPRINT,
            (true, _, false) => FLY,
            (false, true, _) => SNEAK,
            (false, false, true) => SPRINT,
            (false, false, false) => WALK,
        };
        let mut delta = wish * dt * speed;
        let jumped = !self.flying && intent.jump && self.grounded;
        if self.flying {
            self.velocity = 0.0;
            delta.y = (i32::from(intent.jump) - i32::from(intent.sneak)) as f32 * dt * FLY;
        } else {
            if jumped {
                self.velocity = 7.2;
            }
            self.velocity = (self.velocity - 22.0 * dt).max(-18.0);
            delta.y = self.velocity * dt;
        }
        // Sneaking keeps the player on the block they stand on.
        let guard_edges = self.sneaking && self.grounded;
        let was_grounded = self.grounded;
        let start = self.position;
        self.landed = 0.0;
        self.grounded = false;
        let steps = (delta.abs().max_element() / 0.08).ceil().max(1.0) as usize;
        let delta = delta / steps as f32;
        for _ in 0..steps {
            for axis in [0, 2, 1] {
                let previous = self.position[axis];
                self.position[axis] += delta[axis];
                if self.collides(world) {
                    // Resolve to a block face rather than accumulating a visible gap.
                    if delta[axis] > 0.0 {
                        let extent = if axis == 1 { 1.8 } else { 0.3 };
                        self.position[axis] =
                            (self.position[axis] + extent - 0.0001).floor() - extent;
                    } else if delta[axis] < 0.0 {
                        let extent = if axis == 1 { 0.0 } else { 0.3 };
                        self.position[axis] =
                            (self.position[axis] - extent + 0.0001).floor() + 1.0 + extent;
                    }
                    if self.collides(world) {
                        self.position[axis] = previous;
                    }
                    if axis == 1 {
                        self.grounded = delta.y < 0.0;
                        if self.grounded && !was_grounded {
                            self.landed = self.landed.max(-self.velocity);
                        }
                        self.velocity = 0.0;
                    }
                } else if guard_edges && axis != 1 && !self.supported(world) {
                    self.position[axis] = previous;
                }
            }
        }
        if self.position.y > HEIGHT as f32 + 32.0 {
            self.position.y = HEIGHT as f32 + 32.0;
        }
        self.moved = (self.position - start).with_y(0.0).length();
        let eye = if self.sneaking { SNEAK_EYE } else { EYE };
        self.eye_height += (eye - self.eye_height) * (dt * 14.0).min(1.0);
        jumped
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn step(p: &mut Player, w: &World, intent: Intent, n: usize) {
        for _ in 0..n {
            p.tick(w, intent, 1.0 / 60.0);
        }
    }
    #[test]
    fn gravity_jump_wall_and_flight() {
        let mut w = World::new(42);
        w.generate_area(glam::IVec2::ZERO, 1);
        let mut p = Player::new(&w);
        step(&mut p, &w, Intent::default(), 120);
        assert!(p.grounded);
        let floor = p.position.y;
        let jump = Intent {
            jump: true,
            ..Default::default()
        };
        assert!(p.tick(&w, jump, 1.0 / 60.0));
        assert!(p.position.y > floor);
        let mut impact = 0.0f32;
        for _ in 0..120 {
            p.tick(&w, Intent::default(), 1.0 / 60.0);
            impact = impact.max(p.landed);
        }
        assert!(impact > 6.0 && impact < 8.0, "landing speed {impact}");
        p.tick(&w, Intent::default(), 1.0 / 60.0);
        assert_eq!(p.landed, 0.0, "standing still is not a landing");
        assert!((p.position.y - floor).abs() < 0.02);
        let wall = p.position.floor().as_ivec3() - IVec3::Z;
        w.set(wall, 3);
        w.set(wall + IVec3::Y, 3);
        let forward = Intent {
            axis: Vec2::new(0.0, -1.0),
            ..Default::default()
        };
        step(&mut p, &w, forward, 120);
        assert!(!p.collides(&w));
        assert!(p.position.z >= wall.z as f32 + 1.3 - 0.001);
        p.flying = true;
        let y = p.position.y;
        step(&mut p, &w, jump, 60);
        assert!(p.position.y > y + 7.0);
    }

    #[test]
    fn sneaking_stops_at_edges_and_sprinting_is_faster() {
        let mut w = World::new(0);
        w.empty_area(glam::IVec2::new(6, 6), 1);
        // A 3×3 platform at y = 10 surrounded by a long drop.
        for z in 99..102 {
            for x in 99..102 {
                w.set(IVec3::new(x, 10, z), 3);
            }
        }
        let mut p = Player::new(&w);
        p.position = Vec3::new(100.5, 11.0, 100.5);
        step(&mut p, &w, Intent::default(), 30);
        assert!(p.grounded);
        let sneak = Intent {
            axis: Vec2::new(0.0, -1.0),
            sneak: true,
            ..Default::default()
        };
        step(&mut p, &w, sneak, 240);
        assert!(p.sneaking && p.grounded);
        assert!(
            (p.position.y - 11.0).abs() < 1e-3,
            "fell off: {:?}",
            p.position
        );
        // The footprint may overhang the edge but never leaves the platform.
        assert!(
            p.position.z > 99.0 - 0.3 && p.position.z < 99.3,
            "{:?}",
            p.position
        );
        assert!(p.eye_height < EYE - 0.2);

        let mut walk = Player::new(&w);
        walk.flying = true;
        walk.position = Vec3::new(100.5, 40.0, 100.5);
        let mut run = walk.clone();
        let forward = Intent {
            axis: Vec2::new(0.0, -1.0),
            ..Default::default()
        };
        step(&mut walk, &w, forward, 60);
        step(
            &mut run,
            &w,
            Intent {
                sprint: true,
                ..forward
            },
            60,
        );
        assert!(run.sprinting && !walk.sprinting);
        assert!(100.5 - run.position.z > (100.5 - walk.position.z) * 1.5);
    }

    #[test]
    fn invalid_saves_reset_and_buried_players_climb_out() {
        let mut w = World::new(42);
        w.generate_area(glam::IVec2::ZERO, 1);
        let mut p = Player::new(&w);
        p.position = Vec3::new(f32::NAN, 0.0, 0.0);
        p.restore_if_invalid(42);
        assert_eq!(p.position, terrain::spawn(42));
        p.position = Vec3::new(3.5, 10.0, 3.5);
        assert!(p.collides(&w));
        assert!(p.unstuck(&w));
        assert!(!p.collides(&w));
        assert!(!p.unstuck(&w));
        // Unloaded terrain is solid: no falling into the void.
        p.position = Vec3::new(200.5, 100.0, 200.5);
        assert!(p.collides(&w));
    }
}
