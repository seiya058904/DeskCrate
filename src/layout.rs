//! Tile geometry, ported from upstream `FileWidgetIconLayout` so densities match.
//! Everything here is in DIP.

use crate::config::Appearance;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileMetrics {
    pub icon: f32,
    pub text: f32,
    pub label_w: f32,
    pub lines: u32,
    pub tile_w: f32,
    pub tile_h: f32,
    pub margin_x: f32,
    pub margin_y: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    pub spacing: f32,
    pub line_h: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl TileMetrics {
    pub fn new(a: &Appearance) -> Self {
        let icon = a.icon_size;
        let text = a.text_size;
        let h = a.spacing_h;
        let v = a.spacing_v;
        let lines = a.label_lines;
        let label_w = icon.max(lerp(icon, text * 10.5, a.name_width));
        let tile_w = (icon + lerp(6.0, 28.0, h)).max(label_w + lerp(4.0, 16.0, h));
        let line_h = (text * 1.4).ceil();
        let two_line_min = icon + lerp(24.0, 70.0, v);
        let one_line_min = (icon + text + 8.0).max(two_line_min - text - 3.0);
        let visual_min = match lines {
            0 => (icon + 8.0).max(one_line_min - text - 3.0),
            1 => one_line_min,
            _ => two_line_min,
        };
        let spacing = if lines == 0 { 0.0 } else { lerp(1.0, 7.0, v) };
        let label_h = line_h * lines as f32;
        let tile_h = visual_min.max(lerp(1.0, 6.0, v) * 2.0 + icon + spacing + label_h + 2.0).ceil();
        TileMetrics {
            icon,
            text,
            label_w,
            lines,
            tile_w,
            tile_h,
            margin_x: lerp(0.0, 2.0, h),
            margin_y: lerp(0.0, 2.0, v),
            pad_x: lerp(1.0, 5.0, h),
            pad_y: lerp(1.0, 6.0, v),
            spacing,
            line_h,
        }
    }

    pub fn cell_w(&self) -> f32 {
        (self.tile_w + self.margin_x * 2.0).ceil()
    }

    pub fn cell_h(&self) -> f32 {
        (self.tile_h + self.margin_y * 2.0).ceil()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectF {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl RectF {
    pub fn new(l: f32, t: f32, r: f32, b: f32) -> Self {
        RectF { l, t, r, b }
    }
    pub fn w(&self) -> f32 {
        self.r - self.l
    }
    pub fn h(&self) -> f32 {
        self.b - self.t
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
    pub fn intersects(&self, o: &RectF) -> bool {
        self.l < o.r && o.l < self.r && self.t < o.b && o.t < self.b
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Self {
        RectF::new(self.l + dx, self.t + dy, self.r - dx, self.b - dy)
    }
}

/// Icon-grid layout for a given content area.
#[derive(Clone, Copy, Debug)]
pub struct GridLayout {
    pub cols: usize,
    pub origin_x: f32,
    pub origin_y: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub count: usize,
}

impl GridLayout {
    pub fn new(m: &TileMetrics, area: RectF, count: usize) -> Self {
        let cell_w = m.cell_w();
        let cell_h = m.cell_h();
        let cols = ((area.w() / cell_w).floor() as usize).max(1);
        let used = cols as f32 * cell_w;
        let origin_x = area.l + ((area.w() - used) / 2.0).max(0.0).floor();
        GridLayout { cols, origin_x, origin_y: area.t, cell_w, cell_h, count }
    }

    pub fn rows(&self) -> usize {
        self.count.div_ceil(self.cols)
    }

    pub fn content_height(&self) -> f32 {
        self.rows() as f32 * self.cell_h
    }

    /// Tile rectangle (without margin) at `scroll` offset.
    pub fn tile_rect(&self, m: &TileMetrics, i: usize, scroll: f32) -> RectF {
        let col = (i % self.cols) as f32;
        let row = (i / self.cols) as f32;
        let l = self.origin_x + col * self.cell_w + m.margin_x;
        let t = self.origin_y + row * self.cell_h + m.margin_y - scroll;
        RectF::new(l, t, l + m.tile_w, t + m.tile_h)
    }

    pub fn hit(&self, m: &TileMetrics, x: f32, y: f32, scroll: f32) -> Option<usize> {
        if x < self.origin_x || y < self.origin_y {
            return None;
        }
        let col = ((x - self.origin_x) / self.cell_w) as usize;
        let row = ((y - self.origin_y + scroll) / self.cell_h) as usize;
        if col >= self.cols {
            return None;
        }
        let i = row * self.cols + col;
        (i < self.count && self.tile_rect(m, i, scroll).contains(x, y)).then_some(i)
    }

    /// Range of indices whose rows intersect the viewport.
    pub fn visible(&self, viewport_h: f32, scroll: f32) -> std::ops::Range<usize> {
        let first_row = (scroll / self.cell_h).floor().max(0.0) as usize;
        let last_row = ((scroll + viewport_h) / self.cell_h).floor().max(0.0) as usize;
        let a = (first_row * self.cols).min(self.count);
        let b = ((last_row + 1) * self.cols).min(self.count);
        a..b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_metrics_match_upstream() {
        let a = crate::config::Config::default().appearance;
        let m = TileMetrics::new(&a);
        assert!((m.label_w - 62.67).abs() < 0.05, "{}", m.label_w);
        assert!((m.tile_w - 71.47).abs() < 0.05, "{}", m.tile_w);
        assert_eq!(m.tile_h, 82.0);
        assert_eq!(m.cell_w(), 74.0);
        // 280-DIP widget = three columns, as upstream.
        let g = GridLayout::new(&m, RectF::new(8.0, 0.0, 272.0, 300.0), 10);
        assert_eq!(g.cols, 3);
        assert_eq!(g.rows(), 4);
    }

    #[test]
    fn hit_and_visible() {
        let a = crate::config::Config::default().appearance;
        let m = TileMetrics::new(&a);
        let g = GridLayout::new(&m, RectF::new(0.0, 0.0, 222.0, 200.0), 7);
        let r = g.tile_rect(&m, 4, 0.0);
        assert_eq!(g.hit(&m, r.l + 2.0, r.t + 2.0, 0.0), Some(4));
        assert_eq!(g.hit(&m, 500.0, 5.0, 0.0), None);
        assert_eq!(g.visible(100.0, 0.0), 0..6);
    }
}
