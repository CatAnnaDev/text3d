use bytemuck::{Pod, Zeroable};
use lyon_tessellation::math::point;
use lyon_tessellation::path::iterator::PathIterator;
use lyon_tessellation::path::{Path, PathEvent};
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
};

pub const DEFAULT_HALF_DEPTH: f32 = 0.13;
pub const DEFAULT_BEVEL_EM: f32 = 0.018;
pub const DEFAULT_SMOOTH_ANGLE: f32 = 0.6;

const WELD_EPSILON: f32 = 1e-6;
const MITER_LIMIT_RATIO: f32 = 2.5;
const MIN_CONTOUR_AREA_RATIO: f32 = 0.15;
const MIN_GLYPH_AREA_RATIO: f32 = 0.35;
const BEVEL_DEPTH_RATIO: f32 = 0.75;
const BEVEL_STEPS: [f32; 3] = [1.0, 0.5, 0.25];
const BISECTOR_EPSILON: f32 = 1e-4;
const MITER_COSINE_FLOOR: f32 = 1e-3;
const FRAC_1_SQRT_2: f32 = std::f32::consts::FRAC_1_SQRT_2;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

#[derive(Default)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }

    pub fn push_box(&mut self, min: [f32; 3], max: [f32; 3]) {
        const FACES: [([f32; 3], [usize; 4]); 6] = [
            ([0.0, 0.0, 1.0], [0, 1, 3, 2]),
            ([0.0, 0.0, -1.0], [5, 4, 6, 7]),
            ([1.0, 0.0, 0.0], [1, 5, 7, 3]),
            ([-1.0, 0.0, 0.0], [4, 0, 2, 6]),
            ([0.0, 1.0, 0.0], [2, 3, 7, 6]),
            ([0.0, -1.0, 0.0], [4, 5, 1, 0]),
        ];
        let corner = |i: usize| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { max[2] } else { min[2] },
            ]
        };
        self.vertices.reserve(24);
        self.indices.reserve(36);
        for (normal, quad) in FACES {
            let base = self.vertices.len() as u32;
            for &c in &quad {
                self.vertices.push(Vertex { position: corner(c), normal });
            }
            self.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
}

#[derive(Clone, Copy)]
pub struct ExtrudeParams {
    pub scale: f32,
    pub tolerance: f32,
    pub half_depth: f32,
    pub bevel: f32,
    pub smooth_angle: f32,
}

impl ExtrudeParams {
    pub fn new(scale: f32, tolerance: f32) -> ExtrudeParams {
        ExtrudeParams {
            scale,
            tolerance,
            half_depth: DEFAULT_HALF_DEPTH,
            bevel: if scale > 0.0 { DEFAULT_BEVEL_EM / scale } else { 0.0 },
            smooth_angle: DEFAULT_SMOOTH_ANGLE,
        }
    }
}

#[derive(Clone, Copy)]
struct Contour {
    start: u32,
    len: u32,
    area: f32,
    bevel: f32,
}

#[derive(Clone, Copy)]
struct RingEntry {
    point: u32,
    normal: [f32; 2],
}

pub struct Extruder {
    tessellator: FillTessellator,
    fill: VertexBuffers<[f32; 2], u32>,
    events: Vec<PathEvent>,
    points: Vec<[f32; 2]>,
    offsets: Vec<[f32; 2]>,
    normals: Vec<[f32; 2]>,
    contours: Vec<Contour>,
    ring: Vec<RingEntry>,
    slot_in: Vec<u32>,
    slot_out: Vec<u32>,
}

impl Default for Extruder {
    fn default() -> Extruder {
        Extruder::new()
    }
}

impl Extruder {
    pub fn new() -> Extruder {
        Extruder {
            tessellator: FillTessellator::new(),
            fill: VertexBuffers::new(),
            events: Vec::new(),
            points: Vec::new(),
            offsets: Vec::new(),
            normals: Vec::new(),
            contours: Vec::new(),
            ring: Vec::new(),
            slot_in: Vec::new(),
            slot_out: Vec::new(),
        }
    }

    pub fn glyph(&mut self, path: &Path, params: &ExtrudeParams, out: &mut MeshData) {
        self.flatten(path, params.tolerance);
        if self.contours.is_empty() {
            return;
        }

        let mut total_area = 0.0;
        for contour in &self.contours {
            total_area += contour.area;
        }
        let orient = if total_area < 0.0 { -1.0f32 } else { 1.0f32 };

        self.build_normals(orient);

        let bevel = if params.bevel > 0.0 && params.scale > 0.0 {
            params
                .bevel
                .min(params.half_depth * BEVEL_DEPTH_RATIO / params.scale)
        } else {
            0.0
        };
        self.build_offsets(bevel, total_area);

        self.tessellate_caps(params.tolerance);
        self.push_caps(params.scale, params.half_depth, out);
        self.push_shells(params, orient, out);
    }

    pub fn bounds(&mut self, path: &Path, tolerance: f32) -> Option<[f32; 4]> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for event in path.iter().flattened(tolerance) {
            let p = match event {
                PathEvent::Begin { at } => at,
                PathEvent::Line { to, .. } => to,
                _ => continue,
            };
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
        if min_x > max_x || min_y > max_y {
            return None;
        }
        Some([min_x, min_y, max_x, max_y])
    }

    fn flatten(&mut self, path: &Path, tolerance: f32) {
        self.points.clear();
        self.contours.clear();
        let mut start = 0usize;
        for event in path.iter().flattened(tolerance) {
            match event {
                PathEvent::Begin { at } => {
                    start = self.points.len();
                    self.points.push([at.x, at.y]);
                }
                PathEvent::Line { to, .. } => {
                    let candidate = [to.x, to.y];
                    if self.points.len() > start
                        && near(self.points[self.points.len() - 1], candidate)
                    {
                        continue;
                    }
                    self.points.push(candidate);
                }
                PathEvent::End { .. } => {
                    while self.points.len() > start + 1
                        && near(self.points[self.points.len() - 1], self.points[start])
                    {
                        self.points.pop();
                    }
                    let len = self.points.len() - start;
                    if len >= 3 {
                        let area = signed_area(&self.points[start..start + len]);
                        self.contours.push(Contour {
                            start: start as u32,
                            len: len as u32,
                            area,
                            bevel: 0.0,
                        });
                    } else {
                        self.points.truncate(start);
                    }
                }
                _ => {}
            }
        }
    }

    fn build_normals(&mut self, orient: f32) {
        self.normals.clear();
        self.normals.resize(self.points.len(), [0.0, 0.0]);
        for contour in &self.contours {
            let start = contour.start as usize;
            let len = contour.len as usize;
            for i in 0..len {
                let p0 = self.points[start + i];
                let p1 = self.points[start + (i + 1) % len];
                let dx = p1[0] - p0[0];
                let dy = p1[1] - p0[1];
                let length = (dx * dx + dy * dy).sqrt();
                self.normals[start + i] = if length > 0.0 {
                    [orient * dy / length, orient * -dx / length]
                } else {
                    [0.0, 0.0]
                };
            }
        }
    }

    fn build_offsets(&mut self, bevel: f32, total_area: f32) {
        self.offsets.clear();
        self.offsets.resize(self.points.len(), [0.0, 0.0]);
        if bevel > 0.0 {
            let floor = total_area.abs() * MIN_GLYPH_AREA_RATIO;
            for &step in &BEVEL_STEPS {
                if self.offset_pass(bevel * step).abs() >= floor {
                    return;
                }
            }
        }
        self.offset_pass(0.0);
    }

    fn offset_pass(&mut self, bevel: f32) -> f32 {
        let Extruder { points, offsets, normals, contours, .. } = self;
        let mut total = 0.0;
        for contour in contours.iter_mut() {
            let start = contour.start as usize;
            let len = contour.len as usize;
            let src = &points[start..start + len];
            let edge = &normals[start..start + len];
            let dst = &mut offsets[start..start + len];
            contour.bevel = 0.0;
            if bevel > 0.0 {
                for &step in &BEVEL_STEPS {
                    let candidate = bevel * step;
                    offset_contour(src, edge, candidate, dst);
                    if offset_is_sane(src, dst, contour.area) {
                        contour.bevel = candidate;
                        break;
                    }
                }
            }
            if contour.bevel == 0.0 {
                dst.copy_from_slice(src);
                total += contour.area;
            } else {
                total += signed_area(dst);
            }
        }
        total
    }

    fn tessellate_caps(&mut self, tolerance: f32) {
        let Extruder { tessellator, fill, events, offsets, contours, .. } = self;
        events.clear();
        for contour in contours.iter() {
            let start = contour.start as usize;
            let len = contour.len as usize;
            let ring = &offsets[start..start + len];
            let first = point(ring[0][0], ring[0][1]);
            events.push(PathEvent::Begin { at: first });
            let mut from = first;
            for p in &ring[1..] {
                let to = point(p[0], p[1]);
                events.push(PathEvent::Line { from, to });
                from = to;
            }
            events.push(PathEvent::End { last: from, first, close: true });
        }
        fill.vertices.clear();
        fill.indices.clear();
        let options = FillOptions::default()
            .with_fill_rule(FillRule::NonZero)
            .with_tolerance(tolerance);
        let failed = {
            let mut builder = BuffersBuilder::new(&mut *fill, |v: FillVertex| {
                let p = v.position();
                [p.x, p.y]
            });
            tessellator
                .tessellate(events.iter().copied(), &options, &mut builder)
                .is_err()
        };
        if failed {
            fill.vertices.clear();
            fill.indices.clear();
        }
    }

    fn push_caps(&self, scale: f32, half_depth: f32, out: &mut MeshData) {
        let flat = &self.fill.vertices;
        if self.fill.indices.is_empty() {
            return;
        }
        out.vertices.reserve(flat.len() * 2);
        out.indices.reserve(self.fill.indices.len() * 2);
        let front_base = out.vertices.len() as u32;
        for p in flat {
            out.vertices.push(Vertex {
                position: [p[0] * scale, p[1] * scale, half_depth],
                normal: [0.0, 0.0, 1.0],
            });
        }
        let back_base = out.vertices.len() as u32;
        for p in flat {
            out.vertices.push(Vertex {
                position: [p[0] * scale, p[1] * scale, -half_depth],
                normal: [0.0, 0.0, -1.0],
            });
        }
        for &[a, b, c] in self.fill.indices.as_chunks::<3>().0 {
            let pa = flat[a as usize];
            let pb = flat[b as usize];
            let pc = flat[c as usize];
            let area = (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pb[1] - pa[1]) * (pc[0] - pa[0]);
            let (b, c) = if area < 0.0 { (c, b) } else { (b, c) };
            out.indices
                .extend_from_slice(&[front_base + a, front_base + b, front_base + c]);
            out.indices
                .extend_from_slice(&[back_base + a, back_base + c, back_base + b]);
        }
    }

    fn push_shells(&mut self, params: &ExtrudeParams, orient: f32, out: &mut MeshData) {
        let scale = params.scale;
        let half_depth = params.half_depth;
        let cos_threshold = params.smooth_angle.cos();
        let flip = orient < 0.0;
        out.vertices.reserve(self.points.len() * 6);
        out.indices.reserve(self.points.len() * 18);

        let Extruder { points, offsets, normals, contours, ring, slot_in, slot_out, .. } = self;
        for contour in contours.iter() {
            let start = contour.start as usize;
            let len = contour.len as usize;
            let src = &points[start..start + len];
            let off = &offsets[start..start + len];
            let edge = &normals[start..start + len];
            build_ring(edge, cos_threshold, ring, slot_in, slot_out);

            let bevel_z = contour.bevel * scale;
            let wall_z = half_depth - bevel_z;
            let banded = bevel_z > 0.0;

            let wall_base = out.vertices.len() as u32;
            for entry in ring.iter() {
                let p = src[entry.point as usize];
                let normal = [entry.normal[0], entry.normal[1], 0.0];
                let x = p[0] * scale;
                let y = p[1] * scale;
                out.vertices.push(Vertex { position: [x, y, wall_z], normal });
                out.vertices.push(Vertex { position: [x, y, -wall_z], normal });
            }

            let band_base = out.vertices.len() as u32;
            if banded {
                for entry in ring.iter() {
                    let p = src[entry.point as usize];
                    let o = off[entry.point as usize];
                    let front = [
                        entry.normal[0] * FRAC_1_SQRT_2,
                        entry.normal[1] * FRAC_1_SQRT_2,
                        FRAC_1_SQRT_2,
                    ];
                    let back = [front[0], front[1], -FRAC_1_SQRT_2];
                    let (ox, oy) = (o[0] * scale, o[1] * scale);
                    let (px, py) = (p[0] * scale, p[1] * scale);
                    out.vertices.push(Vertex { position: [ox, oy, half_depth], normal: front });
                    out.vertices.push(Vertex { position: [px, py, wall_z], normal: front });
                    out.vertices.push(Vertex { position: [ox, oy, -half_depth], normal: back });
                    out.vertices.push(Vertex { position: [px, py, -wall_z], normal: back });
                }
            }

            for i in 0..len {
                let a = slot_out[i];
                let b = slot_in[(i + 1) % len];
                let wa = wall_base + a * 2;
                let wb = wall_base + b * 2;
                push_quad(out, wa, wa + 1, wb + 1, wb, flip);
                if banded {
                    let ba = band_base + a * 4;
                    let bb = band_base + b * 4;
                    push_quad(out, ba, ba + 1, bb + 1, bb, flip);
                    push_quad(out, ba + 2, ba + 3, bb + 3, bb + 2, !flip);
                }
            }
        }
    }
}

fn build_ring(
    edge: &[[f32; 2]],
    cos_threshold: f32,
    ring: &mut Vec<RingEntry>,
    slot_in: &mut Vec<u32>,
    slot_out: &mut Vec<u32>,
) {
    ring.clear();
    slot_in.clear();
    slot_out.clear();
    let len = edge.len();
    for i in 0..len {
        let previous = edge[(i + len - 1) % len];
        let current = edge[i];
        let alignment = previous[0] * current[0] + previous[1] * current[1];
        let slot = ring.len() as u32;
        if alignment >= cos_threshold {
            let mut normal = [previous[0] + current[0], previous[1] + current[1]];
            normalize(&mut normal, current);
            slot_in.push(slot);
            slot_out.push(slot);
            ring.push(RingEntry { point: i as u32, normal });
        } else {
            slot_in.push(slot);
            ring.push(RingEntry { point: i as u32, normal: previous });
            slot_out.push(slot + 1);
            ring.push(RingEntry { point: i as u32, normal: current });
        }
    }
}

fn offset_contour(src: &[[f32; 2]], edge: &[[f32; 2]], bevel: f32, dst: &mut [[f32; 2]]) {
    let len = src.len();
    let miter_limit = bevel * MITER_LIMIT_RATIO;
    for i in 0..len {
        let previous = edge[(i + len - 1) % len];
        let current = edge[i];
        let inward = [-current[0], -current[1]];
        let mut bisector = [-(previous[0] + current[0]), -(previous[1] + current[1])];
        let length = (bisector[0] * bisector[0] + bisector[1] * bisector[1]).sqrt();
        let step = if length > BISECTOR_EPSILON {
            bisector[0] /= length;
            bisector[1] /= length;
            let cosine = bisector[0] * inward[0] + bisector[1] * inward[1];
            if cosine > MITER_COSINE_FLOOR { (bevel / cosine).min(miter_limit) } else { miter_limit }
        } else {
            bisector = inward;
            bevel
        };
        dst[i] = [src[i][0] + bisector[0] * step, src[i][1] + bisector[1] * step];
    }
}

fn offset_is_sane(src: &[[f32; 2]], dst: &[[f32; 2]], area: f32) -> bool {
    let offset_area = signed_area(dst);
    if offset_area * area <= 0.0 {
        return false;
    }
    if offset_area.abs() < area.abs() * MIN_CONTOUR_AREA_RATIO {
        return false;
    }
    let len = src.len();
    for i in 0..len {
        let j = (i + 1) % len;
        let ox = src[j][0] - src[i][0];
        let oy = src[j][1] - src[i][1];
        let nx = dst[j][0] - dst[i][0];
        let ny = dst[j][1] - dst[i][1];
        if ox * nx + oy * ny < 0.0 {
            return false;
        }
    }
    true
}

fn push_quad(out: &mut MeshData, a: u32, b: u32, c: u32, d: u32, flip: bool) {
    if flip {
        out.indices.extend_from_slice(&[a, d, c, a, c, b]);
    } else {
        out.indices.extend_from_slice(&[a, b, c, a, c, d]);
    }
}

fn normalize(value: &mut [f32; 2], fallback: [f32; 2]) {
    let length = (value[0] * value[0] + value[1] * value[1]).sqrt();
    if length > WELD_EPSILON {
        value[0] /= length;
        value[1] /= length;
    } else {
        *value = fallback;
    }
}

fn near(a: [f32; 2], b: [f32; 2]) -> bool {
    (a[0] - b[0]).abs() < WELD_EPSILON && (a[1] - b[1]).abs() < WELD_EPSILON
}

fn signed_area(contour: &[[f32; 2]]) -> f32 {
    let len = contour.len();
    let mut acc = 0.0;
    for i in 0..len {
        let p0 = contour[i];
        let p1 = contour[(i + 1) % len];
        acc += p0[0] * p1[1] - p1[0] * p0[1];
    }
    acc * 0.5
}
