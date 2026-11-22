//! WaveTag editor, awk studios "Vector HUD" look (blue accent).
//! Header: presets, A/B, undo/redo, power (bypass), MIX.
//! - Shape chips: 13 classic + 8 BASS. Click one to pick it.
//! - THE WAVE: the real gain curve over one bar (beat grid) with the current
//!   knobs, gain axis in dB, right channel dashed when WIDTH > 0, live playhead
//!   and the live L/R gain in dB.
//! - WAVE panel: RATE (RATE HZ when free), DEPTH, BEND, SMOOTH, PHASE, WIDTH, OUTPUT.
//! Knobs: drag (Ctrl or Shift = fine), scroll to nudge, double-click to reset.

use crate::awk_ui::*;
use crate::presets::PresetBar;
use crate::{shape_at, Shared, WaveTagParams, RATE_BEATS, RATE_NAMES, SHAPE_NAMES};
use nih_plug::prelude::*;
use nih_plug_egui::create_egui_editor;
use nih_plug_egui::egui::{self, pos2, vec2, Align2, Color32, Pos2, Rect, Shape, Stroke};
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub const WIDTH: u32 = 960;
pub const HEIGHT: u32 = 676;
const ACC: Color32 = rgb(0x3f, 0xd0, 0xff);

struct St {
    presets: PresetBar,
    skin: Skin,
}

pub fn create(params: Arc<WaveTagParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    let state = params.editor_state.clone();
    let st = St {
        presets: PresetBar::new("WaveTag"),
        skin: Skin::new(Look::Hud, ACC, "WAVETAG", env!("CARGO_PKG_VERSION"), "LFO // TREMOLO // CHOP // HOST-SYNCED // ZERO-X CLICK-FREE"),
    };
    create_egui_editor(
        state,
        st,
        |ctx, _| install_fonts(ctx),
        move |ctx, setter, st| frame(ctx, setter, st, &params, &shared),
    )
}

/// 13 classic shapes on the top row, the 8 BASS shapes below.
fn shape_chips(ui: &mut egui::Ui, setter: &ParamSetter, skin: &Skin, r: Rect, param: &IntParam) {
    use crate::shapes::CLASSIC;
    let gap = 6.0;
    let rh = (r.height() - 8.0) / 2.0;
    let cur = param.value();
    let label_w = 58.0;
    skin.small(ui.painter(), pos2(r.left() + 2.0, r.top() + rh + 8.0 + rh / 2.0), Align2::LEFT_CENTER, "BASS ›", skin.text_dim());
    for (i, name) in SHAPE_NAMES.iter().enumerate() {
        let (row, col, n, x0) = if i < CLASSIC { (0, i, CLASSIC, r.left()) } else { (1, i - CLASSIC, SHAPE_NAMES.len() - CLASSIC, r.left() + label_w) };
        let w = (r.right() - x0 - gap * (n - 1) as f32) / n as f32;
        let cell = Rect::from_min_size(pos2(x0 + col as f32 * (w + gap), r.top() + row as f32 * (rh + 8.0)), vec2(w, rh));
        if skin.chip(ui, cell, name, cur == i as i32, ("shape", i)).clicked() && cur != i as i32 {
            setter.begin_set_parameter(param);
            setter.set_parameter(param, i as i32);
            setter.end_set_parameter(param);
        }
    }
}

/// Linear gain -> y on the wave screen.
fn gain_db_label(g: f32) -> String {
    if g <= 1e-5 {
        "-∞".into()
    } else {
        let d = 20.0 * g.log10();
        if d.abs() < 0.05 {
            "0 dB".into()
        } else {
            format!("{d:.0} dB")
        }
    }
}

fn draw_wave(painter: &egui::Painter, skin: &Skin, r: Rect, p: &WaveTagParams, shared: &Shared) {
    let (sp, pal) = skin.screen(painter, r);
    let (gl, gr, top, base) = (r.left() + 54.0, r.right() - 14.0, r.top() + 36.0, r.bottom() - 26.0);
    let py = |g: f32| base - g.clamp(0.0, 1.0) * (base - top);
    // Gain axis: dB marks placed on linear gain (that's what the plugin multiplies by).
    for (d, g) in [(0.0f32, 1.0f32), (-3.0, 0.708), (-6.0, 0.501), (-12.0, 0.251), (-24.0, 0.063)] {
        let y = py(g).round() + 0.5;
        line(&sp, pos2(gl, y), pos2(gr, y), pal.grid, 1.0);
        txt(&sp, pos2(gl - 6.0, y), Align2::RIGHT_CENTER, if d == 0.0 { "0 dB".to_string() } else { format!("{d:.0} dB") }, mono(9.0), pal.axis, 0.0);
    }
    line(&sp, pos2(gl, base.round() + 0.5), pos2(gr, base.round() + 0.5), pal.grid, 1.0);
    txt(&sp, pos2(gl - 6.0, base), Align2::RIGHT_CENTER, "-∞", mono(9.0), pal.axis, 0.0);

    // Time span: one bar (4 beats) when synced, one second when free.
    let sync = p.sync.value();
    let rate_i = (p.rate.value().round() as usize).min(17);
    let cycles = if sync { 4.0 / RATE_BEATS[rate_i] as f32 } else { p.hz.value() };
    let ticks = 16;
    for b in 0..=ticks {
        let x = (gl + b as f32 / ticks as f32 * (gr - gl)).round() + 0.5;
        let major = b % 4 == 0;
        line(&sp, pos2(x, top), pos2(x, base), if major { pal.grid_hi } else { pal.grid }, 1.0);
        if major && b < ticks {
            let lab = if sync { format!("BEAT {}", b / 4 + 1) } else { format!("{:.2} s", b as f32 / ticks as f32) };
            txt(&sp, pos2(x + 4.0, base + 4.0), Align2::LEFT_TOP, lab, mono(9.0), pal.axis, 0.0);
        }
    }

    let kind = p.shape.value().clamp(0, crate::shapes::LAST_SHAPE) as usize;
    let (depth, bend) = (p.depth.value(), p.bend.value());
    let ph0 = p.phase.value() / 360.0;
    let wd = p.width.value() / 360.0;
    let gain = |u: f32, off: f32| 1.0 - depth * shape_at(kind, (u * cycles + ph0 + off).rem_euclid(1.0), bend);
    let n = ((gr - gl) as usize).max(2);
    let pts: Vec<Pos2> = (0..=n).map(|i| {
        let u = i as f32 / n as f32;
        pos2(gl + u * (gr - gl), py(gain(u, 0.0)))
    }).collect();
    fill_under(&sp, &pts, base, pal.fill(0.12));
    polyline(&sp, pts, pal.trace, 2.0, true);
    if wd > 0.001 {
        let pr: Vec<Pos2> = (0..=n).map(|i| {
            let u = i as f32 / n as f32;
            pos2(gl + u * (gr - gl), py(gain(u, wd)))
        }).collect();
        sp.extend(Shape::dashed_line(&pr, Stroke::new(1.2_f32, Color32::from_rgba_unmultiplied(220, 245, 255, 150)), 5.0, 5.0));
    }

    // Playhead: the live phase (inside the cycle) shown on every cycle in view.
    let ph_l = f32::from_bits(shared.phase_l.load(Ordering::Relaxed));
    let g_l = f32::from_bits(shared.gain_l.load(Ordering::Relaxed));
    let g_r = f32::from_bits(shared.gain_r.load(Ordering::Relaxed));
    let within = (ph_l - ph0).rem_euclid(1.0);
    if cycles > 0.0 && cycles <= 16.0 {
        let mut k = 0.0;
        let mut first = None;
        while k < cycles.ceil() {
            let u = (k + within) / cycles;
            if u <= 1.0 {
                let x = gl + u * (gr - gl);
                line(&sp, pos2(x, top - 4.0), pos2(x, base), Color32::from_rgba_unmultiplied(220, 245, 255, if k == 0.0 { 210 } else { 90 }), 1.0);
                first.get_or_insert(x);
            }
            k += 1.0;
        }
        if let Some(x) = first {
            sp.circle_stroke(pos2(x, py(g_l)), 5.0, Stroke::new(1.5_f32, Color32::WHITE));
            let bx = (x + 10.0).min(gr - 140.0);
            let br = Rect::from_min_size(pos2(bx, top + 4.0), vec2(132.0, 36.0));
            cham_fill(&sp, br, 5.0, 5.0, Color32::from_rgba_unmultiplied(3, 10, 18, 235), Stroke::new(1.0_f32, pal.trace));
            txt(&sp, pos2(br.left() + 8.0, br.top() + 6.0), Align2::LEFT_TOP, format!("L {}", gain_db_label(g_l)), mono(10.5), rgb(0xea, 0xfa, 0xff), 0.0);
            txt(&sp, pos2(br.left() + 8.0, br.top() + 20.0), Align2::LEFT_TOP, format!("R {}", gain_db_label(g_r)), mono(10.5), Color32::from_rgba_unmultiplied(220, 245, 255, 180), 0.0);
        }
    }

    let rate_s = if sync { RATE_NAMES[rate_i].to_string() } else { p.hz.normalized_value_to_string(p.hz.unmodulated_normalized_value(), true) };
    txt(&sp, pos2(r.left() + 22.0, r.top() + 10.0), Align2::LEFT_TOP, format!("THE WAVE // {} @ {}", SHAPE_NAMES[kind], rate_s), disp(8.5), pal.txt, 2.0);
    let legend = if wd > 0.001 { "— L   - - R" } else { "— L = R" };
    let span = if sync { "1 BAR" } else { "1 SECOND" };
    txt(&sp, pos2(gr, r.top() + 10.0), Align2::RIGHT_TOP, format!("{span} · TOP = FULL VOLUME · {legend}"), mono(9.0), pal.axis, 1.0);
    skin.screen_end(painter, r);
}

/// One editor frame (pulled out of the update closure so tests can render it).
fn frame(ctx: &egui::Context, setter: &ParamSetter, st: &mut St, params: &Arc<WaveTagParams>, shared: &Arc<Shared>) {
            // Animate only while something plays and the window has focus;
            // otherwise a slow 4 fps refresh (knob moves still repaint instantly).
            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
            if focused && shared.active.load(Ordering::Relaxed) {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
            st.skin.begin(ctx);
            let p = &params;
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(rgb(0x04, 0x07, 0x0c))).show(ctx, |ui| {
                let whole = ui.max_rect();
                st.skin.background(ui.painter(), whole);
                let full = st.presets.strip(ui, setter, params.as_ref(), whole, &st.skin, &p.mix, &p.bypass);
                let painter = ui.painter().clone();
                let skin = &mut st.skin;
                let (l, r, t) = (full.left(), full.right(), full.top());
                let foot = Rect::from_min_max(pos2(l, full.bottom() - 30.0), full.right_bottom());

                // ---- shape chips
                shape_chips(ui, setter, skin, Rect::from_min_max(pos2(l + 14.0, t + 12.0), pos2(r - 14.0, t + 68.0)), &p.shape);

                // ---- THE WAVE
                let meters = Rect::from_min_max(pos2(r - 88.0, t + 80.0), pos2(r - 14.0, foot.top() - 10.0));
                let wave = Rect::from_min_max(pos2(l + 14.0, t + 80.0), pos2(meters.left() - 12.0, t + 376.0));
                draw_wave(&painter, skin, wave, p, &shared);

                // ---- WAVE knobs
                let kp = Rect::from_min_max(pos2(wave.left(), wave.bottom() + 12.0), pos2(wave.right(), foot.top() - 10.0));
                skin.panel(&painter, kp, "WAVE", "SEC.02");
                let chips: [(&BoolParam, (&str, &str), f32); 3] = [
                    (&p.sync, ("RATE: TEMPO SYNC", "RATE: FREE HZ"), 140.0),
                    (&p.retrig, ("TRIG: MIDI NOTE", "TRIG: LOCKED TO BEAT"), 164.0),
                    (&p.zero_x, ("ZERO-X: ON", "ZERO-X: OFF"), 104.0),
                ];
                let mut cx = kp.left() + 90.0;
                for (prm, labels, w) in chips {
                    skin.chip_param(ui, setter, Rect::from_min_size(pos2(cx, kp.top() + 6.0), vec2(w, 18.0)), prm, labels);
                    cx += w + 8.0;
                }
                let sw = kp.width() / 7.0;
                let ky = kp.top() + 78.0;
                let kx = |i: usize| pos2(kp.left() + sw * (i as f32 + 0.5), ky);
                if p.sync.value() {
                    skin.knob(ui, setter, kx(0), 28.0, "RATE", &p.rate, true, false, false);
                } else {
                    skin.knob(ui, setter, kx(0), 28.0, "RATE HZ", &p.hz, true, false, false);
                }
                skin.knob(ui, setter, kx(1), 28.0, "DEPTH", &p.depth, true, false, false);
                skin.knob(ui, setter, kx(2), 28.0, "BEND", &p.bend, true, true, false);
                skin.knob(ui, setter, kx(3), 28.0, "SMOOTH", &p.smooth, true, false, false);
                skin.knob(ui, setter, kx(4), 28.0, "PHASE", &p.phase, true, false, false);
                skin.knob(ui, setter, kx(5), 28.0, "WIDTH", &p.width, true, false, false);
                skin.knob(ui, setter, kx(6), 28.0, "OUTPUT", &p.output, true, false, false);

                // ---- meters
                skin.panel(&painter, meters, "", "");
                let m = &shared.meters;
                let (my, mh) = (meters.top() + 40.0, meters.height() - 64.0);
                skin.meter(&painter, meters.left() + 10.0, my, mh, "IN", m.db(0), m.db(1), Side::Right, "in");
                skin.meter(&painter, meters.left() + 52.0, my, mh, "OUT", m.db(2), m.db(3), Side::None, "out");

                // ---- footer
                skin.footer(&painter, foot, foot.left() + 140.0);
                skin.safe_chip(ui, setter, Rect::from_center_size(pos2(foot.left() + 72.0, foot.center().y), vec2(112.0, 18.0)), &p.safe, shared.meters.db(4));
            });
        }

#[cfg(test)]
mod shot_tests {
    use super::*;
    use crate::shot::*;
    use std::collections::HashMap;
    /// SHOT_DIR=/src/target/shots cargo test --lib screenshots  ->  wavetag0.rgb (raw RGB, WIDTH x HEIGHT)
    #[test]
    fn screenshots() {
        let Ok(dir) = std::env::var("SHOT_DIR") else { return };
        let plug = crate::WaveTag::default();
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let mock = Mock;
        let setter = ParamSetter::new(&mock);
        let mut st = St { presets: PresetBar::new("WaveTagShot"), skin: Skin::new(Look::Hud, ACC, "WAVETAG", env!("CARGO_PKG_VERSION"), "LFO // TREMOLO // CHOP // HOST-SYNCED // ZERO-X CLICK-FREE") };
        let mut texs = HashMap::new();
        let mut out = None;
        for i in 0..4 {
            out = Some(ctx.run(raw(i, vec![]), |c| frame(c, &setter, &mut st, &plug.params, &plug.shared)));
            if i < 3 { apply_delta(&mut texs, out.as_ref().unwrap()); }
        }
        render(&ctx, out.unwrap(), &mut texs, &format!("{dir}/wavetag0.rgb"));
    }
}
