//! Tiny SVG-path vector icons rendered with Direct2D geometry.
//!
//! Supports the path commands used by Fluent UI System Icons (M L H V C S A Z,
//! absolute and relative). The title icon is Microsoft's MIT-licensed Fluent
//! "laptop 20 color" icon — the same artwork upstream uses for file grids.

/// A point in icon (viewBox) units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Move(Pt),
    Line(Pt),
    Cubic(Pt, Pt, Pt),
    Arc { to: Pt, rx: f32, ry: f32, rot: f32, large: bool, sweep: bool },
    Close,
}

struct Cursor<'a> {
    s: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn command(&mut self) -> Option<u8> {
        self.skip();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() && c != b'e' && c != b'E' {
            self.i += 1;
            Some(c)
        } else {
            None
        }
    }

    fn at_number(&mut self) -> bool {
        self.skip();
        matches!(self.s.get(self.i), Some(c) if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.'))
    }

    fn number(&mut self) -> Option<f32> {
        self.skip();
        let start = self.i;
        let s = self.s;
        if matches!(s.get(self.i), Some(b'-' | b'+')) {
            self.i += 1;
        }
        let mut dot = false;
        while let Some(&c) = s.get(self.i) {
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !dot {
                dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        if matches!(s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(s.get(self.i), Some(b'-' | b'+')) {
                self.i += 1;
            }
            while matches!(s.get(self.i), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
            }
        }
        std::str::from_utf8(&s[start..self.i]).ok()?.parse().ok()
    }

    /// Arc flags are single characters and may be written without separators.
    fn flag(&mut self) -> Option<bool> {
        self.skip();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Parses SVG path data into absolute segments. Returns `None` on malformed data.
pub fn parse(d: &str) -> Option<Vec<Seg>> {
    let mut c = Cursor { s: d.as_bytes(), i: 0 };
    let mut out = Vec::new();
    let (mut cur, mut start) = (Pt { x: 0.0, y: 0.0 }, Pt { x: 0.0, y: 0.0 });
    let mut last_ctrl: Option<Pt> = None;
    let mut cmd = c.command()?;
    loop {
        let rel = cmd.is_ascii_lowercase();
        let off = |p: Pt, cur: Pt| if rel { Pt { x: p.x + cur.x, y: p.y + cur.y } } else { p };
        let mut ctrl = None;
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let p = off(Pt { x: c.number()?, y: c.number()? }, cur);
                out.push(Seg::Move(p));
                cur = p;
                start = p;
                // Further coordinate pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
                if !c.at_number() {
                    match c.command() {
                        Some(n) => cmd = n,
                        None => break,
                    }
                }
                last_ctrl = None;
                continue;
            }
            b'L' => {
                let p = off(Pt { x: c.number()?, y: c.number()? }, cur);
                out.push(Seg::Line(p));
                cur = p;
            }
            b'H' => {
                let x = c.number()?;
                let p = Pt { x: if rel { cur.x + x } else { x }, y: cur.y };
                out.push(Seg::Line(p));
                cur = p;
            }
            b'V' => {
                let y = c.number()?;
                let p = Pt { x: cur.x, y: if rel { cur.y + y } else { y } };
                out.push(Seg::Line(p));
                cur = p;
            }
            b'C' => {
                let c1 = off(Pt { x: c.number()?, y: c.number()? }, cur);
                let c2 = off(Pt { x: c.number()?, y: c.number()? }, cur);
                let p = off(Pt { x: c.number()?, y: c.number()? }, cur);
                out.push(Seg::Cubic(c1, c2, p));
                ctrl = Some(c2);
                cur = p;
            }
            b'S' => {
                let c1 = match last_ctrl {
                    Some(lc) => Pt { x: 2.0 * cur.x - lc.x, y: 2.0 * cur.y - lc.y },
                    None => cur,
                };
                let c2 = off(Pt { x: c.number()?, y: c.number()? }, cur);
                let p = off(Pt { x: c.number()?, y: c.number()? }, cur);
                out.push(Seg::Cubic(c1, c2, p));
                ctrl = Some(c2);
                cur = p;
            }
            b'A' => {
                let (rx, ry, rot) = (c.number()?, c.number()?, c.number()?);
                let (large, sweep) = (c.flag()?, c.flag()?);
                let p = off(Pt { x: c.number()?, y: c.number()? }, cur);
                out.push(Seg::Arc { to: p, rx: rx.abs(), ry: ry.abs(), rot, large, sweep });
                cur = p;
            }
            b'Z' => {
                out.push(Seg::Close);
                cur = start;
                last_ctrl = None;
                match c.command() {
                    Some(n) => {
                        cmd = n;
                        continue;
                    }
                    None if c.at_number() => return None,
                    None => break,
                }
            }
            _ => return None,
        }
        last_ctrl = ctrl;
        if !c.at_number() {
            match c.command() {
                Some(n) => cmd = n,
                None => break,
            }
        }
    }
    Some(out)
}

/// Fill of one icon layer: a linear gradient in viewBox coordinates.
#[derive(Clone, Copy, Debug)]
pub enum Fill {
    Linear { from: (f32, f32), to: (f32, f32), stops: &'static [(f32, u32)] },
}

#[derive(Clone, Copy, Debug)]
pub struct Layer {
    pub d: &'static str,
    pub fill: Fill,
}

/// Fluent UI System Icons "laptop 20 color" (MIT) — upstream's file-grid title icon.
pub const LAPTOP: &[Layer] = &[
    Layer {
        d: "m4.5 14 5-.5 6 .5h.26a1 1 0 0 1 .45.1l1.34.68a.8.8 0 0 1 .45.72H2c0-.3.17-.59.45-.72l1.34-.67a1 1 0 0 1 .45-.11h.26Z",
        fill: Fill::Linear { from: (18.0, 15.98), to: (17.2, 11.16), stops: &[(0.0, 0xFF63686E), (1.0, 0xFF889096)] },
    },
    Layer {
        d: "M2.5 15a.5.5 0 0 0 0 1h15a.5.5 0 0 0 0-1h-15Z",
        fill: Fill::Linear { from: (18.0, 15.93), to: (17.88, 14.08), stops: &[(0.0, 0xFF889096), (1.0, 0xFFAAB3BD)] },
    },
    Layer {
        d: "M4.5 5C3.67 5 3 5.67 3 6.5v6c0 .83.67 1.5 1.5 1.5h11c.83 0 1.5-.67 1.5-1.5v-6c0-.83-.67-1.5-1.5-1.5h-11Z",
        fill: Fill::Linear { from: (12.8, 5.0), to: (13.28, 15.3), stops: &[(0.0, 0xFF6CE0FF), (1.0, 0xFF4894FE)] },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fluent_paths() {
        for layer in LAPTOP {
            let segs = parse(layer.d).expect(layer.d);
            assert!(matches!(segs.first(), Some(Seg::Move(_))));
            assert_eq!(segs.last(), Some(&Seg::Close));
        }
        // Relative commands, implicit line-tos, compact numbers and arc flags.
        let s = parse("m1 2 3 4h-1v.5a.5.5 0 0 1-.25.25Z").unwrap();
        assert_eq!(s[0], Seg::Move(Pt { x: 1.0, y: 2.0 }));
        assert_eq!(s[1], Seg::Line(Pt { x: 4.0, y: 6.0 }));
        assert_eq!(s[2], Seg::Line(Pt { x: 3.0, y: 6.0 }));
        assert_eq!(s[3], Seg::Line(Pt { x: 3.0, y: 6.5 }));
        assert_eq!(
            s[4],
            Seg::Arc { to: Pt { x: 2.75, y: 6.75 }, rx: 0.5, ry: 0.5, rot: 0.0, large: false, sweep: true }
        );
        // Smooth cubic reflects the previous control point.
        let s = parse("M0 0C1 0 2 1 2 2S3 4 4 4").unwrap();
        assert_eq!(s[2], Seg::Cubic(Pt { x: 2.0, y: 3.0 }, Pt { x: 3.0, y: 4.0 }, Pt { x: 4.0, y: 4.0 }));
        assert!(parse("M0 0 X1").is_none());
    }
}
