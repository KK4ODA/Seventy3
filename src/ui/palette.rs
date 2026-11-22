//! Waterfall colour maps.

use egui::Color32;

use crate::config::Palette;

/// A 256-entry colour lookup table.
#[derive(Clone)]
pub struct Lut {
    colors: [Color32; 256],
}

fn stops(p: Palette) -> &'static [(f32, [u8; 3])] {
    match p {
        Palette::Classic => &[
            (0.00, [0, 0, 0]),
            (0.18, [4, 10, 70]),
            (0.38, [10, 60, 200]),
            (0.55, [0, 190, 220]),
            (0.70, [230, 230, 40]),
            (0.85, [255, 110, 0]),
            (1.00, [255, 255, 255]),
        ],
        Palette::Ember => &[
            (0.00, [0, 0, 4]),
            (0.25, [70, 14, 100]),
            (0.50, [180, 54, 90]),
            (0.75, [248, 140, 20]),
            (1.00, [252, 255, 164]),
        ],
        Palette::Phosphor => &[(0.00, [0, 0, 0]), (0.40, [0, 60, 20]), (0.75, [40, 200, 80]), (1.00, [220, 255, 220])],
        Palette::Gray => &[(0.0, [0, 0, 0]), (1.0, [255, 255, 255])],
    }
}

impl Lut {
    pub fn new(palette: Palette) -> Lut {
        let stops = stops(palette);
        let mut colors = [Color32::BLACK; 256];
        for (i, c) in colors.iter_mut().enumerate() {
            let t = i as f32 / 255.0;
            let k = stops.iter().position(|s| s.0 >= t).unwrap_or(stops.len() - 1).max(1);
            let (t0, a) = stops[k - 1];
            let (t1, b) = stops[k];
            let f = if t1 > t0 { ((t - t0) / (t1 - t0)).clamp(0.0, 1.0) } else { 0.0 };
            let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * f).round() as u8;
            *c = Color32::from_rgb(mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]));
        }
        Lut { colors }
    }

    /// Colour for `db` on a scale from `floor` to `floor + range`.
    #[inline]
    pub fn color(&self, db: f32, floor: f32, range: f32) -> Color32 {
        let t = (db - floor) / range.max(1.0);
        if t.is_nan() {
            return Color32::BLACK;
        }
        self.colors[(t.clamp(0.0, 1.0) * 255.0) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ends_and_monotone_brightness() {
        for p in Palette::ALL {
            let lut = Lut::new(p);
            assert_eq!(lut.color(-200.0, -100.0, 50.0), lut.colors[0]);
            assert_eq!(lut.color(0.0, -100.0, 50.0), lut.colors[255]);
            let lum = |c: Color32| u32::from(c.r()) * 3 + u32::from(c.g()) * 6 + u32::from(c.b());
            assert!(lum(lut.colors[255]) > lum(lut.colors[128]));
            assert!(lum(lut.colors[128]) > lum(lut.colors[0]));
        }
    }
}
