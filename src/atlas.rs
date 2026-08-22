use std::collections::HashMap;

use crate::extrude::{DEFAULT_HALF_DEPTH, ExtrudeParams, Extruder, MeshData, Vertex};
use crate::font::{Font, GlyphKey};

pub const UNSET: u32 = u32::MAX;
pub const BLANK: u32 = u32::MAX - 1;

pub const LOD_NEAR: u8 = 0;
pub const LOD_MID: u8 = 1;
pub const LOD_FAR: u8 = 2;
pub const LOD_COUNT: usize = 3;

const NEAR_TOLERANCE_EM: f32 = 0.004;
const MID_TOLERANCE_EM: f32 = 0.014;
const CURSOR_HALF_WIDTH: f32 = 0.045;
const CURSOR_DEPTH_RATIO: f32 = 1.6;
const QUAD_HALF_DEPTH: f32 = 0.004;
const DIRECT_GIDS: usize = 512;
const MIN_INK: f32 = 1e-5;

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
    direct: Box<[u32]>,
    wide: HashMap<(GlyphKey, u8), u32>,
    extruder: Extruder,
    scratch: MeshData,
    quad_slot: u32,
    bevel: bool,
}

impl GlyphAtlas {
    pub fn new(font: &Font) -> GlyphAtlas {
        let mut atlas = GlyphAtlas {
            vertices: Vec::new(),
            indices: Vec::new(),
            slots: Vec::new(),
            cursor_slot: 0,
            dirty: true,
            direct: vec![UNSET; LOD_COUNT * DIRECT_GIDS].into_boxed_slice(),
            wide: HashMap::new(),
            extruder: Extruder::new(),
            scratch: MeshData::default(),
            quad_slot: 0,
            bevel: true,
        };
        atlas.build_fixed(font);
        atlas
    }

    pub fn slot_for(&mut self, font: &Font, key: GlyphKey, lod: u8) -> u32 {
        let lod = lod.min(LOD_FAR);
        let gid = key.gid as usize;
        if key.face == 0 && gid < DIRECT_GIDS {
            let index = lod as usize * DIRECT_GIDS + gid;
            let cached = self.direct[index];
            if cached != UNSET {
                return cached;
            }
            let slot = self.build(font, key, lod);
            self.direct[index] = slot;
            return slot;
        }
        if let Some(&cached) = self.wide.get(&(key, lod)) {
            return cached;
        }
        let slot = self.build(font, key, lod);
        self.wide.insert((key, lod), slot);
        slot
    }

    pub fn reset(&mut self, font: &Font) {
        self.vertices.clear();
        self.indices.clear();
        self.slots.clear();
        self.direct.fill(UNSET);
        self.wide.clear();
        self.build_fixed(font);
        self.dirty = true;
    }

    pub fn bevel_enabled(&self) -> bool {
        self.bevel
    }

    pub fn set_bevel(&mut self, enabled: bool, font: &Font) {
        if self.bevel == enabled {
            return;
        }
        self.bevel = enabled;
        self.reset(font);
    }

    pub fn quad_slot(&self) -> u32 {
        self.quad_slot
    }

    pub fn memory_bytes(&self) -> usize {
        self.vertices.len() * size_of::<Vertex>()
            + self.indices.len() * size_of::<u32>()
            + self.slots.len() * size_of::<Slot>()
            + self.direct.len() * size_of::<u32>()
            + self.wide.capacity() * size_of::<((GlyphKey, u8), u32)>()
    }

    fn build_fixed(&mut self, font: &Font) {
        let depth = DEFAULT_HALF_DEPTH * CURSOR_DEPTH_RATIO;
        self.scratch.clear();
        self.scratch.push_box(
            [-CURSOR_HALF_WIDTH, font.descender() * 0.8, -depth],
            [CURSOR_HALF_WIDTH, font.ascender() * 0.82, depth],
        );
        self.cursor_slot = self.push_scratch();

        self.scratch.clear();
        self.scratch.push_box(
            [0.0, 0.0, -QUAD_HALF_DEPTH],
            [1.0, font.line_height(), QUAD_HALF_DEPTH],
        );
        self.quad_slot = self.push_scratch();
    }

    fn build(&mut self, font: &Font, key: GlyphKey, lod: u8) -> u32 {
        let Some((path, units_per_em)) = font.outline(key) else {
            return BLANK;
        };
        if units_per_em <= 0.0 {
            return BLANK;
        }
        let scale = 1.0 / units_per_em;
        self.scratch.clear();
        if lod == LOD_FAR {
            let tolerance = units_per_em * MID_TOLERANCE_EM;
            let Some(ink) = self.extruder.bounds(&path, tolerance) else {
                return BLANK;
            };
            if ink[2] - ink[0] < MIN_INK || ink[3] - ink[1] < MIN_INK {
                return BLANK;
            }
            self.scratch.push_box(
                [ink[0] * scale, ink[1] * scale, -DEFAULT_HALF_DEPTH],
                [ink[2] * scale, ink[3] * scale, DEFAULT_HALF_DEPTH],
            );
            return self.push_scratch();
        }
        let tolerance_em = if lod == LOD_NEAR { NEAR_TOLERANCE_EM } else { MID_TOLERANCE_EM };
        let mut params = ExtrudeParams::new(scale, units_per_em * tolerance_em);
        if lod != LOD_NEAR || !self.bevel {
            params.bevel = 0.0;
        }
        self.extruder.glyph(&path, &params, &mut self.scratch);
        if self.scratch.is_empty() {
            return BLANK;
        }
        self.push_scratch()
    }

    fn push_scratch(&mut self) -> u32 {
        let slot = self.slots.len() as u32;
        self.slots.push(Slot {
            index_start: self.indices.len() as u32,
            index_count: self.scratch.indices.len() as u32,
            base_vertex: self.vertices.len() as i32,
        });
        self.vertices.extend_from_slice(&self.scratch.vertices);
        self.indices.extend_from_slice(&self.scratch.indices);
        self.dirty = true;
        slot
    }
}
