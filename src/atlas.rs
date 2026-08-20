use std::collections::HashMap;

use crate::extrude::{MeshData, Vertex, extrude_glyph};
use crate::font::Font;

pub const UNSET: u32 = u32::MAX;
pub const BLANK: u32 = u32::MAX - 1;

const GLYPH_HALF_DEPTH: f32 = 0.13;
const TOLERANCE_EM: f32 = 0.006;
const CURSOR_HALF_WIDTH: f32 = 0.045;

pub struct Slot {
    pub index_start: u32,
    pub index_count: u32,
    pub base_vertex: i32,
}

pub struct GlyphAtlas {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub slots: Vec<Slot>,
    pub cursor_slot: u32,
    pub dirty: bool,
    ascii: [u32; 128],
    wide: HashMap<char, u32>,
    scale: f32,
    tolerance: f32,
}

impl GlyphAtlas {
    pub fn new(font: &Font) -> Self {
        let units = font.units_per_em();
        let mut atlas = GlyphAtlas {
            vertices: Vec::new(),
            indices: Vec::new(),
            slots: Vec::new(),
            cursor_slot: 0,
            dirty: true,
            ascii: [UNSET; 128],
            wide: HashMap::new(),
            scale: 1.0 / units,
            tolerance: units * TOLERANCE_EM,
        };

        let mut cursor = MeshData::default();
        cursor.push_box(
            [-CURSOR_HALF_WIDTH, font.descender() * 0.8, -GLYPH_HALF_DEPTH * 1.6],
            [CURSOR_HALF_WIDTH, font.ascender() * 0.82, GLYPH_HALF_DEPTH * 1.6],
        );
        atlas.cursor_slot = atlas.push_mesh(cursor);
        atlas
    }

    pub fn slot_for(&mut self, font: &Font, ch: char) -> u32 {
        let code = ch as u32;
        if code < 128 {
            let cached = self.ascii[code as usize];
            if cached != UNSET {
                return cached;
            }
            let slot = self.build(font, ch);
            self.ascii[code as usize] = slot;
            return slot;
        }
        if let Some(&cached) = self.wide.get(&ch) {
            return cached;
        }
        let slot = self.build(font, ch);
        self.wide.insert(ch, slot);
        slot
    }

    fn build(&mut self, font: &Font, ch: char) -> u32 {
        let gid = font.glyph_id(ch);
        let Some(path) = font.outline(gid) else {
            return BLANK;
        };
        let mesh = extrude_glyph(&path, self.scale, self.tolerance, GLYPH_HALF_DEPTH);
        if mesh.is_empty() {
            return BLANK;
        }
        self.push_mesh(mesh)
    }

    fn push_mesh(&mut self, mesh: MeshData) -> u32 {
        let slot = self.slots.len() as u32;
        self.slots.push(Slot {
            index_start: self.indices.len() as u32,
            index_count: mesh.indices.len() as u32,
            base_vertex: self.vertices.len() as i32,
        });
        self.vertices.extend_from_slice(&mesh.vertices);
        self.indices.extend_from_slice(&mesh.indices);
        self.dirty = true;
        slot
    }
}
