//! Motion primitives: easing helpers, a retargetable tween (fades) and a damped
//! spring (geometry). Both continue from the on-screen value when interrupted;
//! the spring also keeps its velocity, which is what makes reversals feel fluid.

pub fn out_cubic(x: f32) -> f32 {
    let inv = 1.0 - x.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

pub fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Maps `v` in [start, end] to [0, 1].
pub fn track(v: f32, start: f32, end: f32) -> f32 {
    if end <= start {
        return if v >= end { 1.0 } else { 0.0 };
    }
    ((v - start) / (end - start)).clamp(0.0, 1.0)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A scalar that eases toward a target; retargeting starts from the current value.
#[derive(Clone, Copy, Debug)]
pub struct Tween {
    from: f32,
    to: f32,
    start: f64,
    dur: f32,
}

impl Tween {
    pub const fn at(v: f32) -> Self {
        Tween { from: v, to: v, start: 0.0, dur: 0.0 }
    }

    pub fn value(&self, now: f64) -> f32 {
        if self.dur <= 0.0 {
            return self.to;
        }
        let t = ((now - self.start) as f32 / self.dur).clamp(0.0, 1.0);
        lerp(self.from, self.to, out_cubic(t))
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    pub fn active(&self, now: f64) -> bool {
        self.dur > 0.0 && ((now - self.start) as f32) < self.dur
    }

    pub fn set(&mut self, to: f32, dur_ms: f32, now: f64) {
        if (to - self.to).abs() < 1e-4 {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.start = now;
        self.dur = dur_ms * (to - self.from).abs().clamp(0.35, 1.0);
    }
}

/// Damped spring (unit mass), parameterised like SwiftUI/iOS: `response` is the
/// undamped period in seconds, `damping` the damping ratio (1 = critical).
/// Retargeting keeps position *and velocity*, so an interrupted motion bends
/// smoothly toward the new goal instead of stopping and restarting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
    pub target: f32,
    k: f32,
    c: f32,
    last: f64,
}

impl Spring {
    pub fn new(x: f32, target: f32, response: f32, damping: f32, now: f64) -> Self {
        let mut s = Spring { x, v: 0.0, target, k: 0.0, c: 0.0, last: now };
        s.tune(response, damping);
        s
    }

    pub fn tune(&mut self, response: f32, damping: f32) {
        let w = std::f32::consts::TAU / response.max(0.02);
        self.k = w * w;
        self.c = 2.0 * damping * w;
    }

    /// Advances to `now`; returns `true` once settled (snapped to the target).
    pub fn step(&mut self, now: f64) -> bool {
        // Cap the step so a stalled frame never makes the spring jump.
        let mut dt = ((now - self.last) / 1000.0).clamp(0.0, 0.05) as f32;
        self.last = now;
        while dt > 0.0 {
            let h = dt.min(0.002);
            let a = -self.k * (self.x - self.target) - self.c * self.v;
            self.v += a * h;
            self.x += self.v * h;
            dt -= h;
        }
        if (self.x - self.target).abs() < 0.4 && self.v.abs() < 8.0 {
            self.x = self.target;
            self.v = 0.0;
            return true;
        }
        false
    }
}

/// High-resolution monotonic clock in milliseconds.
pub fn now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_settles_and_keeps_velocity() {
        let mut s = Spring::new(0.0, 100.0, 0.25, 1.0, 0.0);
        let mut t = 0.0;
        while !s.step(t) {
            t += 16.0;
            assert!(t < 1000.0, "spring did not settle");
        }
        assert_eq!(s.x, 100.0);
        // Retarget mid-flight: velocity is preserved, so it keeps moving forward briefly.
        let mut s = Spring::new(0.0, 100.0, 0.25, 1.0, 0.0);
        s.step(48.0);
        let (x, v) = (s.x, s.v);
        assert!(v > 0.0);
        s.target = 0.0;
        s.step(52.0);
        assert!(s.x > x);
    }

    #[test]
    fn tween_retarget_is_continuous() {
        let mut t = Tween::at(0.0);
        t.set(1.0, 100.0, 0.0);
        let mid = t.value(50.0);
        t.set(0.0, 100.0, 50.0);
        assert!((t.value(50.0) - mid).abs() < 1e-5);
        assert!(!t.active(1000.0));
        assert_eq!(t.value(1000.0), 0.0);
    }
}
