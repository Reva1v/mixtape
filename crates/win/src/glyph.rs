//! Media-button glyphs rasterised at runtime: white shapes with anti-aliased edges,
//! readable on the dark taskbar preview.

#![cfg_attr(not(windows), allow(dead_code))]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Previous,
    Play,
    Pause,
    Next,
}

/// Shapes in unit coordinates (0..1, y down).
enum Shape {
    Rect { x0: f32, y0: f32, x1: f32, y1: f32 },
    Tri([(f32, f32); 3]),
}

impl Shape {
    fn contains(&self, x: f32, y: f32) -> bool {
        match *self {
            Shape::Rect { x0, y0, x1, y1 } => (x0..x1).contains(&x) && (y0..y1).contains(&y),
            Shape::Tri([a, b, c]) => {
                let edge = |p: (f32, f32), q: (f32, f32)| {
                    (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0)
                };
                let (d0, d1, d2) = (edge(a, b), edge(b, c), edge(c, a));
                (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
            }
        }
    }

    fn mirrored(self) -> Self {
        match self {
            Shape::Rect { x0, y0, x1, y1 } => Shape::Rect {
                x0: 1.0 - x1,
                y0,
                x1: 1.0 - x0,
                y1,
            },
            Shape::Tri(points) => Shape::Tri(points.map(|(x, y)| (1.0 - x, y))),
        }
    }
}

impl Glyph {
    fn shapes(self) -> Vec<Shape> {
        match self {
            Glyph::Play => vec![Shape::Tri([(0.32, 0.2), (0.32, 0.8), (0.82, 0.5)])],
            Glyph::Pause => vec![
                Shape::Rect {
                    x0: 0.28,
                    y0: 0.22,
                    x1: 0.43,
                    y1: 0.78,
                },
                Shape::Rect {
                    x0: 0.57,
                    y0: 0.22,
                    x1: 0.72,
                    y1: 0.78,
                },
            ],
            Glyph::Next => vec![
                Shape::Tri([(0.2, 0.22), (0.2, 0.78), (0.64, 0.5)]),
                Shape::Rect {
                    x0: 0.66,
                    y0: 0.22,
                    x1: 0.8,
                    y1: 0.78,
                },
            ],
            Glyph::Previous => Glyph::Next
                .shapes()
                .into_iter()
                .map(Shape::mirrored)
                .collect(),
        }
    }
}

/// Premultiplied `0xAARRGGBB` pixels, top-down rows, `size`×`size`.
pub fn render(glyph: Glyph, size: u32) -> Vec<u32> {
    const SS: u32 = 4; // supersampling per axis
    let shapes = glyph.shapes();
    let scale = 1.0 / (size * SS) as f32;
    let mut pixels = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = ((x * SS + sx) as f32 + 0.5) * scale;
                    let v = ((y * SS + sy) as f32 + 0.5) * scale;
                    hits += u32::from(shapes.iter().any(|s| s.contains(u, v)));
                }
            }
            let a = hits * 255 / (SS * SS);
            pixels.push(a << 24 | a << 16 | a << 8 | a);
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(pixels: &[u32], size: u32, x: u32, y: u32) -> u32 {
        pixels[(y * size + x) as usize] >> 24
    }

    #[test]
    fn play_is_filled_in_the_middle_and_clear_at_corners() {
        let px = render(Glyph::Play, 20);
        assert_eq!(px.len(), 400);
        assert_eq!(alpha(&px, 20, 10, 10), 255);
        for (x, y) in [(0, 0), (19, 0), (0, 19), (19, 19)] {
            assert_eq!(alpha(&px, 20, x, y), 0);
        }
    }

    #[test]
    fn pause_has_a_gap_between_bars() {
        let px = render(Glyph::Pause, 20);
        assert_eq!(alpha(&px, 20, 10, 10), 0);
        assert_eq!(alpha(&px, 20, 7, 10), 255);
        assert_eq!(alpha(&px, 20, 12, 10), 255);
    }

    #[test]
    fn previous_mirrors_next() {
        let size = 16;
        let (next, prev) = (render(Glyph::Next, size), render(Glyph::Previous, size));
        for y in 0..size {
            for x in 0..size {
                assert_eq!(
                    alpha(&prev, size, x, y),
                    alpha(&next, size, size - 1 - x, y)
                );
            }
        }
    }

    #[test]
    fn pixels_are_premultiplied_white_with_soft_edges() {
        let px = render(Glyph::Next, 24);
        assert!(px.iter().all(|p| {
            let a = p >> 24;
            (p >> 16 & 0xff) == a && (p >> 8 & 0xff) == a && (p & 0xff) == a
        }));
        assert!(
            px.iter().any(|p| (1..255).contains(&(p >> 24))),
            "expected anti-aliased edge pixels"
        );
    }
}
