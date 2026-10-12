use crate::world::{HEIGHT, WIDTH, World};
use glam::{IVec3, Vec2, Vec3};
use serde::{Deserialize, Serialize};

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
        }
    }
    pub fn eye(&self) -> Vec3 {
        self.position + Vec3::Y * 1.62
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
                    if x < 0
                        || z < 0
                        || x >= WIDTH
                        || z >= WIDTH
                        || y < 0
                        || world.get(IVec3::new(x, y, z)) != 0
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
    pub fn restore_if_invalid(&mut self, world: &World) {
        if !self.position.is_finite()
            || !self.yaw.is_finite()
            || !self.pitch.is_finite()
            || self.position.x < 0.3
            || self.position.z < 0.3
            || self.position.x > WIDTH as f32 - 0.3
            || self.position.z > WIDTH as f32 - 0.3
            || self.position.y < 1.0
            || self.position.y > HEIGHT as f32 + 32.0
            || self.collides(world)
        {
            *self = Self::new(world);
        }
        self.pitch = self.pitch.clamp(-1.54, 1.54);
    }
    pub fn tick(&mut self, world: &World, axis: Vec2, jump: bool, descend: bool, dt: f32) -> bool {
        let forward = Vec3::new(self.yaw.sin(), 0.0, -self.yaw.cos());
        let right = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let wish = (right * axis.x - forward * axis.y).clamp_length_max(1.0);
        let mut delta = wish * dt * if self.flying { 7.5 } else { 4.8 };
        let jumped = !self.flying && jump && self.grounded;
        if self.flying {
            self.velocity = 0.0;
            delta.y = (i32::from(jump) - i32::from(descend)) as f32 * dt * 7.5;
        } else {
            if jumped {
                self.velocity = 7.2;
            }
            self.velocity = (self.velocity - 22.0 * dt).max(-18.0);
            delta.y = self.velocity * dt;
        }
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
                        self.velocity = 0.0;
                    }
                }
            }
        }
        if self.position.y > HEIGHT as f32 + 32.0 {
            self.position.y = HEIGHT as f32 + 32.0;
        }
        jumped
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gravity_jump_wall_and_flight() {
        let mut w = World::generate(42);
        let mut p = Player::new(&w);
        for _ in 0..120 {
            p.tick(&w, Vec2::ZERO, false, false, 1.0 / 60.0);
        }
        assert!(p.grounded);
        let floor = p.position.y;
        assert!(p.tick(&w, Vec2::ZERO, true, false, 1.0 / 60.0));
        assert!(p.position.y > floor);
        for _ in 0..120 {
            p.tick(&w, Vec2::ZERO, false, false, 1.0 / 60.0);
        }
        assert!((p.position.y - floor).abs() < 0.02);
        let wall = p.position.floor().as_ivec3() - IVec3::Z;
        w.set(wall, 3);
        w.set(wall + IVec3::Y, 3);
        for _ in 0..120 {
            p.tick(&w, Vec2::new(0.0, -1.0), false, false, 1.0 / 60.0);
        }
        assert!(!p.collides(&w));
        assert!(p.position.z >= wall.z as f32 + 1.3 - 0.001);
        p.flying = true;
        let y = p.position.y;
        for _ in 0..60 {
            p.tick(&w, Vec2::ZERO, true, false, 1.0 / 60.0);
        }
        assert!(p.position.y > y + 7.0);
    }
}
