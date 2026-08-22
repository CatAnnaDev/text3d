use glam::Vec3;

use crate::camera::Camera;
use crate::font::Font;
use crate::layout::LineLayout;
use crate::text::TextBuffer;

const PARALLEL_EPSILON: f32 = 1.0e-5;
const LEFT_MARGIN_EM: f32 = 2.0;

pub struct Hit {
    pub line: usize,
    pub column: usize,
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
) -> Option<Hit> {
    let (origin, direction) = ray_from_screen(camera, aspect, width, height, x, y);
    if direction.z.abs() < PARALLEL_EPSILON {
        return None;
    }
    let travel = -origin.z / direction.z;
    if travel <= 0.0 || !travel.is_finite() {
        return None;
    }
    let point = origin + direction * travel;

    let line_height = font.line_height();
    if line_height <= 0.0 {
        return None;
    }
    let row = (-point.y / line_height).round();
    if row < 0.0 {
        return None;
    }
    let line = row as usize;
    if line >= text.line_count() {
        return None;
    }

    if point.x < -LEFT_MARGIN_EM * font.advance() {
        return None;
    }
    layout.build(font, text.lines[line].as_str());
    Some(Hit { line, column: layout.column_at_x(point.x) })
}
