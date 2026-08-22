use crate::font::{Font, GlyphKey};

const MISSING: GlyphKey = GlyphKey { face: 0, gid: 0 };

#[derive(Clone, Copy)]
pub struct Placement {
    pub key: GlyphKey,
    pub x: f32,
    pub advance: f32,
    pub column: usize,
    pub byte: usize,
}

#[derive(Default)]
pub struct LineLayout {
    pub placements: Vec<Placement>,
    pub width: f32,
}

impl LineLayout {
    pub fn build(&mut self, font: &Font, line: &str) {
        self.placements.clear();
        let missing_advance = font.advance();
        let mut x = 0.0;
        let mut previous: Option<GlyphKey> = None;
        for (column, (byte, ch)) in line.char_indices().enumerate() {
            let resolved = font.glyph(ch);
            let (key, advance) = match resolved {
                Some(key) => (key, font.advance_of(key)),
                None => (MISSING, missing_advance),
            };
            if let (Some(left), Some(right)) = (previous, resolved) {
                x += font.kerning(left, right);
            }
            self.placements.push(Placement { key, x, advance, column, byte });
            x += advance;
            previous = resolved;
        }
        self.width = x;
    }

    pub fn x_of_column(&self, column: usize) -> f32 {
        match self.placements.get(column) {
            Some(placement) => placement.x,
            None => self.width,
        }
    }

    pub fn column_at_x(&self, x: f32) -> usize {
        let mut low = 0;
        let mut high = self.placements.len();
        while low < high {
            let middle = low + (high - low) / 2;
            let placement = &self.placements[middle];
            if x < placement.x + placement.advance * 0.5 {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        low
    }

    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }
}
