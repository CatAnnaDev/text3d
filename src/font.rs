use lyon_tessellation::path::Path;
use lyon_tessellation::path::math::point;
use ttf_parser::{Face, GlyphId, OutlineBuilder};

const CANDIDATES: &[(&str, u32)] = &[
    ("/System/Library/Fonts/SFNSMono.ttf", 0),
    ("/System/Library/Fonts/Menlo.ttc", 0),
    ("/System/Library/Fonts/Supplemental/Andale Mono.ttf", 0),
    ("/System/Library/Fonts/Supplemental/Courier New.ttf", 0),
    ("/Library/Fonts/Menlo.ttc", 0),
];

pub struct Font {
    face: Face<'static>,
    units_per_em: f32,
    line_height: f32,
    advance: f32,
    ascender: f32,
    descender: f32,
}

impl Font {
    pub fn load() -> Result<Font, String> {
        let mut last = String::from("no candidate font found");
        for &(path, index) in CANDIDATES {
            match Self::try_load(path, index) {
                Ok(font) => return Ok(font),
                Err(err) => last = format!("{path}: {err}"),
            }
        }
        Err(last)
    }

    fn try_load(path: &str, index: u32) -> Result<Font, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let face = Face::parse(data, index).map_err(|e| e.to_string())?;

        let probe = face
            .glyph_index('A')
            .ok_or_else(|| String::from("no glyph for 'A'"))?;
        let mut sink = Probe::default();
        if face.outline_glyph(probe, &mut sink).is_none() || !sink.any {
            return Err(String::from("glyph outlines unavailable"));
        }

        let units_per_em = f32::from(face.units_per_em());
        let ascender = f32::from(face.ascender()) / units_per_em;
        let descender = f32::from(face.descender()) / units_per_em;
        let gap = f32::from(face.line_gap()) / units_per_em;
        let line_height = (ascender - descender + gap).max(1.0);
        let advance = face
            .glyph_index('M')
            .and_then(|gid| face.glyph_hor_advance(gid))
            .map(|a| f32::from(a) / units_per_em)
            .unwrap_or(0.6);

        Ok(Font { face, units_per_em, line_height, advance, ascender, descender })
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

    pub fn glyph_id(&self, ch: char) -> u16 {
        self.face.glyph_index(ch).map(|g| g.0).unwrap_or(0)
    }

    pub fn outline(&self, gid: u16) -> Option<Path> {
        let mut builder = PathSink::new();
        self.face.outline_glyph(GlyphId(gid), &mut builder)?;
        builder.finish()
    }
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
