use std::cell::{Cell, OnceCell};

use lyon_tessellation::path::Path;
use lyon_tessellation::path::math::point;
use ttf_parser::gpos::{PairAdjustment, PositioningSubtable};
use ttf_parser::kern::Subtable as KernSubtable;
use ttf_parser::{Face, GlyphId, OutlineBuilder, Tag};

const KERN_FEATURE: Tag = Tag::from_bytes(b"kern");

const FAMILIES: &[(&str, &str, u32)] = &[
    ("SFMono", "/System/Library/Fonts/SFNSMono.ttf", 0),
    ("Menlo", "/System/Library/Fonts/Menlo.ttc", 0),
    ("Menlo", "/Library/Fonts/Menlo.ttc", 0),
    ("Monaco", "/System/Library/Fonts/Monaco.ttf", 0),
    ("Andale Mono", "/System/Library/Fonts/Supplemental/Andale Mono.ttf", 0),
    ("Courier New", "/System/Library/Fonts/Supplemental/Courier New.ttf", 0),
    ("Helvetica", "/System/Library/Fonts/Helvetica.ttc", 0),
    ("SF Pro", "/System/Library/Fonts/SFNS.ttf", 0),
    ("Georgia", "/System/Library/Fonts/Supplemental/Georgia.ttf", 0),
];

const FALLBACKS: &[(&str, u32)] = &[
    ("/System/Library/Fonts/Symbols.ttf", 0),
    ("/System/Library/Fonts/Apple Symbols.ttf", 0),
    ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
    ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
];

const ASCII_CACHE: usize = 128;
const CACHE_UNSET: u32 = 0;
const CACHE_MISSING: u32 = u32::MAX;
const FIRST_PRINTABLE: u32 = 0x20;
const DELETE_CODE: u32 = 0x7f;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlyphKey {
    pub face: u16,
    pub gid: u16,
}

struct FaceData {
    face: Face<'static>,
    units_per_em: f32,
    kern: Vec<KernSubtable<'static>>,
    pairs: Vec<PairAdjustment<'static>>,
}

impl FaceData {
    fn load(path: &str, index: u32) -> Option<FaceData> {
        let bytes = std::fs::read(path).ok()?;
        Face::parse(&bytes, index).ok()?;
        let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let face = Face::parse(data, index).ok()?;
        if !has_outlines(&face) {
            return None;
        }
        let units_per_em = f32::from(face.units_per_em()).max(1.0);
        let kern = collect_kern(&face);
        let pairs = collect_pairs(&face);
        Some(FaceData { face, units_per_em, kern, pairs })
    }

    fn kerning(&self, left: GlyphId, right: GlyphId) -> f32 {
        for subtable in &self.kern {
            if let Some(value) = subtable.glyphs_kerning(left, right)
                && value != 0
            {
                return f32::from(value) / self.units_per_em;
            }
        }
        for pair in &self.pairs {
            if let Some(value) = pair_advance(pair, left, right)
                && value != 0
            {
                return f32::from(value) / self.units_per_em;
            }
        }
        0.0
    }
}

fn has_outlines(face: &Face<'_>) -> bool {
    let tables = face.tables();
    tables.glyf.is_some() || tables.cff.is_some() || tables.cff2.is_some()
}

fn collect_kern(face: &Face<'static>) -> Vec<KernSubtable<'static>> {
    let mut out = Vec::new();
    let Some(table) = face.tables().kern else {
        return out;
    };
    for subtable in table.subtables {
        if subtable.horizontal
            && !subtable.variable
            && !subtable.has_cross_stream
            && !subtable.has_state_machine
        {
            out.push(subtable);
        }
    }
    out
}

fn collect_pairs(face: &Face<'static>) -> Vec<PairAdjustment<'static>> {
    let mut out = Vec::new();
    let Some(gpos) = face.tables().gpos else {
        return out;
    };
    let mut visited: Vec<u16> = Vec::new();
    for feature in gpos.features {
        if feature.tag != KERN_FEATURE {
            continue;
        }
        for lookup_index in feature.lookup_indices {
            if let Err(position) = visited.binary_search(&lookup_index) {
                visited.insert(position, lookup_index);
            } else {
                continue;
            }
            let Some(lookup) = gpos.lookups.get(lookup_index) else {
                continue;
            };
            for subtable in lookup.subtables.into_iter::<PositioningSubtable>() {
                if let PositioningSubtable::Pair(pair) = subtable {
                    out.push(pair);
                }
            }
        }
    }
    out
}

fn pair_advance(pair: &PairAdjustment<'static>, left: GlyphId, right: GlyphId) -> Option<i16> {
    match pair {
        PairAdjustment::Format1 { coverage, sets } => {
            let index = coverage.get(left)?;
            let (first, _) = sets.get(index)?.get(right)?;
            Some(first.x_advance)
        }
        PairAdjustment::Format2 { coverage, classes, matrix } => {
            coverage.get(left)?;
            let pair = (classes.0.get(left), classes.1.get(right));
            let (first, _) = matrix.get(pair)?;
            Some(first.x_advance)
        }
    }
}

struct FaceSlot {
    path: &'static str,
    index: u32,
    data: OnceCell<Option<FaceData>>,
}

impl FaceSlot {
    fn deferred(path: &'static str, index: u32) -> FaceSlot {
        FaceSlot { path, index, data: OnceCell::new() }
    }

    fn ready(path: &'static str, index: u32, data: FaceData) -> FaceSlot {
        let cell = OnceCell::new();
        let _ = cell.set(Some(data));
        FaceSlot { path, index, data: cell }
    }

    fn get(&self) -> Option<&FaceData> {
        self.data
            .get_or_init(|| FaceData::load(self.path, self.index))
            .as_ref()
    }
}

struct Metrics {
    units_per_em: f32,
    line_height: f32,
    advance: f32,
    ascender: f32,
    descender: f32,
    monospace: bool,
}

pub struct Font {
    faces: Vec<FaceSlot>,
    names: Vec<String>,
    slots: Vec<u16>,
    fallback_end: usize,
    family_index: usize,
    units_per_em: f32,
    line_height: f32,
    advance: f32,
    ascender: f32,
    descender: f32,
    monospace: bool,
    ascii: [Cell<u32>; ASCII_CACHE],
}

impl Font {
    pub fn load() -> Result<Font, String> {
        let mut candidates: Vec<(&'static str, &'static str, u32)> = Vec::new();
        for &(name, path, index) in FAMILIES {
            if !std::path::Path::new(path).is_file() {
                continue;
            }
            if candidates.iter().any(|entry| entry.0 == name) {
                continue;
            }
            candidates.push((name, path, index));
        }

        let mut main: Option<(&'static str, &'static str, u32, FaceData)> = None;
        let mut rest: Vec<(&'static str, &'static str, u32)> = Vec::new();
        for &(name, path, index) in &candidates {
            if main.is_some() {
                rest.push((name, path, index));
                continue;
            }
            if let Some(data) = FaceData::load(path, index)
                && has_letter_outline(&data.face)
            {
                main = Some((name, path, index, data));
            }
        }

        let Some((main_name, main_path, main_index, main_data)) = main else {
            return Err(String::from("aucune fonte candidate trouvee"));
        };

        let mut faces = Vec::with_capacity(1 + FALLBACKS.len() + rest.len());
        faces.push(FaceSlot::ready(main_path, main_index, main_data));
        for &(path, index) in FALLBACKS {
            if std::path::Path::new(path).is_file() {
                faces.push(FaceSlot::deferred(path, index));
            }
        }
        let fallback_end = faces.len();

        let mut names = Vec::with_capacity(1 + rest.len());
        let mut slots = Vec::with_capacity(1 + rest.len());
        names.push(String::from(main_name));
        slots.push(0);
        for (name, path, index) in rest {
            faces.push(FaceSlot::deferred(path, index));
            names.push(String::from(name));
            slots.push((faces.len() - 1) as u16);
        }

        let mut font = Font {
            faces,
            names,
            slots,
            fallback_end,
            family_index: 0,
            units_per_em: 1000.0,
            line_height: 1.2,
            advance: 0.6,
            ascender: 0.8,
            descender: -0.2,
            monospace: true,
            ascii: [const { Cell::new(CACHE_UNSET) }; ASCII_CACHE],
        };
        font.refresh_metrics();
        Ok(font)
    }

    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    pub fn advance(&self) -> f32 {
        self.advance
    }

    pub fn ascender(&self) -> f32 {
        self.ascender
    }

    pub fn descender(&self) -> f32 {
        self.descender
    }

    pub fn name(&self) -> &str {
        &self.names[self.family_index]
    }

    pub fn is_monospace(&self) -> bool {
        self.monospace
    }

    pub fn glyph(&self, ch: char) -> Option<GlyphKey> {
        let code = ch as u32;
        if code >= ASCII_CACHE as u32 {
            return self.resolve(ch);
        }
        let cell = &self.ascii[code as usize];
        let cached = cell.get();
        if cached == CACHE_MISSING {
            return None;
        }
        if cached != CACHE_UNSET {
            return Some(GlyphKey { face: ((cached >> 16) - 1) as u16, gid: cached as u16 });
        }
        let resolved = self.resolve(ch);
        cell.set(match resolved {
            Some(key) => ((u32::from(key.face) + 1) << 16) | u32::from(key.gid),
            None => CACHE_MISSING,
        });
        resolved
    }

    pub fn advance_of(&self, key: GlyphKey) -> f32 {
        let Some(data) = self.data(key.face) else {
            return self.advance;
        };
        data.face
            .glyph_hor_advance(GlyphId(key.gid))
            .map(|width| f32::from(width) / data.units_per_em)
            .unwrap_or(self.advance)
    }

    pub fn kerning(&self, left: GlyphKey, right: GlyphKey) -> f32 {
        if left.face != right.face {
            return 0.0;
        }
        let Some(data) = self.data(left.face) else {
            return 0.0;
        };
        data.kerning(GlyphId(left.gid), GlyphId(right.gid))
    }

    pub fn outline(&self, key: GlyphKey) -> Option<(Path, f32)> {
        let data = self.data(key.face)?;
        let mut builder = PathSink::new();
        data.face.outline_glyph(GlyphId(key.gid), &mut builder)?;
        builder.finish().map(|path| (path, data.units_per_em))
    }

    pub fn face_units_per_em(&self, face: u16) -> f32 {
        self.data(face)
            .map(|data| data.units_per_em)
            .unwrap_or(self.units_per_em)
    }

    pub fn next_family(&mut self) -> bool {
        let count = self.names.len();
        if count < 2 {
            return false;
        }
        for step in 1..count {
            let candidate = (self.family_index + step) % count;
            let slot = usize::from(self.slots[candidate]);
            if self.faces[slot].get().is_none() {
                continue;
            }
            let previous = self.family_index;
            self.slots[candidate] = 0;
            self.slots[previous] = slot as u16;
            self.faces.swap(0, slot);
            self.family_index = candidate;
            for cell in &self.ascii {
                cell.set(CACHE_UNSET);
            }
            self.refresh_metrics();
            return true;
        }
        false
    }

    pub fn family_names(&self) -> &[String] {
        &self.names
    }

    pub fn family_index(&self) -> usize {
        self.family_index
    }

    fn data(&self, face: u16) -> Option<&FaceData> {
        self.faces.get(usize::from(face)).and_then(FaceSlot::get)
    }

    fn resolve(&self, ch: char) -> Option<GlyphKey> {
        let code = ch as u32;
        if code < FIRST_PRINTABLE || code == DELETE_CODE {
            return None;
        }
        if let Some(data) = self.faces[0].get()
            && let Some(gid) = data.face.glyph_index(ch)
        {
            return Some(GlyphKey { face: 0, gid: gid.0 });
        }
        for slot in 1..self.fallback_end {
            let Some(data) = self.faces[slot].get() else {
                continue;
            };
            if let Some(gid) = data.face.glyph_index(ch) {
                return Some(GlyphKey { face: slot as u16, gid: gid.0 });
            }
        }
        None
    }

    fn refresh_metrics(&mut self) {
        let Some(metrics) = self.faces[0].get().map(measure) else {
            return;
        };
        self.units_per_em = metrics.units_per_em;
        self.line_height = metrics.line_height;
        self.advance = metrics.advance;
        self.ascender = metrics.ascender;
        self.descender = metrics.descender;
        self.monospace = metrics.monospace;
    }
}

fn measure(data: &FaceData) -> Metrics {
    let face = &data.face;
    let units_per_em = data.units_per_em;
    let ascender = f32::from(face.ascender()) / units_per_em;
    let descender = f32::from(face.descender()) / units_per_em;
    let gap = f32::from(face.line_gap()) / units_per_em;
    let line_height = (ascender - descender + gap).max(1.0);
    let advance = face
        .glyph_index('M')
        .and_then(|gid| face.glyph_hor_advance(gid))
        .map(|width| f32::from(width) / units_per_em)
        .unwrap_or(0.6);
    let monospace = face.is_monospaced() || uniform_advances(face, units_per_em, advance);
    Metrics { units_per_em, line_height, advance, ascender, descender, monospace }
}

fn uniform_advances(face: &Face<'_>, units_per_em: f32, advance: f32) -> bool {
    let mut measured = false;
    for ch in 'A'..='z' {
        let Some(gid) = face.glyph_index(ch) else {
            continue;
        };
        let Some(width) = face.glyph_hor_advance(gid) else {
            continue;
        };
        if (f32::from(width) / units_per_em - advance).abs() > 1e-3 {
            return false;
        }
        measured = true;
    }
    measured
}

fn has_letter_outline(face: &Face<'_>) -> bool {
    let Some(gid) = face.glyph_index('A') else {
        return false;
    };
    let mut probe = Probe::default();
    face.outline_glyph(gid, &mut probe).is_some() && probe.any
}

#[derive(Default)]
struct Probe {
    any: bool,
}

impl OutlineBuilder for Probe {
    fn move_to(&mut self, _x: f32, _y: f32) {
        self.any = true;
    }
    fn line_to(&mut self, _x: f32, _y: f32) {
        self.any = true;
    }
    fn quad_to(&mut self, _x1: f32, _y1: f32, _x: f32, _y: f32) {
        self.any = true;
    }
    fn curve_to(&mut self, _x1: f32, _y1: f32, _x2: f32, _y2: f32, _x: f32, _y: f32) {
        self.any = true;
    }
    fn close(&mut self) {}
}

struct PathSink {
    builder: lyon_tessellation::path::path::Builder,
    open: bool,
    empty: bool,
}

impl PathSink {
    fn new() -> Self {
        PathSink { builder: Path::builder(), open: false, empty: true }
    }

    fn finish(mut self) -> Option<Path> {
        if self.open {
            self.builder.end(true);
        }
        if self.empty { None } else { Some(self.builder.build()) }
    }
}

impl OutlineBuilder for PathSink {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.open {
            self.builder.end(true);
        }
        self.builder.begin(point(x, y));
        self.open = true;
        self.empty = false;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        if self.open {
            self.builder.line_to(point(x, y));
        }
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        if self.open {
            self.builder.quadratic_bezier_to(point(x1, y1), point(x, y));
        }
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        if self.open {
            self.builder
                .cubic_bezier_to(point(x1, y1), point(x2, y2), point(x, y));
        }
    }

    fn close(&mut self) {
        if self.open {
            self.builder.end(true);
            self.open = false;
        }
    }
}
