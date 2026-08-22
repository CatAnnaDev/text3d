use glam::Vec3;

use crate::camera::Camera;
use crate::font::Font;
use crate::layout::LineLayout;
use crate::render::indent_level;
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
