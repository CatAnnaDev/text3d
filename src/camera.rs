use glam::{Mat4, Vec3};

const MIN_PITCH: f32 = -1.45;
const MAX_PITCH: f32 = 1.45;
const MIN_DISTANCE: f32 = 1.5;
const MAX_DISTANCE: f32 = 400.0;
const ORBIT_SPEED: f32 = 0.006;
const FOLLOW_RATE: f32 = 14.0;
const AIM_RATE: f32 = 9.0;
const FRAME_MARGIN: f32 = 1.12;
const MIN_EXTENT: f32 = 0.5;
const AIM_ANGLE_SLACK: f32 = 0.0015;
const AIM_DISTANCE_SLACK: f32 = 0.02;
const AIM_POSITION_SLACK: f32 = 0.02;

pub struct Camera {
    pub target: Vec3,
    pub focus: Vec3,
    pub pan: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov: f32,
    aim_yaw: f32,
    aim_pitch: f32,
    aim_distance: f32,
    aiming: bool,
}

impl Default for Camera {
    fn default() -> Camera {
        Camera::new()
    }
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
            aim_yaw: 0.0,
            aim_pitch: 0.18,
            aim_distance: 30.0,
            aiming: false,
        }
    }

    pub fn reset_orientation(&mut self) {
        self.yaw = 0.0;
        self.pitch = 0.18;
        self.distance = 30.0;
        self.pan = Vec3::ZERO;
        self.release();
    }

    pub fn forward(&self) -> Vec3 {
        direction_of(self.yaw, self.pitch)
    }

    pub fn facing(&self) -> Vec3 {
        if self.aiming {
            direction_of(self.aim_yaw, self.aim_pitch)
        } else {
            self.forward()
        }
    }

    pub fn settled(&self) -> bool {
        !self.aiming
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
        self.release();
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let world_per_pixel = 2.0 * self.distance * (self.fov * 0.5).tan() / viewport_height.max(1.0);
        self.pan += self.right() * (-dx * world_per_pixel) + self.up() * (dy * world_per_pixel);
        self.release();
    }

    pub fn half_extent(&self, aspect: f32) -> (f32, f32) {
        let half_height = self.distance * (self.fov * 0.5).tan();
        (half_height * aspect, half_height)
    }

    pub fn zoom(&mut self, amount: f32) {
        self.distance = (self.distance * (1.0 - amount * 0.12)).clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.release();
    }

    pub fn set_focus(&mut self, focus: Vec3) {
        self.focus = focus;
    }

    pub fn snap(&mut self) {
        self.target = self.focus + self.pan;
        if self.aiming {
            self.settle();
        }
    }

    pub fn release(&mut self) {
        self.aim_yaw = self.yaw;
        self.aim_pitch = self.pitch;
        self.aim_distance = self.distance;
        self.aiming = false;
    }

    pub fn distance_for(&self, extent: f32) -> f32 {
        let tangent = (self.fov * 0.5).tan().max(1.0e-3);
        let wanted = if extent.is_finite() {
            extent.abs().max(MIN_EXTENT)
        } else {
            MIN_EXTENT
        };
        (wanted / tangent * FRAME_MARGIN).clamp(MIN_DISTANCE, MAX_DISTANCE)
    }

    pub fn face(&mut self, center: Vec3, normal: Vec3, extent: f32) {
        if !center.is_finite() || !normal.is_finite() {
            return;
        }
        let toward = normal.normalize_or_zero();
        if toward == Vec3::ZERO {
            return;
        }
        let forward = -toward;
        self.aim_pitch = (-forward.y).clamp(-1.0, 1.0).asin().clamp(MIN_PITCH, MAX_PITCH);
        let wanted = f32::atan2(-forward.x, -forward.z);
        self.aim_yaw = self.yaw + shortest_turn(wanted - self.yaw);
        self.aim_distance = self.distance_for(extent);
        self.focus = center;
        self.pan = Vec3::ZERO;
        self.aiming = true;
        if self.aim_reached() {
            self.settle();
        }
    }

    pub fn frame_selection(&mut self, min: Vec3, max: Vec3, aspect: f32) {
        let center = (min + max) * 0.5;
        let extent = ((max - min) * 0.5).abs();
        let tangent = (self.fov * 0.5).tan().max(1.0e-3);
        let fit_height = extent.y / tangent;
        let fit_width = extent.x / (tangent * aspect.max(0.01));
        let needed = fit_height.max(fit_width) * FRAME_MARGIN + extent.z + 1.0;
        self.distance = needed.clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.pan = Vec3::ZERO;
        self.focus = center;
        self.target = center;
        self.release();
    }

    pub fn update(&mut self, dt: f32) {
        let step = dt.max(0.0);
        let rate = if self.aiming { AIM_RATE } else { FOLLOW_RATE };
        let blend = 1.0 - (-rate * step).exp();
        if self.aiming {
            self.yaw += (self.aim_yaw - self.yaw) * blend;
            self.pitch += (self.aim_pitch - self.pitch) * blend;
            self.distance += (self.aim_distance - self.distance) * blend;
        }
        let goal = self.focus + self.pan;
        self.target += (goal - self.target) * blend;
        if self.aiming && self.aim_reached() {
            self.settle();
        }
    }

    fn aim_reached(&self) -> bool {
        (self.aim_yaw - self.yaw).abs() < AIM_ANGLE_SLACK
            && (self.aim_pitch - self.pitch).abs() < AIM_ANGLE_SLACK
            && (self.aim_distance - self.distance).abs() < AIM_DISTANCE_SLACK
            && (self.focus + self.pan - self.target).length_squared()
                < AIM_POSITION_SLACK * AIM_POSITION_SLACK
    }

    fn settle(&mut self) {
        self.yaw = self.aim_yaw;
        self.pitch = self.aim_pitch;
        self.distance = self.aim_distance;
        self.target = self.focus + self.pan;
        self.aiming = false;
    }
}

fn direction_of(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    Vec3::new(-sy * cp, -sp, -cy * cp)
}

fn shortest_turn(delta: f32) -> f32 {
    if !delta.is_finite() {
        return 0.0;
    }
    let wrapped = delta.rem_euclid(std::f32::consts::TAU);
    if wrapped > std::f32::consts::PI {
        wrapped - std::f32::consts::TAU
    } else {
        wrapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE_NORMAL: Vec3 = Vec3::new(0.0, 0.0, 1.0);
    const TREE_NORMAL: Vec3 = Vec3::new(1.0, 0.0, 0.0);
    const PROBLEMS_NORMAL: Vec3 = Vec3::new(-1.0, 0.0, 0.0);
    const OUTPUT_NORMAL: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    const RESULTS_NORMAL: Vec3 = Vec3::new(0.0, -1.0, 0.0);

    fn laisser_poser(camera: &mut Camera, duree: f32) {
        let pas = 1.0 / 120.0;
        let mut passe = 0.0;
        while passe < duree {
            camera.update(pas);
            passe += pas;
        }
    }

    #[test]
    fn la_visee_aligne_la_camera_sur_la_normale() {
        for normale in [CODE_NORMAL, TREE_NORMAL, PROBLEMS_NORMAL] {
            let mut camera = Camera::new();
            camera.yaw = 2.2;
            camera.pitch = -0.7;
            camera.face(Vec3::new(3.0, -4.0, 1.0), normale, 13.0);
            laisser_poser(&mut camera, 1.0);
            assert!(camera.settled(), "la visee doit se poser");
            let vue = camera.forward();
            assert!(
                vue.distance(-normale) < 1.0e-3,
                "vue {vue:?} attendue {:?}",
                -normale
            );
            assert!(camera.target.distance(Vec3::new(3.0, -4.0, 1.0)) < 1.0e-3);
        }
    }

    #[test]
    fn la_visee_du_sol_et_du_plafond_reste_sous_la_limite_de_tangage() {
        for (normale, signe) in [(OUTPUT_NORMAL, -1.0f32), (RESULTS_NORMAL, 1.0f32)] {
            let mut camera = Camera::new();
            camera.yaw = 1.9;
            camera.face(Vec3::ZERO, normale, 8.0);
            laisser_poser(&mut camera, 1.0);
            assert!(camera.settled());
            assert!(camera.pitch <= MAX_PITCH && camera.pitch >= MIN_PITCH);
            assert!(camera.yaw.abs() < 1.0e-3, "cap horizontal {}", camera.yaw);
            let vue = camera.forward();
            assert!(
                vue.y * signe > 0.98,
                "la vue doit plonger vers la surface, obtenu {vue:?}"
            );
        }
    }

    #[test]
    fn la_visee_prend_le_chemin_le_plus_court() {
        let mut camera = Camera::new();
        camera.yaw = 3.0;
        camera.release();
        let vue_voulue = Vec3::new(0.14112, 0.0, 0.98999).normalize();
        camera.face(Vec3::ZERO, -vue_voulue, 12.0);
        let parcours = (camera.aim_yaw - 3.0).abs();
        assert!(
            parcours < std::f32::consts::PI,
            "le cap doit tourner du cote court, parcours {parcours}"
        );
        assert!((camera.aim_yaw - 3.283185).abs() < 1.0e-3, "cap vise {}", camera.aim_yaw);
        laisser_poser(&mut camera, 1.0);
        assert!(camera.forward().distance(vue_voulue) < 1.0e-3);
    }

    #[test]
    fn la_visee_cadre_l_etendue_demandee() {
        let mut camera = Camera::new();
        camera.face(Vec3::ZERO, TREE_NORMAL, 13.0);
        laisser_poser(&mut camera, 1.0);
        let tangente = (camera.fov * 0.5).tan();
        let attendu = 13.0 / tangente * FRAME_MARGIN;
        assert!((camera.distance - attendu).abs() < 1.0e-2, "distance {}", camera.distance);
        let demi = camera.half_extent(1.6).1;
        assert!(demi > 13.0, "la surface doit tenir dans le cadre, demi hauteur {demi}");
    }

    #[test]
    fn la_visee_est_animee_et_non_un_saut() {
        let mut camera = Camera::new();
        camera.yaw = 0.0;
        camera.pitch = 0.18;
        camera.release();
        camera.face(Vec3::new(-34.0, 0.0, 0.0), TREE_NORMAL, 13.0);
        assert!(!camera.settled(), "la visee doit rester en cours");
        camera.update(1.0 / 120.0);
        let cap = camera.yaw;
        assert!(cap > 0.0 && cap < camera.aim_yaw * 0.5, "cap intermediaire {cap}");
        assert!(!camera.settled());
        let course = camera.aim_yaw;
        laisser_poser(&mut camera, 0.34);
        let reste = (camera.aim_yaw - camera.yaw).abs();
        assert!(
            reste < course * 0.05,
            "la visee doit etre faite a plus de 95 pour cent apres 0.35 s, reste {reste}"
        );
        laisser_poser(&mut camera, 1.0);
        assert!(camera.settled(), "la visee doit finir par se poser");
    }

    #[test]
    fn l_orbite_manuelle_annule_la_visee() {
        let mut camera = Camera::new();
        camera.face(Vec3::new(0.0, 0.0, -20.0), TREE_NORMAL, 13.0);
        assert!(!camera.settled());
        camera.orbit(12.0, 4.0);
        assert!(camera.settled(), "une orbite manuelle reprend la main");
        let cap = camera.yaw;
        laisser_poser(&mut camera, 0.5);
        assert!((camera.yaw - cap).abs() < 1.0e-6, "le cap ne doit plus bouger seul");
    }

    #[test]
    fn le_cap_vise_est_lisible_pendant_l_animation() {
        let mut camera = Camera::new();
        camera.face(Vec3::ZERO, PROBLEMS_NORMAL, 13.0);
        let vise = camera.facing();
        assert!(vise.distance(-PROBLEMS_NORMAL) < 1.0e-3, "cap vise {vise:?}");
        laisser_poser(&mut camera, 1.0);
        assert!(camera.facing().distance(camera.forward()) < 1.0e-6);
    }

    #[test]
    fn une_visee_deja_atteinte_se_declare_posee() {
        let mut camera = Camera::new();
        camera.face(Vec3::ZERO, CODE_NORMAL, 13.0);
        laisser_poser(&mut camera, 1.0);
        camera.face(Vec3::ZERO, CODE_NORMAL, 13.0);
        assert!(camera.settled(), "viser deux fois la meme surface ne relance rien");
    }

    #[test]
    fn une_normale_degeneree_ne_touche_pas_la_camera() {
        let mut camera = Camera::new();
        camera.yaw = 0.4;
        camera.pitch = 0.2;
        camera.release();
        camera.face(Vec3::ZERO, Vec3::ZERO, 13.0);
        assert!(camera.settled());
        assert!((camera.yaw - 0.4).abs() < 1.0e-6);
        camera.face(Vec3::new(f32::NAN, 0.0, 0.0), CODE_NORMAL, 13.0);
        assert!(camera.settled());
        assert!((camera.yaw - 0.4).abs() < 1.0e-6);
    }
}
