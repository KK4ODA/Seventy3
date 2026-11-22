//! The waterfall: a ring buffer of spectrum lines held in a GPU texture.
//!
//! Each line remembers the dial frequency it was captured at. When drawing,
//! lines are offset by how far the dial has moved since, so the history
//! stays aligned to RF as you tune (signals don't jump sideways), with no
//! re-upload of the texture.

use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, TextureHandle, TextureOptions, pos2};

use super::palette::Lut;
use crate::config::Palette;

pub const LINES: usize = 1024;

pub struct Waterfall {
    width: usize,
    db: Vec<f32>,
    pixels: Vec<Color32>,
    /// Dial frequency per line; NaN for lines not yet written.
    line_dial: Vec<f64>,
    line_tx: Vec<bool>,
    /// Index of the newest line. Older lines follow at increasing indices.
    head: usize,
    texture: Option<TextureHandle>,
    dirty: Vec<usize>,
    all_dirty: bool,
    pub generation: u64,
    /// Offset from the dial of column 0's centre, and column width, Hz.
    pub start_hz: f64,
    pub col_hz: f64,
    lut: Lut,
    palette: Palette,
    floor: f32,
    range: f32,
}

impl Default for Waterfall {
    fn default() -> Self {
        Waterfall {
            width: 0,
            db: Vec::new(),
            pixels: Vec::new(),
            line_dial: vec![f64::NAN; LINES],
            line_tx: vec![false; LINES],
            head: 0,
            texture: None,
            dirty: Vec::new(),
            all_dirty: true,
            generation: 0,
            start_hz: 0.0,
            col_hz: 1.0,
            lut: Lut::new(Palette::Classic),
            palette: Palette::Classic,
            floor: -110.0,
            range: 50.0,
        }
    }
}

impl Waterfall {
    pub fn width(&self) -> usize {
        self.width
    }

    /// Clears the history and adopts a new frame geometry.
    pub fn reset(&mut self, width: usize, generation: u64, start_hz: f64, col_hz: f64) {
        self.width = width;
        self.generation = generation;
        self.start_hz = start_hz;
        self.col_hz = col_hz;
        self.db = vec![f32::NAN; width * LINES];
        self.pixels = vec![Color32::BLACK; width * LINES];
        self.line_dial.iter_mut().for_each(|d| *d = f64::NAN);
        self.line_tx.iter_mut().for_each(|t| *t = false);
        self.head = 0;
        self.texture = None;
        self.all_dirty = true;
    }

    /// Sets the colour scale. With `recolor`, history is redrawn in the new
    /// colours; otherwise only new lines use them.
    pub fn set_colors(&mut self, palette: Palette, floor: f32, range: f32, recolor: bool) {
        if palette != self.palette {
            self.palette = palette;
            self.lut = Lut::new(palette);
        }
        self.floor = floor;
        self.range = range;
        if recolor && self.width > 0 {
            for (p, &d) in self.pixels.iter_mut().zip(&self.db) {
                *p = self.lut.color(d, floor, range);
            }
            self.all_dirty = true;
        }
    }

    pub fn push(&mut self, db: &[f32], dial_hz: f64, transmitting: bool) {
        if db.len() != self.width || self.width == 0 {
            return;
        }
        self.head = (self.head + LINES - 1) % LINES;
        let row = self.head * self.width;
        self.db[row..row + self.width].copy_from_slice(db);
        for (p, &d) in self.pixels[row..row + self.width].iter_mut().zip(db) {
            *p = self.lut.color(d, self.floor, self.range);
        }
        self.line_dial[self.head] = dial_hz;
        self.line_tx[self.head] = transmitting;
        if !self.all_dirty {
            self.dirty.push(self.head);
        }
    }

    /// Sends changed lines to the GPU.
    pub fn upload(&mut self, ctx: &egui::Context) {
        if self.width == 0 {
            return;
        }
        let options = TextureOptions::LINEAR;
        if self.texture.is_none() || self.all_dirty || self.dirty.len() > 64 {
            let image = egui::ColorImage::new([self.width, LINES], self.pixels.clone());
            match &mut self.texture {
                Some(t) => t.set(image, options),
                None => self.texture = Some(ctx.load_texture("waterfall", image, options)),
            }
        } else if let Some(t) = &mut self.texture {
            for &line in &self.dirty {
                let row = line * self.width;
                let image = egui::ColorImage::new([self.width, 1], self.pixels[row..row + self.width].to_vec());
                t.set_partial([0, line], image, options);
            }
        }
        self.dirty.clear();
        self.all_dirty = false;
    }

    /// Draws the history into `rect`, newest line at the top. `x_of` maps an
    /// offset from the *current* dial (Hz) to a screen x coordinate.
    pub fn paint(&self, painter: &Painter, rect: Rect, dial_hz: f64, x_of: impl Fn(f64) -> f32) {
        let Some(texture) = &self.texture else { return };
        let line_h = rect.height() / LINES as f32;
        let mut mesh = Mesh::with_texture(texture.id());
        let mut tx_marks = Vec::new();

        let mut i = 0;
        while i < LINES {
            let line = (self.head + i) % LINES;
            let dial = self.line_dial[line];
            if dial.is_nan() {
                i += 1;
                continue;
            }
            // Extend over following lines captured at the same dial
            // frequency, as long as they're contiguous in the texture.
            let mut j = i + 1;
            while j < LINES {
                let l = (self.head + j) % LINES;
                if l == 0 || self.line_dial[l] != dial {
                    break;
                }
                j += 1;
            }
            let first = line;
            let count = j - i;
            let shift = dial - dial_hz;
            let left = x_of(shift + self.start_hz - self.col_hz / 2.0);
            let right = x_of(shift + self.start_hz + (self.width as f64 - 0.5) * self.col_hz);
            let top = rect.top() + i as f32 * line_h;
            let quad = Rect::from_min_max(pos2(left, top), pos2(right, top + count as f32 * line_h));
            let uv = Rect::from_min_max(
                pos2(0.0, first as f32 / LINES as f32),
                pos2(1.0, (first + count) as f32 / LINES as f32),
            );
            mesh.add_rect_with_uv(quad, uv, Color32::WHITE);
            i = j;
        }
        for i in 0..LINES {
            let line = (self.head + i) % LINES;
            if self.line_tx[line] && !self.line_dial[line].is_nan() {
                tx_marks.push(rect.top() + i as f32 * line_h);
            }
        }
        painter.add(Shape::mesh(mesh));
        // A red tick at the left edge marks lines received while transmitting.
        for y in tx_marks {
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.left() + 4.0, y)],
                egui::Stroke::new(line_h.max(1.0), super::theme::TX),
            );
        }
    }
}
