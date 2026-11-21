//! WaveTag shapes. Every shape returns how much to turn the sound DOWN
//! (0 = full volume, 1 = full cut) at phase `x` (0..1) through one cycle.
//! `bend` (0..1, 0.5 = normal) reshapes each one.

pub const SHAPE_NAMES: [&str; 21] = [
    "SINE", "TRI", "SQUARE", "SAW DN", "SAW UP", "SMOOTH", "PUNCH", "EXPO", "PUMP", "GATE", "DOUBLE", "CHOP",
    "RANDOM", // ---- BASS row (dubstep / riddim) ----
    "WOBBLE", "TRIPLET", "HALF-TIME", "RIDDIM", "STUTTER", "GROWL", "YOI", "SWING",
];
/// Shapes 0..CLASSIC are the first row; the rest are the BASS row.
pub const CLASSIC: usize = 13;
pub const LAST_SHAPE: i32 = SHAPE_NAMES.len() as i32 - 1;

pub const RATE_NAMES: [&str; 18] = [
    "4 BAR", "2 BAR", "1 BAR", "1/2D", "1/2", "1/2T", "1/4D", "1/4", "1/4T", "1/8D", "1/8", "1/8T", "1/16D", "1/16",
    "1/16T", "1/32", "1/32T", "1/64",
];
/// Length of one cycle in beats (quarter notes), matching RATE_NAMES.
pub const RATE_BEATS: [f64; 18] = [
    16.0,
    8.0,
    4.0,
    3.0,
    2.0,
    4.0 / 3.0,
    1.5,
    1.0,
    2.0 / 3.0,
    0.75,
    0.5,
    1.0 / 3.0,
    0.375,
    0.25,
    1.0 / 6.0,
    0.125,
    1.0 / 12.0,
    0.0625,
];
pub const DEFAULT_RATE: usize = 10; // 1/8

/// Small integer hash -> 0..1, for the RANDOM shape (same "random" every loop).
fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

#[inline]
pub fn shape_at(kind: usize, x: f32, bend: f32) -> f32 {
    let x = x.rem_euclid(1.0);
    let k = 2.0f32.powf(bend * 4.0 - 1.0); // 0.5 .. 8, 2 at bend 0.5
    let pi = std::f32::consts::PI;
    let v = match kind {
        // Smooth tremolo; bend skews where the dip sits.
        0 => {
            let xs = x.powf(2.0f32.powf((0.5 - bend) * 2.0));
            0.5 + 0.5 * (2.0 * pi * xs).cos()
        }
        1 => {
            let xs = x.powf(2.0f32.powf((0.5 - bend) * 2.0));
            (1.0 - 2.0 * xs).abs()
        }
        // Bend = how long the cut lasts (pulse width).
        2 => {
            if x < 0.05 + 0.9 * bend { 1.0 } else { 0.0 }
        }
        3 => (1.0 - x).powf(k / 2.0),
        4 => x.powf(k / 2.0),
        // The KickNull / Kickstart-style family.
        5 => (1.0 - x).powf(k),
        6 => {
            let h = 0.05 + 0.4 * (1.0 - bend);
            if x < h { 1.0 } else { (1.0 - (x - h) / (1.0 - h)).powf(2.0) }
        }
        7 => {
            let a = 2.0 + 14.0 * bend;
            ((-a * x).exp() - (-a).exp()) / (1.0 - (-a).exp())
        }
        8 => 1.0 - x.powf(k),
        9 => {
            let g = 0.15 + 0.7 * bend;
            if x < g { 1.0 } else { 0.0 }
        }
        10 => {
            if x < 0.5 { (1.0 - 2.0 * x).powf(k) } else { 0.6 * (1.0 - (2.0 * x - 1.0)).powf(k) }
        }
        // Chopper: 2..16 hard steps per cycle (bend = how many).
        11 => {
            let steps = (2.0 + (bend * 14.0).round()) as u32;
            if ((x * steps as f32) as u32) % 2 == 0 { 0.0 } else { 1.0 }
        }
        // Stepped random level, 4..16 steps per cycle (bend = how many).
        12 => {
            let steps = (4.0 + (bend * 12.0).round()) as u32;
            let i = (x * steps as f32) as u32;
            hash01(i * 7919 + steps)
        }
        // ---- BASS row ----
        // WOBBLE: one wub per cycle, opens in the middle. Bend = how narrow.
        13 => 1.0 - (pi * x).sin().powf(0.4 + 3.0 * bend),
        // TRIPLET: three wubs per cycle (set RATE to 1/2 or 1 BAR for triplet wubs).
        14 => 1.0 - (pi * (x * 3.0).fract()).sin().powf(0.4 + 3.0 * bend),
        // HALF-TIME: long wub on the first half, gap, short wub at the end.
        15 => {
            if x < 0.5 {
                1.0 - (pi * x * 2.0).sin().powf(0.3 + 2.0 * bend)
            } else if x < 0.625 {
                1.0
            } else {
                1.0 - 0.8 * (pi * (x - 0.625) / 0.375).sin().powf(0.5 + 2.0 * bend)
            }
        }
        // RIDDIM: 8-step chop pattern, bend = how long each hit rings.
        16 => {
            const PAT: [bool; 8] = [true, true, false, true, false, true, true, false];
            let s = x * 8.0;
            let on = PAT[(s as usize).min(7)];
            if on && s.fract() < 0.3 + 0.7 * bend { 0.0 } else { 1.0 }
        }
        // STUTTER: chops get faster through the cycle (2, 4, 8, 16), like a fill.
        17 => {
            let (start, len, n) = if x < 0.5 { (0.0, 0.5, 2.0) } else if x < 0.75 { (0.5, 0.25, 4.0) } else if x < 0.875 { (0.75, 0.125, 8.0) } else { (0.875, 0.125, 16.0) };
            let s = ((x - start) / len * n).fract();
            if s < 0.35 + 0.5 * bend { 0.0 } else { 1.0 }
        }
        // GROWL: fast, uneven wobble (4 per cycle, bend = how deep), talk-y.
        18 => {
            let wob = 0.5 + 0.5 * (2.0 * pi * (4.0 * x + 0.15 * (2.0 * pi * x).sin())).cos();
            let depth = 0.35 + 0.65 * bend;
            wob * depth * (0.7 + 0.3 * (2.0 * pi * 2.0 * x).sin().abs())
        }
        // YOI: snap open, dip, swell back, cut off at the end.
        19 => {
            let c = if x < 0.08 {
                1.0 - x / 0.08
            } else if x < 0.3 {
                0.45 * ((x - 0.08) / 0.22)
            } else if x < 0.8 {
                0.45 * (1.0 - (x - 0.3) / 0.5)
            } else {
                ((x - 0.8) / 0.2).powf(0.5)
            };
            c * (0.6 + 0.8 * bend).min(1.0) + (1.0 - (0.6 + 0.8 * bend).min(1.0)) * 0.0
        }
        // SWING: 16th chops (4 per cycle at 1/4) with swing; bend = swing amount.
        _ => {
            let s = x * 4.0;
            let i = s as u32;
            let f = s.fract();
            let sw = 0.5 + 0.25 * bend; // where the off-beat lands
            let hit = if i % 2 == 0 { f < 0.35 } else { f > sw - 0.5 && f < sw - 0.15 };
            if hit { 0.0 } else { 1.0 }
        }
    };
    v.clamp(0.0, 1.0)
}

/// Two-stage (critically damped) smoother on the gain. A single stage still
/// starts every move at full speed, a sharp corner that clicks on loud bass
/// (the PUMP-on-an-808 click). Two stages ease in AND out of every move.
/// `coef` is per stage; use `slew_coef(SMOOTH ms)`.
#[derive(Clone, Copy)]
pub struct GainSlew {
    pub y: f32,
    y1: f32,
}

impl Default for GainSlew {
    fn default() -> Self {
        Self { y: 1.0, y1: 1.0 }
    }
}

impl GainSlew {
    #[inline]
    pub fn tick(&mut self, target: f32, coef: f32) -> f32 {
        self.y1 = target + (self.y1 - target) * coef;
        self.y = self.y1 + (self.y - self.y1) * coef;
        self.y
    }
}

impl GainSlew {
    /// Jump straight to `v` (used by ZERO-X at a zero crossing).
    pub fn snap(&mut self, v: f32) {
        self.y = v;
        self.y1 = v;
    }
}

/// ZERO-X: only change the gain when the audio crosses zero, where the wave is
/// momentarily silent, so even a full-volume -> silence jump can't click.
/// If the audio doesn't cross zero for `max_wait` samples (silence, DC, very
/// slow sub), it falls back to the SMOOTH glide so the wave still moves.
#[derive(Clone, Copy, Default)]
pub struct ZeroX {
    prev: f32,
    since: u32,
}

impl ZeroX {
    #[inline]
    pub fn tick(&mut self, x: f32, target: f32, slew: &mut GainSlew, coef: f32, max_wait: u32) -> f32 {
        let crossed = x == 0.0 || self.prev == 0.0 || (x > 0.0) != (self.prev > 0.0);
        self.prev = x;
        if crossed {
            self.since = 0;
            slew.snap(target);
        } else {
            self.since = self.since.saturating_add(1);
            if self.since > max_wait {
                slew.tick(target, coef);
            }
        }
        slew.y
    }
}

pub const ZERO_X_MAX_WAIT_MS: f32 = 30.0;

/// Per-stage coefficient so the two stages together take about `ms`.
pub fn slew_coef(ms: f32, sr: f32) -> f32 {
    coef(ms * 0.5, sr)
}

pub fn coef(ms: f32, sr: f32) -> f32 {
    let samples = ms * 0.001 * sr;
    if samples < 1.0 { 0.0 } else { (-1.0 / samples).exp() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PUMP at 1/16 on a loud 40 Hz 808, Smooth at the 0.5 ms minimum.
    /// Click = how far the output jumps in one sample beyond what the 808 itself
    /// does. Without ZERO-X the wrap jumps at full level; with it, the jump only
    /// happens at a zero crossing, so it stays at the 808's own sample-to-sample size.
    #[test]
    fn zero_x_removes_the_pump_click() {
        let sr = 44100.0;
        let c = slew_coef(crate::SMOOTH_MIN_MS, sr);
        let cycle_s = RATE_BEATS[13] as f32 * 60.0 / 140.0; // 1/16 @ 140
        let wait = (ZERO_X_MAX_WAIT_MS * 0.001 * sr) as u32;
        let natural = 2.0 * std::f32::consts::PI * 40.0 / sr; // 808's own biggest step
        let run = |zx_on: bool| -> f32 {
            let (mut slew, mut zx) = (GainSlew::default(), ZeroX::default());
            let (mut prev, mut worst) = (0.0f32, 0.0f32);
            for n in 0..(sr as usize) {
                let t = n as f32 / sr;
                let x = (2.0 * std::f32::consts::PI * 40.0 * t).sin();
                let target = 1.0 - shape_at(8, (t / cycle_s).fract(), 0.5);
                let g = if zx_on { zx.tick(x, target, &mut slew, c, wait) } else { slew.tick(target, c) };
                let y = x * g;
                worst = worst.max((y - prev).abs());
                prev = y;
            }
            worst
        };
        let (off, on) = (run(false), run(true));
        assert!(off > 5.0 * natural, "test should show the click without ZERO-X: {off}");
        assert!(on < 1.5 * natural, "ZERO-X still jumps {on} (808 itself: {natural})");
    }

    /// A full-volume -> full-cut jump (the PUMP wrap): the move must start
    /// gently. Measured as the biggest change in speed between two samples;
    /// a one-stage smoother has its worst corner right at the start.
    #[test]
    fn smooth_has_no_sharp_corner() {
        let sr = 44100.0;
        let ms = 5.0;
        let corner = |two: bool| -> f32 {
            let mut s = GainSlew::default();
            let mut one = 1.0f32;
            let c1 = coef(ms, sr);
            let c2 = slew_coef(ms, sr);
            let (mut prev, mut prev_step, mut worst) = (1.0f32, 0.0f32, 0.0f32);
            for _ in 0..(sr as usize / 20) {
                let g = if two { s.tick(0.0, c2) } else { one = 0.0 + (one - 0.0) * c1; one };
                let step = g - prev;
                worst = worst.max((step - prev_step).abs());
                prev = g;
                prev_step = step;
            }
            worst
        };
        let (old, new) = (corner(false), corner(true));
        assert!(new < old * 0.3, "two-stage corner {new} vs one-stage {old}");
    }

    #[test]
    fn every_shape_stays_in_range() {
        for kind in 0..SHAPE_NAMES.len() {
            for b in 0..=10 {
                for i in 0..2000 {
                    let v = shape_at(kind, i as f32 / 2000.0, b as f32 / 10.0);
                    assert!(v.is_finite() && (0.0..=1.0).contains(&v), "{} = {v}", SHAPE_NAMES[kind]);
                }
            }
        }
    }

    #[test]
    fn rate_tables_line_up() {
        assert_eq!(RATE_NAMES.len(), RATE_BEATS.len());
        assert_eq!(RATE_NAMES[DEFAULT_RATE], "1/8");
        assert_eq!(RATE_BEATS[7], 1.0); // 1/4 = one beat
    }

    /// No clicks: at the fastest rate (1/64 at 174 bpm) and the shortest SMOOTH,
    /// the gain never jumps more than ~1/24 in one sample, for every shape.
    #[test]
    fn no_shape_clicks_with_minimum_smooth() {
        let sr = 44100.0;
        let c = slew_coef(crate::SMOOTH_MIN_MS, sr);
        let cycle_s = RATE_BEATS[17] as f32 * 60.0 / 174.0;
        for kind in 0..SHAPE_NAMES.len() {
            let mut s = GainSlew::default();
            let mut prev = 1.0;
            let mut worst = 0.0f32;
            for n in 0..(sr as usize) {
                let x = (n as f32 / sr) / cycle_s;
                let g = s.tick(1.0 - shape_at(kind, x, 0.5), c);
                worst = worst.max((g - prev).abs());
                prev = g;
            }
            assert!(worst < 0.045, "{} jumps by {worst} per sample", SHAPE_NAMES[kind]);
        }
    }
}
