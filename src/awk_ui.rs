//! awk studios faceplate kit, shared by every plugin (same file in each project).
//! Master copy: ~/Projects/awk-ui/awk_ui.rs. Copy it into a project with ~/Projects/awk-ui/sync.sh.
//!
//! Three looks:
//!   Blackface: brushed panel, knurled knobs with silver caps, LED ladders (ChopShop)
//!   Hud:       vector grid, cut-corner frames, segmented ring knobs (WaveTag, AlleyVerb)
//!   Machined:  flat graphite cards, aluminium knobs with a value ring (KickNull)
//!
//! Also holds the audio-side helpers every plugin shares: L/R peak meters,
//! the global MIX + BYPASS params and their click-free crossfade.

#![allow(dead_code)]

use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, pos2, text::LayoutJob, vec2, Align2, Color32, ColorImage, FontData, FontDefinitions, FontFamily, FontId, Mesh, Painter, Pos2, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, TextFormat, TextureHandle, TextureOptions, Vec2,
};
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::hash::Hash;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------- audio side

/// Peak meters: in L, in R, out L, out R in dBFS (fast attack, ~300 ms fall),
/// then [4] = how hard the SAFE output limiter is working, in dB (0 or less).
pub struct Meters(pub [AtomicU32; 5]);
impl Default for Meters {
    fn default() -> Self {
        Self(std::array::from_fn(|i| AtomicU32::new((if i == 4 { 0.0f32 } else { -120.0 }).to_bits())))
    }
}
impl Meters {
    pub fn db(&self, i: usize) -> f32 {
        f32::from_bits(self.0[i].load(Ordering::Relaxed))
    }
}

/// Audio-thread peak follower feeding `Meters`.
#[derive(Clone, Copy)]
pub struct PeakFollower {
    pk: [f32; 4],
    fall: f32,
}
impl Default for PeakFollower {
    fn default() -> Self {
        Self { pk: [0.0; 4], fall: 0.9995 }
    }
}
impl PeakFollower {
    pub fn set_sr(&mut self, sr: f32) {
        // ~300 ms to fall 20 dB.
        self.fall = (-1.0 / (0.3 * sr.max(1.0) / 2.3)).exp();
    }
    #[inline]
    pub fn feed(&mut self, il: f32, ir: f32, ol: f32, or: f32) {
        for (p, v) in self.pk.iter_mut().zip([il, ir, ol, or]) {
            let a = v.abs();
            *p = if a > *p { a } else { *p * self.fall };
        }
    }
    pub fn store(&self, m: &Meters) {
        for i in 0..4 {
            m.0[i].store((20.0 * (self.pk[i] + 1e-9).log10()).to_bits(), Ordering::Relaxed);
        }
    }
}

/// Lock-free ring of recent output samples for an editor spectrum display.
/// The audio thread writes, the editor copies the newest `n` samples.
pub struct SpecRing {
    buf: Vec<AtomicU32>,
    w: std::sync::atomic::AtomicUsize,
    pub sr: AtomicU32,
}
impl SpecRing {
    pub fn new(len: usize) -> Self {
        Self { buf: (0..len.next_power_of_two()).map(|_| AtomicU32::new(0)).collect(), w: std::sync::atomic::AtomicUsize::new(0), sr: AtomicU32::new(48000f32.to_bits()) }
    }
    #[inline]
    pub fn push(&self, x: f32) {
        let i = self.w.load(Ordering::Relaxed);
        self.buf[i & (self.buf.len() - 1)].store(x.to_bits(), Ordering::Relaxed);
        self.w.store(i.wrapping_add(1), Ordering::Release);
    }
    /// Write counter (changes only while audio runs).
    pub fn written(&self) -> usize {
        self.w.load(Ordering::Acquire)
    }
    /// Copies the newest `out.len()` samples, oldest first.
    pub fn read(&self, out: &mut [f32]) {
        let end = self.w.load(Ordering::Acquire);
        let m = self.buf.len() - 1;
        let n = out.len().min(self.buf.len());
        for (k, o) in out.iter_mut().enumerate().take(n) {
            *o = f32::from_bits(self.buf[(end.wrapping_sub(n) + k) & m].load(Ordering::Relaxed));
        }
    }
}

/// The global MIX knob in the header (dry/wet of the whole plugin).
pub fn mix_param() -> FloatParam {
    FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_smoother(SmoothingStyle::Linear(20.0))
        .with_unit("%")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

/// The power button in the header. It's the host's bypass too.
pub fn bypass_param() -> BoolParam {
    BoolParam::new("Bypass", false).make_bypass()
}

/// SAFE: the output never goes above -0.1 dBFS. On by default in every plugin.
pub fn safe_param() -> BoolParam {
    BoolParam::new("Safety Limiter", true)
}

/// Output ceiling for the SAFE switch. Zero latency: gain drops instantly to
/// whatever keeps the loudest channel at the ceiling, then recovers over ~100 ms.
/// Linked stereo, so the image doesn't shift. Also turns NaN/inf into silence.
#[derive(Clone, Copy)]
pub struct Ceiling {
    g: f32,
    rel: f32,
    worst: f32,
}
pub const CEILING: f32 = 0.988_553; // -0.1 dBFS
impl Default for Ceiling {
    fn default() -> Self {
        Self { g: 1.0, rel: 0.9998, worst: 1.0 }
    }
}
impl Ceiling {
    pub fn set_sr(&mut self, sr: f32) {
        self.rel = (-1.0 / (0.1 * sr.max(1.0))).exp();
    }
    #[inline]
    pub fn tick(&mut self, l: f32, r: f32, on: bool) -> (f32, f32) {
        let (l, r) = (if l.is_finite() { l } else { 0.0 }, if r.is_finite() { r } else { 0.0 });
        if !on {
            self.g = 1.0;
            return (l, r);
        }
        let pk = l.abs().max(r.abs());
        let need = if pk > CEILING { CEILING / pk } else { 1.0 };
        self.g = if need < self.g { need } else { need + (self.g - need) * self.rel };
        self.worst = self.worst.min(self.g);
        // Recovery can never push a sample back over the ceiling.
        let g = self.g.min(need);
        (l * g, r * g)
    }
    /// Publishes the deepest gain reduction since the last call into `Meters[4]`.
    pub fn store(&mut self, m: &Meters) {
        m.0[4].store((20.0 * self.worst.max(1e-6).log10()).min(0.0).to_bits(), Ordering::Relaxed);
        self.worst = 1.0;
    }
}

pub const OS_NAMES: [&str; 4] = ["OFF", "2x", "4x", "8x"];
pub fn os_param() -> IntParam {
    IntParam::new("Oversampling", 0, IntRange::Linear { min: 0, max: 3 })
        .with_value_to_string(Arc::new(|v| OS_NAMES[v.clamp(0, 3) as usize].to_string()))
        .with_string_to_value(Arc::new(|s| OS_NAMES.iter().position(|n| n.eq_ignore_ascii_case(s.trim())).map(|i| i as i32)))
}

/// Click-free bypass: a 20 ms ramp. `next()` returns the wet amount (1 = on, 0 = bypassed).
#[derive(Clone, Copy)]
pub struct BypassRamp {
    g: f32,
    c: f32,
}
impl Default for BypassRamp {
    fn default() -> Self {
        Self { g: 1.0, c: 0.999 }
    }
}
impl BypassRamp {
    pub fn set_sr(&mut self, sr: f32) {
        self.c = (-1.0 / (0.02 * sr.max(1.0) / 4.0)).exp();
    }
    #[inline]
    pub fn next(&mut self, bypassed: bool) -> f32 {
        let t = if bypassed { 0.0 } else { 1.0 };
        self.g = t + (self.g - t) * self.c;
        if (self.g - t).abs() < 1e-5 {
            self.g = t;
        }
        self.g
    }
}

/// Fixed delay line for lining the dry signal up with a plugin's latency.
#[derive(Clone)]
pub struct DryDelay {
    buf: Vec<f32>,
    pos: usize,
}
impl DryDelay {
    pub fn new(max: usize) -> Self {
        Self { buf: vec![0.0; max.max(1) + 1], pos: 0 }
    }
    pub fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
    }
    /// Writes `x`, returns the sample from `d` samples ago (d is clamped to the capacity).
    #[inline]
    pub fn tick(&mut self, x: f32, d: usize) -> f32 {
        let n = self.buf.len();
        self.buf[self.pos] = x;
        let out = self.buf[(self.pos + n - d.min(n - 1)) % n];
        self.pos = (self.pos + 1) % n;
        out
    }
}

// ---------------------------------------------------------------- fonts

pub fn install_fonts(ctx: &egui::Context) {
    let mut f = FontDefinitions::default();
    let fallbacks: Vec<String> = ["Ubuntu-Light", "NotoEmoji-Regular", "emoji-icon-font"].iter().filter(|n| f.font_data.contains_key(**n)).map(|s| s.to_string()).collect();
    let faces: [(&str, &'static [u8]); 4] = [
        ("awk-d", include_bytes!("../assets/Michroma-Regular.ttf")),
        ("awk-l", include_bytes!("../assets/BarlowCondensed-SemiBold.ttf")),
        ("awk-b", include_bytes!("../assets/BarlowCondensed-Bold.ttf")),
        ("awk-m", include_bytes!("../assets/ShareTechMono-Regular.ttf")),
    ];
    for (name, bytes) in faces {
        f.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
        let mut fam = vec![name.to_string()];
        fam.extend(fallbacks.iter().cloned());
        f.families.insert(FontFamily::Name(name.into()), fam);
    }
    // Old family names still used by a few widgets.
    for old in ["block", "marker"] {
        let mut fam = vec!["awk-b".to_string()];
        fam.extend(fallbacks.iter().cloned());
        f.families.insert(FontFamily::Name(old.into()), fam);
    }
    if let Some(p) = f.families.get_mut(&FontFamily::Proportional) {
        p.insert(0, "awk-l".into());
    }
    if let Some(m) = f.families.get_mut(&FontFamily::Monospace) {
        m.insert(0, "awk-m".into());
    }
    ctx.set_fonts(f);
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::from_rgb(0x10, 0x11, 0x13);
    ctx.set_visuals(v);
}

pub fn disp(s: f32) -> FontId {
    FontId::new(s, FontFamily::Name("awk-d".into()))
}
pub fn lab(s: f32) -> FontId {
    FontId::new(s, FontFamily::Name("awk-l".into()))
}
pub fn labb(s: f32) -> FontId {
    FontId::new(s, FontFamily::Name("awk-b".into()))
}
pub fn mono(s: f32) -> FontId {
    FontId::new(s, FontFamily::Name("awk-m".into()))
}

/// Text with letter spacing; returns the rect it took.
pub fn txt(p: &Painter, pos: Pos2, a: Align2, s: impl AsRef<str>, font: FontId, col: Color32, sp: f32) -> Rect {
    let mut job = LayoutJob::default();
    job.append(s.as_ref(), 0.0, TextFormat { font_id: font, color: col, extra_letter_spacing: sp, ..Default::default() });
    let g = p.layout_job(job);
    let r = a.anchor_size(pos, g.size());
    p.galley(r.min, g, col);
    r
}
pub fn txt_w(p: &Painter, s: &str, font: FontId, sp: f32) -> f32 {
    let mut job = LayoutJob::default();
    job.append(s, 0.0, TextFormat { font_id: font, extra_letter_spacing: sp, ..Default::default() });
    p.layout_job(job).size().x
}

// ---------------------------------------------------------------- colour + geometry

pub const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}
/// Same colour at `alpha` (0..1). Only for opaque inputs.
pub fn al(c: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}
pub fn mix_col(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}
/// Knob angle for a normalized value: -135..+135 degrees from straight up.
pub fn ang(v: f32) -> f32 {
    (-135.0 + v * 270.0 - 90.0f32).to_radians()
}
pub fn polar(c: Pos2, a: f32, r: f32) -> Pos2 {
    c + vec2(a.cos(), a.sin()) * r
}
pub fn arc_pts(c: Pos2, r: f32, a0: f32, a1: f32, n: usize) -> Vec<Pos2> {
    (0..=n).map(|k| polar(c, a0 + (a1 - a0) * k as f32 / n as f32, r)).collect()
}
pub fn line(p: &Painter, a: Pos2, b: Pos2, col: Color32, w: f32) {
    p.line_segment([a, b], Stroke::new(w, col));
}
pub fn dashed(p: &Painter, a: Pos2, b: Pos2, col: Color32, w: f32, dash: f32, gap: f32) {
    p.extend(Shape::dashed_line(&[a, b], Stroke::new(w, col), dash, gap));
}
pub fn polyline(p: &Painter, pts: Vec<Pos2>, col: Color32, w: f32, glow: bool) {
    if glow {
        p.add(Shape::line(pts.clone(), Stroke::new(w + 7.0_f32, al(col, 0.07))));
        p.add(Shape::line(pts.clone(), Stroke::new(w + 3.0_f32, al(col, 0.18))));
    }
    p.add(Shape::line(pts, Stroke::new(w, col)));
}
/// Fills the area between a curve and a horizontal baseline.
pub fn fill_under(p: &Painter, pts: &[Pos2], base: f32, col: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut m = Mesh::default();
    for w in pts.windows(2) {
        let i = m.vertices.len() as u32;
        m.colored_vertex(w[0], col);
        m.colored_vertex(w[1], col);
        m.colored_vertex(pos2(w[1].x, base), col);
        m.colored_vertex(pos2(w[0].x, base), col);
        m.add_triangle(i, i + 1, i + 2);
        m.add_triangle(i, i + 2, i + 3);
    }
    p.add(Shape::mesh(m));
}
/// Vertical gradient rect.
pub fn vgrad(p: &Painter, r: Rect, top: Color32, bot: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.right_bottom(), bot);
    m.colored_vertex(r.left_bottom(), bot);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}
pub fn hgrad(p: &Painter, r: Rect, left: Color32, right: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(r.left_top(), left);
    m.colored_vertex(r.right_top(), right);
    m.colored_vertex(r.right_bottom(), right);
    m.colored_vertex(r.left_bottom(), left);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}
/// Disc with a radial gradient (inner colour at `c + off`).
pub fn disc_radial(p: &Painter, c: Pos2, r: f32, inner: Color32, outer: Color32, off: Vec2) {
    let mut m = Mesh::default();
    m.colored_vertex(c + off, inner);
    let n = 48u32;
    for i in 0..=n {
        let t = i as f32 / n as f32 * TAU;
        m.colored_vertex(polar(c, t, r), outer);
    }
    for i in 0..n {
        m.add_triangle(0, i + 1, i + 2);
    }
    p.add(Shape::mesh(m));
}
/// Disc with a conic (angle) gradient through `stops` (brushed metal caps).
pub fn disc_conic(p: &Painter, c: Pos2, r: f32, stops: &[Color32], rot: f32) {
    let mut m = Mesh::default();
    let avg = stops.iter().fold([0u32; 3], |a, s| [a[0] + s.r() as u32, a[1] + s.g() as u32, a[2] + s.b() as u32]);
    let n_s = stops.len() as u32;
    m.colored_vertex(c, rgb((avg[0] / n_s) as u8, (avg[1] / n_s) as u8, (avg[2] / n_s) as u8));
    let n = 64u32;
    for i in 0..=n {
        let u = i as f32 / n as f32;
        let f = u * (stops.len() - 1) as f32;
        let k = (f as usize).min(stops.len() - 2);
        let col = mix_col(stops[k], stops[k + 1], f - k as f32);
        m.colored_vertex(polar(c, u * TAU + rot, r), col);
    }
    for i in 0..n {
        m.add_triangle(0, i + 1, i + 2);
    }
    p.add(Shape::mesh(m));
}
/// Soft drop shadow under a disc.
pub fn disc_shadow(p: &Painter, c: Pos2, r: f32, off: Vec2, alpha: f32) {
    for i in 0..6 {
        let t = i as f32 / 6.0;
        p.circle_filled(c + off, r + i as f32 * 1.4, Color32::from_black_alpha((alpha * (1.0 - t) * 70.0) as u8));
    }
}
/// Cut-corner outline: top-left cut `k`, bottom-right cut `k2`.
pub fn cham_pts(r: Rect, k: f32, k2: f32) -> Vec<Pos2> {
    let mut v = vec![pos2(r.left() + k, r.top()), r.right_top()];
    if k2 > 0.0 {
        v.push(pos2(r.right(), r.bottom() - k2));
        v.push(pos2(r.right() - k2, r.bottom()));
    } else {
        v.push(r.right_bottom());
    }
    v.push(r.left_bottom());
    if k > 0.0 {
        v.push(pos2(r.left(), r.top() + k));
    }
    v
}
pub fn cham_fill(p: &Painter, r: Rect, k: f32, k2: f32, fill: Color32, stroke: Stroke) {
    let pts = cham_pts(r, k, k2);
    p.add(Shape::convex_polygon(pts.clone(), fill, Stroke::NONE));
    if stroke.width > 0.0 {
        p.add(Shape::closed_line(pts, stroke));
    }
}

/// Log frequency position 0..1 over 20 Hz .. 20 kHz.
pub fn fu(f: f32) -> f32 {
    (f / 20.0).max(1e-6).ln() / 1000f32.ln()
}
pub const FREQ_TICKS: [(f32, &str); 8] = [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10000.0, "10k")];

// ---------------------------------------------------------------- the skin

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    Blackface,
    Hud,
    Machined,
}

/// HUD line colour (same on every HUD plugin; the accent is per plugin).
pub const CY: Color32 = rgb(0x3f, 0xd0, 0xff);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    None,
}

/// Colours for drawing inside a screen.
#[derive(Clone, Copy)]
pub struct Pal {
    pub grid: Color32,
    pub grid_hi: Color32,
    pub axis: Color32,
    pub trace: Color32,
    pub txt: Color32,
    pub alt: Color32,
    pub acc: Color32,
    pub glow: bool,
}
impl Pal {
    pub fn fill(&self, a: f32) -> Color32 {
        al(self.acc, a)
    }
}

pub struct Skin {
    pub look: Look,
    pub acc: Color32,
    pub name: &'static str,
    pub ver: &'static str,
    pub ticker: &'static str,
    pub time: f64,
    brushed: Option<TextureHandle>,
    peaks: HashMap<&'static str, (f32, f64)>,
}

impl Skin {
    pub fn new(look: Look, acc: Color32, name: &'static str, ver: &'static str, ticker: &'static str) -> Self {
        Self { look, acc, name, ver, ticker, time: 0.0, brushed: None, peaks: HashMap::new() }
    }

    /// Call once per frame before drawing.
    pub fn begin(&mut self, ctx: &egui::Context) {
        self.time = ctx.input(|i| i.time);
        if self.look == Look::Blackface && self.brushed.is_none() {
            self.brushed = Some(ctx.load_texture("awk-brushed", brushed_image(), TextureOptions::LINEAR));
        }
    }

    // Base colours per look.
    pub fn text_hi(&self) -> Color32 {
        match self.look {
            Look::Blackface => rgb(0xec, 0xee, 0xf0),
            Look::Hud => rgb(0xe6, 0xf8, 0xff),
            Look::Machined => rgb(0xe6, 0xe9, 0xec),
        }
    }
    pub fn text_dim(&self) -> Color32 {
        match self.look {
            Look::Blackface => rgb(0xa3, 0xa7, 0xad),
            Look::Hud => rgb(0x8f, 0xc9, 0xe3),
            Look::Machined => rgb(0x8a, 0x91, 0x9b),
        }
    }
    pub fn text_faint(&self) -> Color32 {
        match self.look {
            Look::Blackface => rgb(0x6f, 0x73, 0x7a),
            Look::Hud => al(rgb(0x8f, 0xc9, 0xe3), 0.55),
            Look::Machined => rgb(0x6c, 0x73, 0x7c),
        }
    }
    /// Section title font for this look.
    pub fn title_font(&self) -> (FontId, f32) {
        match self.look {
            Look::Blackface => (labb(13.0), 3.0),
            Look::Hud => (disp(8.5), 2.0),
            Look::Machined => (labb(13.0), 2.5),
        }
    }
    /// Small UI text (footer, notes).
    pub fn small(&self, p: &Painter, pos: Pos2, a: Align2, s: impl AsRef<str>, col: Color32) -> Rect {
        match self.look {
            Look::Hud => txt(p, pos, a, s, mono(10.0), col, 1.2),
            _ => txt(p, pos, a, s, lab(12.5), col, 1.3),
        }
    }

    // ------------------------------------------------------------ backgrounds

    pub fn background(&self, p: &Painter, r: Rect) {
        match self.look {
            Look::Blackface => {
                p.rect_filled(r, 0.0, rgb(0x1b, 0x1c, 0x1f));
                if let Some(t) = &self.brushed {
                    p.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                vignette(p, r, 0.38);
            }
            Look::Hud => {
                p.rect_filled(r, 0.0, rgb(0x04, 0x07, 0x0c));
                let mut x = r.left();
                let mut i = 0;
                while x <= r.right() {
                    let c = if i % 5 == 0 { al(CY, 0.085) } else { al(CY, 0.035) };
                    line(p, pos2(x.round() + 0.5, r.top()), pos2(x.round() + 0.5, r.bottom()), c, 1.0);
                    x += 20.0;
                    i += 1;
                }
                let mut y = r.top();
                i = 0;
                while y <= r.bottom() {
                    let c = if i % 5 == 0 { al(CY, 0.085) } else { al(CY, 0.035) };
                    line(p, pos2(r.left(), y.round() + 0.5), pos2(r.right(), y.round() + 0.5), c, 1.0);
                    y += 20.0;
                    i += 1;
                }
                vignette(p, r, 0.5);
                let mut y = r.top();
                while y < r.bottom() {
                    p.rect_filled(Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), 1.0)), 0.0, Color32::from_white_alpha(3));
                    y += 3.0;
                }
            }
            Look::Machined => {
                p.rect_filled(r, 0.0, rgb(0x1c, 0x1f, 0x23));
            }
        }
    }

    // ------------------------------------------------------------ panels

    /// Height a panel's title takes at its top.
    pub const TITLE_H: f32 = 28.0;

    pub fn panel(&self, p: &Painter, r: Rect, title: &str, code: &str) {
        match self.look {
            Look::Blackface => {
                p.rect_filled(r, 0.0, Color32::from_black_alpha(72));
                p.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 1.0)), 0.0, Color32::from_black_alpha(180));
                p.rect_filled(Rect::from_min_size(r.min, vec2(1.0, r.height())), 0.0, Color32::from_black_alpha(180));
                p.rect_filled(Rect::from_min_size(r.left_bottom(), vec2(r.width() + 1.0, 1.0)), 0.0, Color32::from_white_alpha(18));
                p.rect_filled(Rect::from_min_size(r.right_top(), vec2(1.0, r.height())), 0.0, Color32::from_white_alpha(18));
                if !title.is_empty() {
                    self.engraved_title(p, title, pos2(r.center().x, r.top() + 18.0), r.width());
                }
            }
            Look::Hud => {
                let k = 10.0;
                cham_fill(p, r, k, k, Color32::from_rgba_unmultiplied(6, 18, 30, 184), Stroke::new(1.0_f32, al(CY, 0.42)));
                p.add(Shape::line(vec![pos2(r.left(), r.top() + k + 14.0), pos2(r.left(), r.top() + k), pos2(r.left() + k, r.top()), pos2(r.left() + k + 26.0, r.top())], Stroke::new(2.0_f32, CY)));
                for i in 0..5 {
                    let y = r.bottom() - 30.0 + i as f32 * 4.0;
                    line(p, pos2(r.right() - 6.0, y), pos2(r.right() - 2.0, y), al(CY, 0.5), 1.0);
                }
                if !code.is_empty() {
                    txt(p, pos2(r.right() - 10.0, r.top() + 6.0), Align2::RIGHT_TOP, code, mono(8.0), al(CY, 0.55), 1.0);
                }
                if !title.is_empty() {
                    txt(p, pos2(r.left() + 16.0, r.top() + 18.0), Align2::LEFT_BOTTOM, title, disp(8.5), rgb(0xcd, 0xef, 0xff), 2.0);
                }
            }
            Look::Machined => {
                p.rect_filled(r, 7.0, rgb(0x24, 0x28, 0x2d));
                p.rect_stroke(r, 7.0, Stroke::new(1.0_f32, Color32::from_black_alpha(115)), StrokeKind::Outside);
                p.line_segment([pos2(r.left() + 6.0, r.top() + 0.5), pos2(r.right() - 6.0, r.top() + 0.5)], Stroke::new(1.0_f32, Color32::from_white_alpha(13)));
                if !title.is_empty() {
                    txt(p, pos2(r.left() + 12.0, r.top() + 19.0), Align2::LEFT_BOTTOM, title, labb(13.0), rgb(0xc9, 0xce, 0xd5), 2.5);
                }
            }
        }
    }

    fn engraved_title(&self, p: &Painter, s: &str, c: Pos2, w: f32) {
        let tr = txt(p, c, Align2::CENTER_BOTTOM, s, labb(12.5), rgb(0xe8, 0xe9, 0xeb), 3.0);
        let y = tr.center().y + 1.0;
        for (a, b) in [(c.x - w / 2.0 + 8.0, tr.left() - 8.0), (tr.right() + 6.0, c.x + w / 2.0 - 8.0)] {
            if b > a {
                p.rect_filled(Rect::from_min_max(pos2(a, y), pos2(b, y + 1.0)), 0.0, Color32::from_black_alpha(150));
                p.rect_filled(Rect::from_min_max(pos2(a, y + 1.0), pos2(b, y + 2.0)), 0.0, Color32::from_white_alpha(20));
            }
        }
    }

    /// Engraved vertical divider (Blackface module strips).
    pub fn divider(&self, p: &Painter, x: f32, y0: f32, y1: f32) {
        match self.look {
            Look::Blackface => {
                p.rect_filled(Rect::from_min_max(pos2(x, y0), pos2(x + 1.0, y1)), 0.0, Color32::from_black_alpha(165));
                p.rect_filled(Rect::from_min_max(pos2(x + 1.0, y0), pos2(x + 2.0, y1)), 0.0, Color32::from_white_alpha(15));
            }
            Look::Hud => line(p, pos2(x, y0), pos2(x, y1), al(CY, 0.18), 1.0),
            Look::Machined => line(p, pos2(x, y0), pos2(x, y1), Color32::from_black_alpha(90), 1.0),
        }
    }

    // ------------------------------------------------------------ screens

    /// Draws a display screen and returns a painter clipped to it plus its colours.
    pub fn screen(&self, p: &Painter, r: Rect) -> (Painter, Pal) {
        let a = self.acc;
        match self.look {
            Look::Blackface => {
                p.rect_filled(r, 4.0, rgb(0x0a, 0x0c, 0x0b));
                (
                    p.with_clip_rect(r.shrink(1.0)),
                    Pal { grid: al(a, 0.08), grid_hi: al(a, 0.16), axis: al(a, 0.6), trace: a, txt: al(a, 0.92), alt: Color32::from_white_alpha(100), acc: a, glow: true },
                )
            }
            Look::Hud => {
                self.panel(p, r, "", "");
                cham_fill(p, r.shrink(1.0), 13.0, 9.0, Color32::from_rgba_unmultiplied(3, 9, 16, 140), Stroke::NONE);
                (
                    p.with_clip_rect(r.shrink(1.0)),
                    Pal { grid: al(CY, 0.075), grid_hi: al(CY, 0.2), axis: al(rgb(0x8f, 0xc9, 0xe3), 0.8), trace: a, txt: rgb(0xcd, 0xef, 0xff), alt: al(CY, 0.55), acc: a, glow: true },
                )
            }
            Look::Machined => {
                cham_fill(p, r, 16.0, 0.0, rgb(0x0b, 0x0d, 0x10), Stroke::new(1.0_f32, rgb(0x2c, 0x30, 0x36)));
                (
                    p.with_clip_rect(r.shrink(1.0)),
                    Pal { grid: Color32::from_white_alpha(12), grid_hi: Color32::from_white_alpha(26), axis: rgb(0x6c, 0x73, 0x7c), trace: rgb(0xf3, 0xf4, 0xf6), txt: rgb(0xe6, 0xe9, 0xec), alt: Color32::from_white_alpha(76), acc: a, glow: false },
                )
            }
        }
    }

    pub fn screen_end(&self, p: &Painter, r: Rect) {
        match self.look {
            Look::Blackface => {
                let c = p.with_clip_rect(r);
                vgrad(&c, Rect::from_min_max(r.min, pos2(r.right(), r.top() + r.height() * 0.45)), Color32::from_white_alpha(10), Color32::TRANSPARENT);
                p.rect_stroke(r, 4.0, Stroke::new(2.0_f32, Color32::from_black_alpha(230)), StrokeKind::Outside);
                let col = Color32::from_rgba_unmultiplied(220, 224, 228, 115);
                for (x, y, sx, sy) in [(r.left() - 5.0, r.top() - 5.0, 1.0, 1.0), (r.right() + 5.0, r.top() - 5.0, -1.0, 1.0), (r.left() - 5.0, r.bottom() + 5.0, 1.0, -1.0), (r.right() + 5.0, r.bottom() + 5.0, -1.0, -1.0)] {
                    p.add(Shape::line(vec![pos2(x, y + sy * 10.0), pos2(x, y), pos2(x + sx * 10.0, y)], Stroke::new(1.0_f32, col)));
                }
            }
            Look::Hud => {}
            Look::Machined => {
                p.add(Shape::line(vec![pos2(r.left(), r.top() + 30.0), pos2(r.left(), r.top() + 16.0), pos2(r.left() + 16.0, r.top()), pos2(r.left() + 46.0, r.top())], Stroke::new(2.0_f32, self.acc)));
            }
        }
    }

    // ------------------------------------------------------------ knobs

    /// A knob for any parameter. Drag up/right (Ctrl or Shift = fine), scroll to nudge,
    /// double-click to reset. `bip` = the value ring grows from the centre.
    #[allow(clippy::too_many_arguments)]
    pub fn knob<P: Param>(&self, ui: &mut egui::Ui, setter: &ParamSetter, c: Pos2, r: f32, label: &str, param: &P, big: bool, bip: bool, dim: bool) -> Response {
        let id = ui.id().with(("awk-knob", label, (c.x * 4.0) as i32, (c.y * 4.0) as i32));
        let resp = ui.interact(Rect::from_center_size(c, Vec2::splat(r * 2.0 + 14.0)), id, Sense::click_and_drag());
        knob_input(ui, setter, &resp, id, param);
        let v = param.unmodulated_normalized_value();
        let hot = resp.hovered() || resp.dragged();
        let p = ui.painter().clone();
        let p = if dim { p.with_clip_rect(p.clip_rect()) } else { p };
        let fade = |col: Color32| if dim { col.gamma_multiply(0.38) } else { col };
        let value = param.normalized_value_to_string(v, true);
        let a = ang(v);
        match self.look {
            Look::Blackface => {
                let n = if big { 11 } else { 7 };
                for i in 0..n {
                    let aa = ang(i as f32 / (n - 1) as f32);
                    line(&p, polar(c, aa, r + 4.0), polar(c, aa, r + if big { 9.0 } else { 7.0 }), fade(Color32::from_rgba_unmultiplied(225, 228, 232, 180)), 1.2);
                    if big && i % 2 == 0 {
                        txt(&p, polar(c, aa, r + 16.0), Align2::CENTER_CENTER, format!("{i}"), lab(9.5), fade(Color32::from_rgba_unmultiplied(225, 228, 232, 165)), 0.0);
                    }
                }
                disc_shadow(&p, c, r, vec2(2.0, 4.0), 0.8);
                disc_radial(&p, c, r, fade(rgb(0x40, 0x41, 0x45)), fade(rgb(0x11, 0x12, 0x14)), vec2(-r * 0.3, -r * 0.45));
                for i in 0..64 {
                    let aa = i as f32 / 64.0 * TAU;
                    let col = if i % 2 == 1 { Color32::from_white_alpha(22) } else { Color32::from_black_alpha(128) };
                    line(&p, polar(c, aa, r * 0.84), polar(c, aa, r * 0.99), col, 1.0);
                }
                let cap = [0x9a, 0xec, 0x85, 0xda, 0x7a, 0xe6, 0x8b, 0xe0, 0x9a].map(|g: u8| fade(rgb(g, g.saturating_add(3), g.saturating_add(6))));
                disc_conic(&p, c, r * 0.6, &cap, a);
                p.circle_stroke(c, r * 0.6, Stroke::new(1.0_f32, Color32::from_black_alpha(140)));
                if hot {
                    p.circle_stroke(c, r + 1.5, Stroke::new(1.0_f32, al(self.acc, 0.6)));
                }
                line(&p, polar(c, a, r * 0.64), polar(c, a, r * 0.95), fade(rgb(0xf4, 0xf4, 0xf4)), if big { 2.6 } else { 2.0 });
                line(&p, polar(c, a, r * 0.2), polar(c, a, r * 0.55), Color32::from_black_alpha(200), 1.6);
                let ly = c.y + r + if big { 22.0 } else { 11.0 };
                txt(&p, pos2(c.x, ly), Align2::CENTER_TOP, label, labb(if big { 13.0 } else { 11.5 }), fade(rgb(0xec, 0xee, 0xf0)), 2.0);
                txt(&p, pos2(c.x, ly + if big { 15.0 } else { 13.0 }), Align2::CENTER_TOP, &value, mono(10.5), fade(self.acc), 0.0);
            }
            Look::Hud => {
                let n = if big { 31 } else { 21 };
                let span = 270f32.to_radians() / (n - 1) as f32;
                for i in 0..n {
                    let u = i as f32 / (n - 1) as f32;
                    let lit = if bip { if v >= 0.5 { u >= 0.5 && u <= v + 1e-4 } else { u <= 0.5 && u >= v - 1e-4 } } else { u <= v + 1e-4 };
                    let pts = arc_pts(c, r, ang(u) - span * 0.32, ang(u) + span * 0.32, 4);
                    let w = if big { 5.0 } else { 4.0 };
                    if lit {
                        p.add(Shape::line(pts.clone(), Stroke::new(w + 5.0_f32, fade(al(self.acc, 0.16)))));
                        p.add(Shape::line(pts, Stroke::new(w, fade(self.acc))));
                    } else {
                        p.add(Shape::line(pts, Stroke::new(w, al(CY, 0.13))));
                    }
                }
                p.circle_filled(c, r - 7.0, Color32::from_rgba_unmultiplied(3, 10, 18, 242));
                p.circle_stroke(c, r - 7.0, Stroke::new(1.0_f32, al(CY, if hot { 0.8 } else { 0.35 })));
                line(&p, polar(c, a, r - 15.0), polar(c, a, r - 8.0), fade(Color32::WHITE), 2.0);
                if big {
                    let rot = (self.time as f32 * 0.35) * if (c.x as i32) % 2 == 0 { 1.0 } else { -1.0 };
                    let pts = arc_pts(c, r + 7.0, rot, rot + TAU * 0.7, 48);
                    p.extend(Shape::dashed_line(&pts, Stroke::new(1.0_f32, al(CY, 0.4)), 2.0, 7.0));
                    txt(&p, c, Align2::CENTER_CENTER, format!("{:03}", (v * 100.0).round() as i32), mono(if r > 26.0 { 14.0 } else { 12.5 }), fade(rgb(0xea, 0xfa, 0xff)), 1.0);
                }
                let ly = c.y + r + if big { 12.0 } else { 9.0 };
                txt(&p, pos2(c.x, ly), Align2::CENTER_TOP, label, disp(if big { 8.0 } else { 7.0 }), fade(rgb(0x8f, 0xc9, 0xe3)), 1.0);
                txt(&p, pos2(c.x, ly + 13.0), Align2::CENTER_TOP, &value, mono(10.5), fade(self.acc), 0.0);
            }
            Look::Machined => {
                let tr = r + 5.0;
                p.add(Shape::line(arc_pts(c, tr, ang(0.0), ang(1.0), 48), Stroke::new(if big { 4.0_f32 } else { 3.0_f32 }, rgb(0x15, 0x17, 0x1a))));
                let a0 = if bip { ang(0.5) } else { ang(0.0) };
                if (a - a0).abs() > 0.01 {
                    let pts = arc_pts(c, tr, a0.min(a), a0.max(a), 40);
                    p.add(Shape::line(pts.clone(), Stroke::new(if big { 9.0_f32 } else { 7.0_f32 }, fade(al(self.acc, 0.14)))));
                    p.add(Shape::line(pts, Stroke::new(if big { 4.0_f32 } else { 3.0_f32 }, fade(self.acc))));
                }
                disc_shadow(&p, c, r, vec2(1.0, 3.0), 0.75);
                p.circle_filled(c, r, rgb(0x0f, 0x10, 0x12));
                let cap = [0xa6, 0xe2, 0x9a, 0xd4, 0x95, 0xdf, 0xa1, 0xd8, 0xa6].map(|g: u8| fade(rgb(g, g.saturating_add(3), g.saturating_add(7))));
                disc_conic(&p, c, r - 2.5, &cap, 0.0);
                disc_radial(&p, c, r - 2.5, Color32::from_white_alpha(if hot { 70 } else { 50 }), Color32::from_black_alpha(40), vec2(-r * 0.3, -r * 0.4));
                line(&p, polar(c, a, r * 0.3), polar(c, a, r - 5.0), rgb(0x1a, 0x1c, 0x1f), if big { 3.0 } else { 2.4 });
                let ly = c.y + r + if big { 11.0 } else { 9.0 };
                txt(&p, pos2(c.x, ly), Align2::CENTER_TOP, label, lab(if big { 13.0 } else { 11.5 }), fade(rgb(0x9a, 0xa1, 0xaa)), 1.2);
                txt(&p, pos2(c.x, ly + if big { 15.0 } else { 13.0 }), Align2::CENTER_TOP, &value, mono(10.5), fade(rgb(0xee, 0xf0, 0xf2)), 0.0);
            }
        }
        resp
    }

    /// Small header knob (no label underneath; the caller writes it beside).
    pub fn mini_knob<P: Param>(&self, ui: &mut egui::Ui, setter: &ParamSetter, c: Pos2, r: f32, param: &P) -> Response {
        let id = ui.id().with(("awk-mini", param.name()));
        let resp = ui.interact(Rect::from_center_size(c, Vec2::splat(r * 2.0 + 10.0)), id, Sense::click_and_drag());
        knob_input(ui, setter, &resp, id, param);
        let v = param.unmodulated_normalized_value();
        let a = ang(v);
        let hot = resp.hovered() || resp.dragged();
        let p = ui.painter();
        let track = match self.look {
            Look::Hud => al(CY, 0.15),
            _ => rgb(0x15, 0x17, 0x1a),
        };
        p.add(Shape::line(arc_pts(c, r + 4.0, ang(0.0), ang(1.0), 40), Stroke::new(3.0_f32, track)));
        if v > 0.002 {
            p.add(Shape::line(arc_pts(c, r + 4.0, ang(0.0), a, 40), Stroke::new(3.0_f32, self.acc)));
        }
        match self.look {
            Look::Hud => {
                p.circle_filled(c, r, Color32::from_rgba_unmultiplied(3, 10, 18, 242));
                p.circle_stroke(c, r, Stroke::new(1.0_f32, al(CY, if hot { 0.8 } else { 0.4 })));
                line(p, polar(c, a, r * 0.3), polar(c, a, r - 2.0), Color32::WHITE, 2.0);
            }
            _ => {
                p.circle_filled(c, r, rgb(0x0f, 0x10, 0x12));
                let cap = [0xa6, 0xe2, 0x9a, 0xd4, 0x95, 0xdf, 0xa1, 0xd8, 0xa6].map(|g: u8| rgb(g, g.saturating_add(3), g.saturating_add(7)));
                disc_conic(p, c, r - 1.5, &cap, a);
                if hot {
                    p.circle_stroke(c, r + 1.0, Stroke::new(1.0_f32, al(self.acc, 0.7)));
                }
                line(p, polar(c, a, r * 0.25), polar(c, a, r - 2.5), rgb(0x1a, 0x1c, 0x1f), 2.2);
            }
        }
        resp
    }

    /// Header power button bound to the bypass param (lit = plugin is ON).
    pub fn power_icon(&self, ui: &mut egui::Ui, setter: &ParamSetter, r: Rect, bypass: &BoolParam) -> Response {
        let resp = ui.interact(r, ui.id().with("awk-bypass"), Sense::click());
        let on = !bypass.value();
        if resp.clicked() {
            setter.begin_set_parameter(bypass);
            setter.set_parameter(bypass, on);
            setter.end_set_parameter(bypass);
        }
        let p = ui.painter();
        let c = r.center();
        let rad = r.height() * 0.5;
        match self.look {
            Look::Hud => cham_fill(p, r, 5.0, 5.0, if on { al(self.acc, 0.18) } else { al(CY, 0.04) }, Stroke::new(1.0_f32, if on { self.acc } else { al(CY, 0.35) })),
            Look::Blackface => {
                p.rect_filled(r, 2.0, if on { rgb(0x2a, 0x2b, 0x2e) } else { rgb(0x14, 0x14, 0x16) });
                p.rect_stroke(r, 2.0, Stroke::new(1.0_f32, rgb(0x36, 0x37, 0x3b)), StrokeKind::Inside);
            }
            Look::Machined => {
                p.rect_filled(r, rad, if on { al(self.acc, 0.16) } else { rgb(0x1d, 0x20, 0x24) });
                p.rect_stroke(r, rad, Stroke::new(1.0_f32, if on { al(self.acc, 0.65) } else { rgb(0x2f, 0x34, 0x3a) }), StrokeKind::Inside);
            }
        }
        let col = if on { self.acc } else { self.text_faint() };
        let ir = rad * 0.5;
        let gap = 0.7f32;
        let up = -std::f32::consts::FRAC_PI_2;
        p.add(Shape::line(arc_pts(c, ir, up + gap, up + TAU - gap, 32), Stroke::new(1.8_f32, col)));
        line(p, c + vec2(0.0, -ir - 2.0), c + vec2(0.0, -ir * 0.15), col, 1.8);
        if resp.hovered() {
            resp.clone().on_hover_text(if on { "ON (click to bypass)" } else { "BYPASSED (click to turn on)" });
        }
        resp
    }

    // ------------------------------------------------------------ chips + buttons

    /// A clickable chip (tabs, mode buttons, toggles). Returns the click response.
    pub fn chip(&self, ui: &mut egui::Ui, r: Rect, s: &str, on: bool, id: impl Hash) -> Response {
        let resp = ui.interact(r, ui.id().with(("awk-chip", id)), Sense::click());
        let hot = resp.hovered();
        let p = ui.painter();
        match self.look {
            Look::Blackface => {
                p.rect_filled(r, 2.0, if on { rgb(0x2a, 0x2b, 0x2e) } else if hot { rgb(0x1e, 0x1f, 0x22) } else { rgb(0x14, 0x14, 0x16) });
                p.rect_stroke(r, 2.0, Stroke::new(1.0_f32, if on { rgb(0x4a, 0x4c, 0x50) } else { rgb(0x2e, 0x2f, 0x33) }), StrokeKind::Inside);
                let mut cx = r.center().x;
                if on {
                    let led = pos2(r.left() + 8.0, r.center().y);
                    p.circle_filled(led, 5.0, al(self.acc, 0.2));
                    p.circle_filled(led, 2.6, self.acc);
                    cx += 4.0;
                }
                txt(p, pos2(cx, r.center().y), Align2::CENTER_CENTER, s, labb(11.5), if on { rgb(0xf0, 0xf0, 0xf0) } else { rgb(0x8d, 0x90, 0x96) }, 1.2);
            }
            Look::Hud => {
                cham_fill(p, r, 5.0, 5.0, if on { al(self.acc, 0.2) } else if hot { al(CY, 0.1) } else { al(CY, 0.04) }, Stroke::new(1.0_f32, if on { self.acc } else { al(CY, 0.35) }));
                txt(p, r.center(), Align2::CENTER_CENTER, s, mono(10.5), if on { Color32::WHITE } else { rgb(0x8f, 0xc9, 0xe3) }, 1.0);
            }
            Look::Machined => {
                let rad = r.height() / 2.0;
                p.rect_filled(r, rad, if on { al(self.acc, 0.16) } else if hot { rgb(0x23, 0x27, 0x2c) } else { rgb(0x1d, 0x20, 0x24) });
                p.rect_stroke(r, rad, Stroke::new(1.0_f32, if on { al(self.acc, 0.65) } else { rgb(0x2f, 0x34, 0x3a) }), StrokeKind::Inside);
                txt(p, r.center(), Align2::CENTER_CENTER, s, lab(11.5), if on { self.acc } else { rgb(0x8a, 0x91, 0x9b) }, 1.2);
            }
        }
        resp
    }

    /// A chip bound to a BoolParam (click toggles it).
    pub fn chip_param(&self, ui: &mut egui::Ui, setter: &ParamSetter, r: Rect, param: &BoolParam, labels: (&str, &str)) -> Response {
        let on = param.value();
        let resp = self.chip(ui, r, if on { labels.0 } else { labels.1 }, on, (param.name(), r.left() as i32, r.top() as i32));
        if resp.clicked() {
            setter.begin_set_parameter(param);
            setter.set_parameter(param, !on);
            setter.end_set_parameter(param);
        }
        resp
    }

    /// Module power switch (Blackface: jewel LED, Hud: ON/OFF tag, Machined: slide switch).
    pub fn power(&self, ui: &mut egui::Ui, setter: &ParamSetter, r: Rect, param: &BoolParam) -> Response {
        let on = param.value();
        let resp = ui.interact(r.expand(3.0), ui.id().with(("awk-pwr", param.name(), r.left() as i32, r.top() as i32)), Sense::click());
        if resp.clicked() {
            setter.begin_set_parameter(param);
            setter.set_parameter(param, !on);
            setter.end_set_parameter(param);
        }
        let p = ui.painter();
        match self.look {
            Look::Blackface => {
                let c = r.center();
                let rad = r.height().min(r.width()) / 2.0;
                disc_radial(p, c, rad, rgb(0xcf, 0xd2, 0xd6), rgb(0x55, 0x58, 0x5d), vec2(0.0, -rad * 0.6));
                if on {
                    p.circle_filled(c, rad * 1.3, al(self.acc, 0.18));
                }
                p.circle_filled(c, rad * 0.62, if on { self.acc } else { rgb(0x2a, 0x1a, 0x12) });
            }
            Look::Hud => {
                cham_fill(p, r, 4.0, 4.0, if on { al(self.acc, 0.25) } else { al(CY, 0.04) }, Stroke::new(1.0_f32, if on { self.acc } else { al(CY, 0.35) }));
                txt(p, r.center(), Align2::CENTER_CENTER, if on { "ON" } else { "OFF" }, mono(9.0), if on { self.acc } else { rgb(0x5f, 0x8e, 0xa3) }, 1.0);
            }
            Look::Machined => {
                let rad = r.height() / 2.0;
                p.rect_filled(r, rad, if on { self.acc } else { rgb(0x15, 0x17, 0x1a) });
                let kx = if on { r.right() - rad } else { r.left() + rad };
                p.circle_filled(pos2(kx, r.center().y), rad - 2.0, if on { Color32::WHITE } else { rgb(0x5a, 0x60, 0x69) });
            }
        }
        resp
    }

    /// OVERSAMPLING: OFF / 2x / 4x / 8x chips bound to an IntParam (0..3).
    pub fn os_selector(&self, ui: &mut egui::Ui, setter: &ParamSetter, at: Pos2, param: &IntParam, cpu_base: f32) {
        let p = ui.painter().clone();
        let lr = self.small(&p, pos2(at.x, at.y + 8.5), Align2::LEFT_CENTER, "OVERSAMPLING", self.text_dim());
        let mut x = lr.right() + 10.0;
        let cur = param.value();
        for (i, s) in OS_NAMES.iter().enumerate() {
            let r = Rect::from_min_size(pos2(x, at.y), vec2(34.0, 17.0));
            let shown = if i == 0 { "OFF".to_string() } else { s.replace('x', "×") };
            if self.chip(ui, r, &shown, cur == i as i32, ("os", i)).clicked() {
                setter.begin_set_parameter(param);
                setter.set_parameter(param, i as i32);
                setter.end_set_parameter(param);
            }
            x += 38.0;
        }
        let f = [1.0, 2.0, 4.0, 8.0][cur.clamp(0, 3) as usize];
        self.small(&p, pos2(x + 6.0, at.y + 8.5), Align2::LEFT_CENTER, format!("CPU ~{:.1}%", cpu_base * f), self.text_faint());
    }

    /// SAFE chip for the footer: the output limiter switch, with how hard it's working.
    pub fn safe_chip(&self, ui: &mut egui::Ui, setter: &ParamSetter, r: Rect, param: &BoolParam, gr_db: f32) {
        let on = param.value();
        let s = if !on { "SAFE OFF".to_string() } else if gr_db < -0.1 { format!("SAFE {:.1} dB", gr_db) } else { "SAFE 0 dB".to_string() };
        if self.chip(ui, r, &s, on, "safe").on_hover_text("Output limiter: never goes above 0 dB. Leave on.").clicked() {
            setter.begin_set_parameter(param);
            setter.set_parameter(param, !on);
            setter.end_set_parameter(param);
        }
        if !on {
            let p = ui.painter();
            p.rect_stroke(r.expand(1.0), 3.0, Stroke::new(1.0_f32, rgb(0xff, 0x4a, 0x3d)), StrokeKind::Outside);
        }
    }

    // ------------------------------------------------------------ footer

    /// Footer band. Returns the band rect. HUD gets a status ticker from `ticker_x`.
    pub fn footer(&self, p: &Painter, r: Rect, ticker_x: f32) {
        match self.look {
            Look::Blackface => {
                p.rect_filled(r, 0.0, rgb(0x0c, 0x0c, 0x0d));
                p.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 1.0)), 0.0, Color32::from_white_alpha(13));
                txt(p, pos2(r.right() - 16.0, r.center().y), Align2::RIGHT_CENTER, format!("AWK STUDIOS  ·  v{}", self.ver), mono(10.0), rgb(0x6f, 0x73, 0x7a), 1.0);
            }
            Look::Hud => {
                p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(2, 6, 10, 235));
                line(p, pos2(r.left(), r.top() + 0.5), pos2(r.right(), r.top() + 0.5), al(CY, 0.5), 1.0);
                let msg = format!("{}.VST3 // BUILD {} // {} // AWK STUDIOS // ", self.name, self.ver, self.ticker);
                let clip = Rect::from_min_max(pos2(ticker_x, r.top()), pos2(r.right() - 90.0, r.bottom()));
                if clip.width() > 40.0 {
                    let cp = p.with_clip_rect(clip);
                    let w = txt_w(&cp, &msg, mono(10.0), 2.0).max(1.0);
                    let off = -((self.time as f32 * 30.0) % w);
                    for k in 0..3 {
                        txt(&cp, pos2(clip.left() + off + k as f32 * w, r.center().y), Align2::LEFT_CENTER, &msg, mono(10.0), al(rgb(0x9f, 0xdc, 0xf5), 0.7), 2.0);
                    }
                }
                txt(p, pos2(r.right() - 16.0, r.center().y), Align2::RIGHT_CENTER, format!("v{}", self.ver), mono(10.0), rgb(0x9f, 0xdc, 0xf5), 2.0);
            }
            Look::Machined => {
                p.rect_filled(r, 0.0, rgb(0x13, 0x15, 0x18));
                txt(p, pos2(r.right() - 16.0, r.center().y), Align2::RIGHT_CENTER, format!("AWK STUDIOS  ·  v{}", self.ver), mono(10.0), rgb(0x6c, 0x73, 0x7c), 1.0);
            }
        }
    }

    // ------------------------------------------------------------ meters

    /// One L/R meter with a dB scale (+6 .. -60) and a held peak readout on top.
    #[allow(clippy::too_many_arguments)]
    pub fn meter(&mut self, p: &Painter, x: f32, y: f32, h: f32, label: &str, l: f32, r: f32, side: Side, key: &'static str) {
        let map = |d: f32| y + h - ((d.max(-60.0).min(6.0)) + 60.0) / 66.0 * h;
        self.bar(p, Rect::from_min_size(pos2(x, y), vec2(6.0, h)), l, &map);
        self.bar(p, Rect::from_min_size(pos2(x + 8.0, y), vec2(6.0, h)), r, &map);
        let (txt_c, dim_c, box_c, box_l) = self.meter_cols();
        for d in [6.0f32, 0.0, -6.0, -12.0, -18.0, -24.0, -36.0, -48.0, -60.0] {
            let yy = map(d).round() + 0.5;
            let s = if d > 0.0 { format!("+{d:.0}") } else { format!("{d:.0}") };
            let c = if d == 0.0 { txt_c } else { dim_c };
            match side {
                Side::Right => {
                    line(p, pos2(x + 16.0, yy), pos2(x + 19.0, yy), dim_c, 1.0);
                    txt(p, pos2(x + 21.0, yy), Align2::LEFT_CENTER, s, mono(8.5), c, 0.0);
                }
                Side::Left => {
                    line(p, pos2(x - 5.0, yy), pos2(x - 2.0, yy), dim_c, 1.0);
                    txt(p, pos2(x - 7.0, yy), Align2::RIGHT_CENTER, s, mono(8.5), c, 0.0);
                }
                Side::None => {}
            }
        }
        let pk = l.max(r);
        let now = self.time;
        let e = self.peaks.entry(key).or_insert((-120.0, 0.0));
        if pk > e.0 || now - e.1 > 1.0 {
            *e = (pk, now);
        }
        let held = e.0;
        let hot = held > -0.05;
        let br = Rect::from_min_size(pos2(x - 9.0, y - 22.0), vec2(32.0, 15.0));
        p.rect_filled(br, 1.0, box_c);
        p.rect_stroke(br, 1.0, Stroke::new(1.0_f32, if hot { rgb(0xff, 0x4a, 0x3d) } else { box_l }), StrokeKind::Inside);
        let s = if held < -99.0 { "-inf".to_string() } else { format!("{held:+.1}") };
        txt(p, br.center(), Align2::CENTER_CENTER, s, mono(10.0), if hot { rgb(0xff, 0x6a, 0x5a) } else { txt_c }, 0.0);
        txt(p, pos2(x + 7.0, y + h + 6.0), Align2::CENTER_TOP, label, mono(9.0), dim_c, 1.5);
    }

    fn meter_cols(&self) -> (Color32, Color32, Color32, Color32) {
        match self.look {
            Look::Blackface => (rgb(0xd9, 0xdc, 0xe0), rgb(0x80, 0x84, 0x8a), rgb(0x0b, 0x0c, 0x0d), rgb(0x2d, 0x2e, 0x31)),
            Look::Hud => (rgb(0xdf, 0xf6, 0xff), al(rgb(0x8f, 0xc9, 0xe3), 0.75), Color32::from_rgba_unmultiplied(3, 10, 18, 230), al(CY, 0.45)),
            Look::Machined => (rgb(0xee, 0xf0, 0xf2), rgb(0x6c, 0x73, 0x7c), rgb(0x15, 0x17, 0x1a), rgb(0x2f, 0x34, 0x3a)),
        }
    }

    fn bar(&self, p: &Painter, r: Rect, db: f32, map: &dyn Fn(f32) -> f32) {
        match self.look {
            Look::Blackface => {
                let n = (r.height() / 6.0).floor() as i32;
                for i in 0..n {
                    let y1 = r.bottom() - (i + 1) as f32 * 6.0 + 1.0;
                    let seg = -60.0 + (i as f32 + 0.5) / n as f32 * 66.0;
                    let lit = seg <= db;
                    let col = if seg > 0.0 { rgb(0xff, 0x46, 0x36) } else if seg > -6.0 { rgb(0xff, 0xb0, 0x2e) } else { rgb(0x59, 0xd3, 0x6a) };
                    let sr = Rect::from_min_size(pos2(r.left(), y1), vec2(r.width(), 4.0));
                    if lit {
                        p.rect_filled(sr.expand(1.5), 1.0, al(col, 0.2));
                    }
                    p.rect_filled(sr, 0.0, if lit { col } else { Color32::from_white_alpha(13) });
                }
            }
            Look::Hud => {
                p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, al(CY, 0.3)), StrokeKind::Inside);
                let top = map(db);
                if top < r.bottom() - 1.0 {
                    let zero = map(0.0);
                    let fill = Rect::from_min_max(pos2(r.left() + 1.5, top.max(zero)), pos2(r.right() - 1.5, r.bottom() - 1.0));
                    p.rect_filled(fill.expand2(vec2(2.0, 0.0)), 0.0, al(CY, 0.15));
                    p.rect_filled(fill, 0.0, CY);
                    if top < zero {
                        p.rect_filled(Rect::from_min_max(pos2(r.left() + 1.5, top), pos2(r.right() - 1.5, zero)), 0.0, rgb(0xff, 0x5a, 0x4a));
                    }
                }
            }
            Look::Machined => {
                p.rect_filled(r, r.width() / 2.0, rgb(0x15, 0x17, 0x1a));
                let top = map(db);
                if top < r.bottom() - 1.0 {
                    let (z, w6) = (map(0.0), map(-6.0));
                    let seg = |a: f32, b: f32, c: Color32| {
                        let (a, b) = (a.max(top), b);
                        if b > a {
                            p.rect_filled(Rect::from_min_max(pos2(r.left(), a), pos2(r.right(), b)), 1.5, c);
                        }
                    };
                    seg(w6, r.bottom(), rgb(0x5f, 0xd1, 0x6a));
                    seg(z, w6, rgb(0xff, 0xb0, 0x2e));
                    seg(r.top(), z, rgb(0xff, 0x4a, 0x3d));
                }
            }
        }
    }

    // ------------------------------------------------------------ plot helpers

    /// Horizontal dB grid lines with labels at `lx` (right-aligned).
    pub fn db_axis(&self, p: &Painter, pal: &Pal, x0: f32, x1: f32, map: impl Fn(f32) -> f32, list: &[f32], lx: f32) {
        for &d in list {
            let yy = map(d).round() + 0.5;
            line(p, pos2(x0, yy), pos2(x1, yy), pal.grid, 1.0);
            let s = if d.is_infinite() { "-∞".to_string() } else if d > 0.0 { format!("+{d:.0}") } else { format!("{d:.0}") };
            txt(p, pos2(lx, yy), Align2::RIGHT_CENTER, s, mono(9.0), pal.axis, 0.0);
        }
    }
    /// Log frequency grid (20 Hz..20 kHz mapped by `mapx(u)`), labels at `ly`.
    pub fn freq_axis(&self, p: &Painter, pal: &Pal, mapx: impl Fn(f32) -> f32, y0: f32, y1: f32, ly: f32) {
        for (f, l) in FREQ_TICKS {
            let xx = mapx(fu(f)).round() + 0.5;
            line(p, pos2(xx, y0), pos2(xx, y1), pal.grid, 1.0);
            txt(p, pos2(xx, ly), Align2::CENTER_TOP, l, mono(9.0), pal.axis, 0.0);
        }
        txt(p, pos2(mapx(1.0), ly), Align2::RIGHT_TOP, "Hz", mono(9.0), pal.axis, 0.0);
    }
    /// Little boxed readout (used on screens).
    pub fn readout(&self, p: &Painter, r: Rect, s: &str, col: Color32) {
        match self.look {
            Look::Hud => cham_fill(p, r, 5.0, 5.0, Color32::from_rgba_unmultiplied(3, 10, 18, 235), Stroke::new(1.0_f32, col)),
            _ => {
                p.rect_filled(r, 2.0, Color32::from_black_alpha(170));
            }
        }
        txt(p, r.center(), Align2::CENTER_CENTER, s, mono(10.0), col, 0.0);
    }
}

fn vignette(p: &Painter, r: Rect, a: f32) {
    let d = Color32::from_black_alpha((a * 255.0) as u8);
    let t = Color32::TRANSPARENT;
    let e = 120.0f32.min(r.height() / 3.0);
    vgrad(p, Rect::from_min_max(r.min, pos2(r.right(), r.top() + e)), d, t);
    vgrad(p, Rect::from_min_max(pos2(r.left(), r.bottom() - e), r.max), t, d);
    hgrad(p, Rect::from_min_max(r.min, pos2(r.left() + e, r.bottom())), d, t);
    hgrad(p, Rect::from_min_max(pos2(r.right() - e, r.top()), r.max), t, d);
}

/// Procedural brushed-aluminium texture (dark anodised).
fn brushed_image() -> ColorImage {
    let (w, h) = (1024usize, 1024usize);
    let mut lum = vec![0f32; w * h];
    let mut seed = 7u32;
    let mut rnd = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    for _ in 0..14000 {
        let y = (rnd() * h as f32) as usize % h;
        let x0 = (rnd() * w as f32) as usize;
        let len = 30 + (rnd() * 260.0) as usize;
        let d = if rnd() < 0.5 { 2.0 + rnd() * 7.0 } else { -(4.0 + rnd() * 12.0) };
        for k in 0..len {
            lum[y * w + (x0 + k) % w] += d;
        }
    }
    let mut img = ColorImage::new([w, h], Color32::BLACK);
    for (i, px) in img.pixels.iter_mut().enumerate() {
        let v = lum[i];
        let c = |b: f32| (b + v).clamp(0.0, 255.0) as u8;
        *px = Color32::from_rgb(c(27.0), c(28.0), c(31.0));
    }
    img
}

/// Shared knob mouse handling: drag (Ctrl/Shift = fine), scroll, double-click reset.
pub fn knob_input<P: Param>(ui: &mut egui::Ui, setter: &ParamSetter, resp: &Response, id: egui::Id, param: &P) {
    let step = param.step_count().map(|n| 1.0 / n.max(1) as f32).unwrap_or(0.01);
    if resp.double_clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter_normalized(param, param.default_normalized_value());
        setter.end_set_parameter(param);
        return;
    }
    if resp.drag_started() {
        setter.begin_set_parameter(param);
        let start = param.unmodulated_normalized_value();
        ui.memory_mut(|m| m.data.insert_temp(id, (start, 0.0f32)));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.ctrl || i.modifiers.command || i.modifiers.shift);
        let speed = if fine { 0.0005 } else { 0.005 };
        let d = resp.drag_delta();
        let (start, mut acc) = ui.memory(|m| m.data.get_temp::<(f32, f32)>(id)).unwrap_or((param.unmodulated_normalized_value(), 0.0));
        acc += (d.x - d.y) * speed;
        ui.memory_mut(|m| m.data.insert_temp(id, (start, acc)));
        setter.set_parameter_normalized(param, (start + acc).clamp(0.0, 1.0));
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(param);
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            let n = (param.unmodulated_normalized_value() + scroll.signum() * step).clamp(0.0, 1.0);
            setter.begin_set_parameter(param);
            setter.set_parameter_normalized(param, n);
            setter.end_set_parameter(param);
        }
    }
}

#[cfg(test)]
mod ceiling_tests {
    use super::*;
    /// Nothing gets past -0.1 dBFS, whatever goes in, and quiet audio is untouched.
    #[test]
    fn never_above_ceiling() {
        let mut c = Ceiling::default();
        c.set_sr(48000.0);
        let mut worst = 0.0f32;
        for n in 0..96000 {
            let t = n as f32 / 48000.0;
            let amp = if n % 24000 < 4000 { 40.0 } else { 0.3 };
            let x = amp * (6.283 * 80.0 * t).sin();
            let (l, r) = c.tick(x, -0.7 * x, true);
            worst = worst.max(l.abs()).max(r.abs());
        }
        assert!(worst <= CEILING + 1e-6, "peak {worst}");
        let mut c = Ceiling::default();
        c.set_sr(48000.0);
        for n in 0..1000 {
            let x = 0.5 * (n as f32 * 0.01).sin();
            assert_eq!(c.tick(x, x, true), (x, x));
        }
        assert_eq!(c.tick(f32::NAN, f32::INFINITY, true), (0.0, 0.0));
    }
}
