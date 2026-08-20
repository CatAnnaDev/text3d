use glam::{Mat4, Vec3};

const MIN_PITCH: f32 = -1.45;
const MAX_PITCH: f32 = 1.45;
const MIN_DISTANCE: f32 = 1.5;
const MAX_DISTANCE: f32 = 400.0;
const ORBIT_SPEED: f32 = 0.006;
const FOLLOW_RATE: f32 = 14.0;

pub struct Camera {
    pub target: Vec3,
    pub focus: Vec3,
    pub pan: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov: f32,
}

impl Camera {
    pub fn new() -> Self {
        Camera {
            target: Vec3::ZERO,
            focus: Vec3::ZERO,
            pan: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.18,
            distance: 30.0,
            fov: 50.0_f32.to_radians(),
        }
    }

    pub fn reset_orientation(&mut self) {
        self.yaw = 0.0;
        self.pitch = 0.18;
        self.distance = 30.0;
        self.pan = Vec3::ZERO;
    }

    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(-sy * cp, -sp, -cy * cp)
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Y).normalize_or(Vec3::X)
    }

    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward()).normalize_or(Vec3::Y)
    }

    pub fn eye(&self) -> Vec3 {
        self.target - self.forward() * self.distance
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let eye = self.eye();
        let proj = glam::camera::rh::proj::directx::perspective(
            self.fov,
            aspect.max(0.01),
            0.05,
            2000.0,
        );
        proj * glam::camera::rh::view::look_at_mat4(eye, self.target, Vec3::Y)
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * ORBIT_SPEED;
        self.pitch = (self.pitch + dy * ORBIT_SPEED).clamp(MIN_PITCH, MAX_PITCH);
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let world_per_pixel = 2.0 * self.distance * (self.fov * 0.5).tan() / viewport_height.max(1.0);
        self.pan += self.right() * (-dx * world_per_pixel) + self.up() * (dy * world_per_pixel);
    }

    pub fn half_extent(&self, aspect: f32) -> (f32, f32) {
        let half_height = self.distance * (self.fov * 0.5).tan();
        (half_height * aspect, half_height)
    }

    pub fn zoom(&mut self, amount: f32) {
        self.distance = (self.distance * (1.0 - amount * 0.12)).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    pub fn set_focus(&mut self, focus: Vec3) {
        self.focus = focus;
    }

    pub fn snap(&mut self) {
        self.target = self.focus + self.pan;
    }

    pub fn update(&mut self, dt: f32) {
        let goal = self.focus + self.pan;
        let blend = 1.0 - (-FOLLOW_RATE * dt).exp();
        self.target += (goal - self.target) * blend;
    }
}
