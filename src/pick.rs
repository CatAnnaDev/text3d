use glam::Vec3;

use crate::camera::Camera;
use crate::font::Font;
use crate::hud::{Basis, Hud, Surface};
use crate::layout::LineLayout;
use crate::render::indent_level;
use crate::text::TextBuffer;

const PARALLEL_EPSILON: f32 = 1.0e-5;
const LEFT_MARGIN_EM: f32 = 2.0;

pub struct Hit {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct SurfaceHit {
    pub surface: Surface,
    pub u: f32,
    pub v: f32,
    pub distance: f32,
}

pub fn surface_normal(basis: &Basis) -> Vec3 {
    basis.down.cross(basis.right).normalize_or_zero()
}

pub fn surface_center(basis: &Basis) -> Vec3 {
    basis.origin + basis.right * (basis.width * 0.5) + basis.down * (basis.height * 0.5)
}

pub fn ray_from_screen(
    camera: &Camera,
    aspect: f32,
    width: f32,
    height: f32,
    x: f32,
    y: f32,
) -> (Vec3, Vec3) {
    let tangent = (camera.fov * 0.5).tan();
    let ndc_x = 2.0 * x / width.max(1.0) - 1.0;
    let ndc_y = 1.0 - 2.0 * y / height.max(1.0);
    let forward = camera.forward();
    let direction = forward
        + camera.right() * (ndc_x * tangent * aspect.max(0.01))
        + camera.up() * (ndc_y * tangent);
    (camera.eye(), direction.normalize_or(forward))
}

pub fn hit_surfaces(
    camera: &Camera,
    aspect: f32,
    width: f32,
    height: f32,
    x: f32,
    y: f32,
    hud: &Hud,
) -> Option<SurfaceHit> {
    let (origin, direction) = ray_from_screen(camera, aspect, width, height, x, y);
    let mut closest: Option<SurfaceHit> = None;
    for &surface in hud.surfaces() {
        keep_closest(&mut closest, origin, direction, surface, &hud.basis(surface));
    }
    closest
}

pub fn hit_bases(
    camera: &Camera,
    aspect: f32,
    width: f32,
    height: f32,
    x: f32,
    y: f32,
    bases: &[(Surface, Basis)],
) -> Option<SurfaceHit> {
    let (origin, direction) = ray_from_screen(camera, aspect, width, height, x, y);
    let mut closest: Option<SurfaceHit> = None;
    for (surface, basis) in bases {
        keep_closest(&mut closest, origin, direction, *surface, basis);
    }
    closest
}

pub fn hit_basis(
    camera: &Camera,
    aspect: f32,
    width: f32,
    height: f32,
    x: f32,
    y: f32,
    surface: Surface,
    basis: &Basis,
) -> Option<SurfaceHit> {
    let (origin, direction) = ray_from_screen(camera, aspect, width, height, x, y);
    let (u, v, distance) = intersect_basis(origin, direction, basis)?;
    Some(SurfaceHit { surface, u, v, distance })
}

pub fn hit_text(
    camera: &Camera,
    aspect: f32,
    width: f32,
    height: f32,
    x: f32,
    y: f32,
    text: &TextBuffer,
    font: &Font,
    layout: &mut LineLayout,
    indent_depth: f32,
) -> Option<Hit> {
    let (origin, direction) = ray_from_screen(camera, aspect, width, height, x, y);
    let line_height = font.line_height();
    if line_height <= 0.0 {
        return None;
    }

    let mut plane = 0.0f32;
    let mut point = intersect(origin, direction, plane)?;
    let mut line = row_at(point.y, line_height, text)?;

    if indent_depth > 0.0 {
        for _ in 0..2 {
            let depth = -(indent_level(text.lines[line].as_str()) as f32) * indent_depth;
            if (depth - plane).abs() < PARALLEL_EPSILON {
                break;
            }
            plane = depth;
            point = intersect(origin, direction, plane)?;
            line = row_at(point.y, line_height, text)?;
        }
    }

    if point.x < -LEFT_MARGIN_EM * font.advance() {
        return None;
    }
    layout.build(font, text.lines[line].as_str());
    Some(Hit { line, column: layout.column_at_x(point.x) })
}

fn keep_closest(
    closest: &mut Option<SurfaceHit>,
    origin: Vec3,
    direction: Vec3,
    surface: Surface,
    basis: &Basis,
) {
    if surface == Surface::Screen {
        return;
    }
    let Some((u, v, distance)) = intersect_basis(origin, direction, basis) else {
        return;
    };
    if closest.is_none_or(|found| distance < found.distance) {
        *closest = Some(SurfaceHit { surface, u, v, distance });
    }
}

fn intersect_basis(origin: Vec3, direction: Vec3, basis: &Basis) -> Option<(f32, f32, f32)> {
    let normal = surface_normal(basis);
    if normal == Vec3::ZERO {
        return None;
    }
    let slope = direction.dot(normal);
    if slope > -PARALLEL_EPSILON {
        return None;
    }
    let travel = (basis.origin - origin).dot(normal) / slope;
    if travel <= 0.0 || !travel.is_finite() {
        return None;
    }
    let span_right = basis.right.length_squared();
    let span_down = basis.down.length_squared();
    if span_right < PARALLEL_EPSILON || span_down < PARALLEL_EPSILON {
        return None;
    }
    let offset = origin + direction * travel - basis.origin;
    let u = offset.dot(basis.right) / span_right;
    let v = offset.dot(basis.down) / span_down;
    if u < 0.0 || v < 0.0 || u > basis.width || v > basis.height {
        return None;
    }
    Some((u, v, travel))
}

fn intersect(origin: Vec3, direction: Vec3, plane: f32) -> Option<Vec3> {
    if direction.z.abs() < PARALLEL_EPSILON {
        return None;
    }
    let travel = (plane - origin.z) / direction.z;
    if travel <= 0.0 || !travel.is_finite() {
        return None;
    }
    Some(origin + direction * travel)
}

fn row_at(y: f32, line_height: f32, text: &TextBuffer) -> Option<usize> {
    let row = (-y / line_height).round();
    if row < 0.0 {
        return None;
    }
    let line = row as usize;
    if line >= text.line_count() {
        return None;
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::INDENT_DEPTH;
    use glam::Vec4;

    const ROOM_X: f32 = 34.0;
    const ROOM_Y: f32 = 15.0;
    const VIEW_WIDTH: f32 = 1600.0;
    const VIEW_HEIGHT: f32 = 1000.0;

    fn placed(center: Vec3, right: Vec3, down: Vec3, width: f32, height: f32) -> Basis {
        Basis {
            origin: center - right * (width * 0.5) - down * (height * 0.5),
            right,
            down,
            width,
            height,
        }
    }

    fn room(focus: Vec3) -> Vec<(Surface, Basis)> {
        let up = Vec3::Y;
        let flat = -up;
        vec![
            (Surface::Code, placed(focus, Vec3::X, flat, 40.0, 24.0)),
            (
                Surface::Tabs,
                placed(focus + up * 13.0, Vec3::X, flat, 40.0, 2.0),
            ),
            (
                Surface::Tree,
                placed(focus - Vec3::X * ROOM_X, -Vec3::Z, flat, 28.0, 26.0),
            ),
            (
                Surface::Problems,
                placed(focus + Vec3::X * ROOM_X, Vec3::Z, flat, 28.0, 26.0),
            ),
            (
                Surface::Output,
                placed(focus - up * ROOM_Y, Vec3::X, Vec3::Z, 60.0, 16.0),
            ),
            (
                Surface::Results,
                placed(focus + up * ROOM_Y, Vec3::X, -Vec3::Z, 60.0, 16.0),
            ),
        ]
    }

    fn orbited_at(basis: &Basis) -> Camera {
        let mut camera = Camera::new();
        let extent = basis.width.max(basis.height) * 0.5;
        camera.face(surface_center(basis), surface_normal(basis), extent);
        camera.snap();
        let sway = if camera.pitch > 0.0 { -70.0 } else { 70.0 };
        camera.orbit(-95.0, sway);
        camera.snap();
        camera
    }

    fn to_screen(view: &glam::Mat4, point: Vec3) -> (f32, f32, f32) {
        let clip = *view * Vec4::new(point.x, point.y, point.z, 1.0);
        (
            (clip.x / clip.w * 0.5 + 0.5) * VIEW_WIDTH,
            (0.5 - clip.y / clip.w * 0.5) * VIEW_HEIGHT,
            clip.w,
        )
    }

    #[test]
    fn la_base_de_chaque_surface_est_orthogonale_et_tournee_vers_la_piece() {
        let focus = Vec3::new(6.0, -9.0, 0.0);
        for (surface, basis) in room(focus) {
            assert!(
                basis.right.dot(basis.down).abs() < 1.0e-5,
                "{surface:?} doit avoir une base orthogonale"
            );
            let normal = surface_normal(&basis);
            assert!(normal.length() > 0.99, "{surface:?} sans normale");
            let center = surface_center(&basis);
            let reader = focus + Vec3::Z * 12.0;
            assert!(
                normal.dot((reader - center).normalize()) > 0.5,
                "{surface:?} doit tourner sa face vers l interieur de la piece"
            );
        }
    }

    #[test]
    fn aller_retour_ecran_surface_pour_chaque_surface() {
        let focus = Vec3::new(6.0, -9.0, 0.0);
        let aspect = VIEW_WIDTH / VIEW_HEIGHT;
        for (surface, basis) in room(focus) {
            let camera = orbited_at(&basis);
            assert!(
                camera.forward().dot(surface_normal(&basis)) < -0.5,
                "{surface:?} doit rester vue de face"
            );
            assert!(
                camera.forward().dot(surface_normal(&basis)) > -0.999,
                "{surface:?} doit etre vue de biais, pas pile de face"
            );
            let view = camera.view_proj(aspect);
            let probes = [
                (0.04, 0.06),
                (0.5, 0.5),
                (0.96, 0.94),
                (0.08, 0.9),
                (0.9, 0.05),
            ];
            for (part_u, part_v) in probes {
                let (u, v) = (basis.width * part_u, basis.height * part_v);
                let world = basis.origin + basis.right * u + basis.down * v;
                let (screen_x, screen_y, w) = to_screen(&view, world);
                assert!(w > 0.0, "{surface:?} projete derriere la camera");
                let hit = hit_basis(
                    &camera,
                    aspect,
                    VIEW_WIDTH,
                    VIEW_HEIGHT,
                    screen_x,
                    screen_y,
                    surface,
                    &basis,
                )
                .unwrap_or_else(|| panic!("{surface:?} perd le point local {u} {v}"));
                assert_eq!(hit.surface, surface);
                assert!(
                    (hit.u - u).abs() < 0.01,
                    "{surface:?} u vise {u} retrouve {}",
                    hit.u
                );
                assert!(
                    (hit.v - v).abs() < 0.01,
                    "{surface:?} v vise {v} retrouve {}",
                    hit.v
                );
                assert!(hit.distance > 0.0);
            }
        }
    }

    #[test]
    fn le_pointage_ignore_ce_qui_deborde_de_la_surface() {
        let focus = Vec3::ZERO;
        let aspect = VIEW_WIDTH / VIEW_HEIGHT;
        for (surface, basis) in room(focus) {
            let camera = orbited_at(&basis);
            let view = camera.view_proj(aspect);
            for (u, v) in [
                (-1.5, basis.height * 0.5),
                (basis.width + 1.5, basis.height * 0.5),
                (basis.width * 0.5, -1.5),
                (basis.width * 0.5, basis.height + 1.5),
            ] {
                let world = basis.origin + basis.right * u + basis.down * v;
                let (screen_x, screen_y, w) = to_screen(&view, world);
                assert!(w > 0.0);
                assert!(
                    hit_basis(
                        &camera,
                        aspect,
                        VIEW_WIDTH,
                        VIEW_HEIGHT,
                        screen_x,
                        screen_y,
                        surface,
                        &basis,
                    )
                    .is_none(),
                    "{surface:?} ne doit pas repondre hors de ses bornes en {u} {v}"
                );
            }
        }
    }

    #[test]
    fn le_dos_d_une_surface_n_est_pas_pointable() {
        let aspect = VIEW_WIDTH / VIEW_HEIGHT;
        let basis = placed(Vec3::new(-34.0, 0.0, 0.0), -Vec3::Z, -Vec3::Y, 28.0, 26.0);
        let center = surface_center(&basis);
        let normal = surface_normal(&basis);

        let mut devant = Camera::new();
        devant.face(center, normal, basis.height * 0.5);
        devant.snap();
        let vu = hit_basis(
            &devant,
            aspect,
            VIEW_WIDTH,
            VIEW_HEIGHT,
            VIEW_WIDTH * 0.5,
            VIEW_HEIGHT * 0.5,
            Surface::Tree,
            &basis,
        )
        .expect("de face la surface doit repondre");
        assert!((vu.u - basis.width * 0.5).abs() < 0.05);
        assert!((vu.v - basis.height * 0.5).abs() < 0.05);

        let mut derriere = Camera::new();
        derriere.face(center, -normal, basis.height * 0.5);
        derriere.snap();
        assert!(
            hit_basis(
                &derriere,
                aspect,
                VIEW_WIDTH,
                VIEW_HEIGHT,
                VIEW_WIDTH * 0.5,
                VIEW_HEIGHT * 0.5,
                Surface::Tree,
                &basis,
            )
            .is_none(),
            "le dos d une surface ne doit jamais repondre"
        );
    }

    #[test]
    fn le_pointage_retient_la_surface_la_plus_proche() {
        let aspect = VIEW_WIDTH / VIEW_HEIGHT;
        let proche = placed(Vec3::ZERO, Vec3::X, -Vec3::Y, 8.0, 6.0);
        let loin = placed(Vec3::new(0.0, 0.0, -18.0), Vec3::X, -Vec3::Y, 60.0, 40.0);
        let ecran = placed(Vec3::new(0.0, 0.0, 8.0), Vec3::X, -Vec3::Y, 60.0, 40.0);
        let bases = [
            (Surface::Screen, ecran),
            (Surface::Problems, loin),
            (Surface::Code, proche),
        ];
        let mut camera = Camera::new();
        camera.face(Vec3::ZERO, Vec3::Z, 5.0);
        camera.snap();
        camera.orbit(-60.0, 40.0);
        camera.snap();

        let hit = hit_bases(
            &camera,
            aspect,
            VIEW_WIDTH,
            VIEW_HEIGHT,
            VIEW_WIDTH * 0.5,
            VIEW_HEIGHT * 0.5,
            &bases,
        )
        .expect("le rayon doit toucher quelque chose");
        assert_eq!(hit.surface, Surface::Code, "la surface la plus proche gagne");

        let dehors = hit_bases(
            &camera,
            aspect,
            VIEW_WIDTH,
            VIEW_HEIGHT,
            VIEW_WIDTH * 0.5 + 600.0,
            VIEW_HEIGHT * 0.5,
            &bases,
        )
        .expect("le rayon doit passer a cote et toucher le fond");
        assert_eq!(dehors.surface, Surface::Problems);
    }

    #[test]
    fn le_pointage_suit_le_relief_d_indentation() {
        let Ok(font) = Font::load() else {
            return;
        };
        let source = "fn dehors() {\n    let a = 1;\n        let plus_loin = 2;\n                                let tres_profond = 3;\n}\n";
        let text = TextBuffer::from_str(source, None);
        let mut camera = Camera::new();
        camera.yaw = 0.85;
        camera.pitch = 0.22;
        camera.distance = 22.0;
        camera.release();
        camera.target = Vec3::new(14.0, -2.4, 0.0);

        let aspect = VIEW_WIDTH / VIEW_HEIGHT;
        let view = camera.view_proj(aspect);
        let mut layout = LineLayout::default();

        for (line, column) in [(1usize, 6usize), (2, 12), (3, 34), (3, 44), (0, 3)] {
            layout.build(&font, text.lines[line].as_str());
            let advance = layout
                .placements
                .get(column)
                .map(|place| place.advance)
                .unwrap_or(font.advance());
            let world = Vec3::new(
                layout.x_of_column(column) + advance * 0.25,
                -(line as f32) * font.line_height(),
                -(indent_level(text.lines[line].as_str()) as f32) * INDENT_DEPTH,
            );
            let (screen_x, screen_y, w) = to_screen(&view, world);
            assert!(w > 0.0, "point derriere la camera");
            let hit = hit_text(
                &camera,
                aspect,
                VIEW_WIDTH,
                VIEW_HEIGHT,
                screen_x,
                screen_y,
                &text,
                &font,
                &mut layout,
                INDENT_DEPTH,
            )
            .expect("le rayon doit toucher le texte");
            assert_eq!(hit.line, line, "ligne visee {line} colonne {column}");
            assert_eq!(hit.column, column, "colonne visee {line}:{column}");
        }
    }

    #[test]
    fn une_base_degeneree_ne_repond_pas() {
        let aspect = VIEW_WIDTH / VIEW_HEIGHT;
        let camera = Camera::new();
        let plat = Basis {
            origin: Vec3::ZERO,
            right: Vec3::ZERO,
            down: Vec3::ZERO,
            width: 10.0,
            height: 10.0,
        };
        assert!(
            hit_basis(
                &camera,
                aspect,
                VIEW_WIDTH,
                VIEW_HEIGHT,
                VIEW_WIDTH * 0.5,
                VIEW_HEIGHT * 0.5,
                Surface::Tree,
                &plat,
            )
            .is_none()
        );
    }

    fn interior_reference(hub: Vec3, surface: Surface, center: Vec3) -> Vec3 {
        match surface {
            Surface::Code | Surface::Tabs => Vec3::Z,
            _ => (hub - center).normalize_or(Vec3::Z),
        }
    }

    #[test]
    fn les_surfaces_du_hud_tournent_leur_face_vers_l_interieur() {
        let hud = Hud::default();
        let hub = surface_center(&hud.basis(Surface::Code)) + Vec3::Z * crate::hud::ROOM_Z;
        for &surface in hud.surfaces() {
            let basis = hud.basis(surface);
            assert!(
                basis.right.dot(basis.down).abs() < 1.0e-4,
                "{surface:?} doit avoir une base orthogonale"
            );
            assert!(basis.width > 0.0 && basis.height > 0.0, "{surface:?} sans etendue");
            let normal = surface_normal(&basis);
            let center = surface_center(&basis);
            let inward = interior_reference(hub, surface, center);
            assert!(
                normal.dot(inward) > 0.9,
                "{surface:?} doit tourner sa face vers l interieur de la piece"
            );
        }
    }

    #[test]
    fn aller_retour_ecran_surface_par_le_hud_reel() {
        let hud = Hud::default();
        let aspect = VIEW_WIDTH / VIEW_HEIGHT;
        for &surface in hud.surfaces() {
            let basis = hud.basis(surface);
            let camera = orbited_at(&basis);
            for (part_u, part_v) in [(0.2, 0.25), (0.5, 0.5), (0.75, 0.7)] {
                let (u, v) = (basis.width * part_u, basis.height * part_v);
                let world = basis.origin + basis.right * u + basis.down * v;
                let view = camera.view_proj(aspect);
                let (screen_x, screen_y, w) = to_screen(&view, world);
                assert!(w > 0.0, "{surface:?} projete derriere la camera");
                let hit = hit_basis(
                    &camera,
                    aspect,
                    VIEW_WIDTH,
                    VIEW_HEIGHT,
                    screen_x,
                    screen_y,
                    surface,
                    &basis,
                )
                .unwrap_or_else(|| panic!("{surface:?} perd le point local {u} {v}"));
                assert!(
                    (hit.u - u).abs() < 0.01,
                    "{surface:?} u vise {u} retrouve {}",
                    hit.u
                );
                assert!(
                    (hit.v - v).abs() < 0.01,
                    "{surface:?} v vise {v} retrouve {}",
                    hit.v
                );
                let closest = hit_surfaces(
                    &camera,
                    aspect,
                    VIEW_WIDTH,
                    VIEW_HEIGHT,
                    screen_x,
                    screen_y,
                    &hud,
                )
                .unwrap_or_else(|| panic!("{surface:?} laisse passer le rayon"));
                assert!(
                    closest.distance <= hit.distance + 1.0e-3,
                    "la piece doit rendre la surface la plus proche, {:?} a {} contre {surface:?} a {}",
                    closest.surface,
                    closest.distance,
                    hit.distance
                );
                if closest.surface == surface {
                    assert!((closest.u - u).abs() < 0.01 && (closest.v - v).abs() < 0.01);
                }
            }
        }
    }

    #[test]
    fn chaque_surface_se_lit_a_l_endroit_apres_visee() {
        let hud = Hud::default();
        let aspect = VIEW_WIDTH / VIEW_HEIGHT;
        for &surface in hud.surfaces() {
            let basis = hud.basis(surface);
            let mut camera = Camera::new();
            let extent = (basis.height * 0.5).max(basis.width * 0.5 / aspect);
            camera.face(surface_center(&basis), surface_normal(&basis), extent);
            camera.snap();
            assert!(camera.settled(), "{surface:?} doit se poser");
            let view = camera.view_proj(aspect);
            let coin = to_screen(&view, basis.origin);
            let bout_u = to_screen(&view, basis.origin + basis.right * basis.width);
            let bout_v = to_screen(&view, basis.origin + basis.down * basis.height);
            assert!(coin.2 > 0.0 && bout_u.2 > 0.0 && bout_v.2 > 0.0);
            let course_u = bout_u.0 - coin.0;
            let course_v = bout_v.1 - coin.1;
            assert!(
                course_u > VIEW_WIDTH * 0.1,
                "{surface:?} doit se lire de la gauche vers la droite, ecart {course_u}"
            );
            assert!(
                course_u > (bout_u.1 - coin.1).abs(),
                "{surface:?} doit poser ses lignes a l horizontale, derive {}",
                bout_u.1 - coin.1
            );
            assert!(
                course_v > 1.0,
                "{surface:?} doit empiler ses lignes vers le bas, ecart {course_v}"
            );
            assert!(
                course_v > (bout_v.0 - coin.0).abs(),
                "{surface:?} doit empiler sa colonne a la verticale, derive {}",
                bout_v.0 - coin.0
            );
        }
    }
}
