use bytemuck::{Pod, Zeroable};
use lyon_tessellation::path::Path;
use lyon_tessellation::path::iterator::PathIterator;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers,
};

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

pub fn extrude_glyph(path: &Path, scale: f32, tolerance: f32, half_depth: f32) -> MeshData {
    let mut mesh = MeshData::default();
    build_caps(path, scale, tolerance, half_depth, &mut mesh);
    build_walls(path, scale, tolerance, half_depth, &mut mesh);
    mesh
}

fn build_caps(path: &Path, scale: f32, tolerance: f32, half_depth: f32, mesh: &mut MeshData) {
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let options = FillOptions::default()
        .with_fill_rule(FillRule::NonZero)
        .with_tolerance(tolerance);
    let mut tessellator = FillTessellator::new();
    let ok = tessellator
        .tessellate_path(
            path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| {
                let p = v.position();
                [p.x, p.y]
            }),
        )
        .is_ok();
    if !ok || buffers.indices.is_empty() {
        return;
    }

    let front_base = mesh.vertices.len() as u32;
    for p in &buffers.vertices {
        mesh.vertices.push(Vertex {
            position: [p[0] * scale, p[1] * scale, half_depth],
            normal: [0.0, 0.0, 1.0],
        });
    }
    let back_base = mesh.vertices.len() as u32;
    for p in &buffers.vertices {
        mesh.vertices.push(Vertex {
            position: [p[0] * scale, p[1] * scale, -half_depth],
            normal: [0.0, 0.0, -1.0],
        });
    }

    mesh.indices.reserve(buffers.indices.len() * 2);
    for &[a, b, c] in buffers.indices.as_chunks::<3>().0 {
        let (pa, pb, pc) = (
            buffers.vertices[a as usize],
            buffers.vertices[b as usize],
            buffers.vertices[c as usize],
        );
        let area = (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pb[1] - pa[1]) * (pc[0] - pa[0]);
        let (a, b, c) = if area < 0.0 { (a, c, b) } else { (a, b, c) };
        mesh.indices
            .extend_from_slice(&[front_base + a, front_base + b, front_base + c]);
        mesh.indices
            .extend_from_slice(&[back_base + a, back_base + c, back_base + b]);
    }
}

fn build_walls(path: &Path, scale: f32, tolerance: f32, half_depth: f32, mesh: &mut MeshData) {
    let mut contours: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();

    for event in path.iter().flattened(tolerance) {
        use lyon_tessellation::path::PathEvent;
        match event {
            PathEvent::Begin { at } => {
                current.clear();
                current.push([at.x, at.y]);
            }
            PathEvent::Line { to, .. } => current.push([to.x, to.y]),
            PathEvent::End { .. } => {
                if current.len() >= 3 {
                    contours.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
            _ => {}
        }
    }

    let total_area: f32 = contours.iter().map(|c| signed_area(c)).sum();
    let orient = if total_area < 0.0 { -1.0f32 } else { 1.0f32 };

    for contour in &contours {
        let n = contour.len();
        for i in 0..n {
            let p0 = contour[i];
            let p1 = contour[(i + 1) % n];
            let dx = p1[0] - p0[0];
            let dy = p1[1] - p0[1];
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                continue;
            }
            let normal = [orient * dy / len, orient * -dx / len, 0.0];
            let base = mesh.vertices.len() as u32;
            let (a, b) = if orient > 0.0 { (p0, p1) } else { (p1, p0) };
            for position in [
                [a[0] * scale, a[1] * scale, half_depth],
                [a[0] * scale, a[1] * scale, -half_depth],
                [b[0] * scale, b[1] * scale, -half_depth],
                [b[0] * scale, b[1] * scale, half_depth],
            ] {
                mesh.vertices.push(Vertex { position, normal });
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
}

fn signed_area(contour: &[[f32; 2]]) -> f32 {
    let mut acc = 0.0;
    for i in 0..contour.len() {
        let p0 = contour[i];
        let p1 = contour[(i + 1) % contour.len()];
        acc += p0[0] * p1[1] - p1[0] * p0[1];
    }
    acc * 0.5
}
