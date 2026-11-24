//! Header bar + presets, shared by every awk studios plugin (same file in each project;
//! master copy in ~/Projects/awk-ui/presets.rs).
//!
//! Presets are the plugin's full state (every knob, switch and saved path)
//! as JSON in Documents\Awknode\Presets\<Plugin>\<name>.awkp, so they are
//! shared by every project. INIT puts every control back to its default.
//!
//! Header: AWK STUDIOS + name | [<] [ preset v ] [>] | SAVE  SAVE AS | A B | undo redo | power | MIX
//! Click the name for the full list (click a star to favourite). Your own presets
//! live in <Plugin>\User and are favourited when saved. SAVE updates your preset
//! (or saves a copy of a factory one), SAVE AS asks for a name: Enter or the SAVE
//! button saves, clicking anywhere else cancels (Esc closes FL's window, so it
//! isn't needed).
//! A/B: two settings to flip between. Undo/redo: every change (Ctrl+Z / Ctrl+Shift+Z).

use crate::awk_ui::*;
use nice_plug::prelude::*;
use nice_plug::plugin::{ParamValue, PluginState};
use egui::{pos2, vec2, Align2, Color32, Id, Key, Order, Rect, Sense, Shape, Stroke, StrokeKind};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const INIT: &str = "INIT";
/// Height of the header across the top of every plugin window.
pub const STRIP: f32 = 44.0;
const EXT: &str = "awkp";
/// Your own presets go in <Plugin>\User and show up as "USER - name".
const USER_DIR: &str = "User";
const USER_G: &str = "USER - ";
const FAV_FILE: &str = "favorites.txt";
const HISTORY: usize = 100;

pub fn dir(plugin: &str) -> PathBuf {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    PathBuf::from(home).join("Documents").join("Awknode").join("Presets").join(plugin)
}

/// Window sizes in the SIZE dropdown. 100% is the designed (smallest) size.
pub const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];
/// Size a plugin opens at until that instance has its own saved size.
pub const DEFAULT_SCALE: f32 = 1.25;

/// Before the editor opens: use this instance's saved size, else the default.
pub fn apply_scale(state: &nice_plug_egui::EguiEditorState, saved: &std::sync::RwLock<f32>) {
    let s = saved.read().map(|v| *v).unwrap_or(0.0);
    state.set_zoom_factor(if s > 0.0 { s } else { DEFAULT_SCALE });
}

pub struct PresetBar {
    plugin: &'static str,
    list: Vec<String>,
    scanned: bool,
    pub current: String,
    dirty_check: Option<Vec<f32>>,
    open: bool,
    size_open: bool,
    naming: Option<String>,
    /// True on the frame the name box opened (so the click that opened it can't close it).
    naming_new: bool,
    favs: Vec<String>,
    msg: Option<(String, f64)>,
    /// Persisted non-knob data from the last loaded preset (e.g. AlleyVerb's
    /// impulse path). The editor takes it and applies it.
    pub pending_fields: Option<BTreeMap<String, String>>,
    history: Vec<Vec<f32>>,
    hist_pos: usize,
    ab: [Option<Vec<f32>>; 2],
    ab_on: usize,
    /// Dice button: Some(ids it leaves alone). None = no dice on this plugin.
    dice: Option<&'static [&'static str]>,
}

impl PresetBar {
    pub fn new(plugin: &'static str) -> Self {
        Self {
            plugin,
            list: Vec::new(),
            scanned: false,
            current: INIT.into(),
            dirty_check: None,
            open: false,
            size_open: false,
            naming: None,
            naming_new: false,
            favs: Vec::new(),
            msg: None,
            pending_fields: None,
            history: Vec::new(),
            hist_pos: 0,
            dice: None,
            ab: [None, None],
            ab_on: 0,
        }
    }

    fn rescan(&mut self) {
        let scan = |d: PathBuf| -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(d)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case(EXT)).unwrap_or(false))
                        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                        .collect()
                })
                .unwrap_or_default();
            v.sort_by_key(|s| s.to_lowercase());
            v
        };
        let base = dir(self.plugin);
        let user = scan(base.join(USER_DIR)).into_iter().map(|n| format!("{USER_G}{n}"));
        self.list = user.chain(scan(base.clone())).collect();
        self.favs = std::fs::read_to_string(base.join(FAV_FILE)).map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
        self.scanned = true;
    }

    /// File for a list name ("USER - x" lives in the User folder).
    fn path_of(&self, name: &str) -> PathBuf {
        let base = dir(self.plugin);
        match name.strip_prefix(USER_G) {
            Some(n) => base.join(USER_DIR).join(format!("{n}.{EXT}")),
            None => base.join(format!("{name}.{EXT}")),
        }
    }

    fn set_fav(&mut self, name: &str, on: bool) {
        self.favs.retain(|f| f != name);
        if on {
            self.favs.push(name.to_string());
        }
        let d = dir(self.plugin);
        let _ = std::fs::create_dir_all(&d).and_then(|_| std::fs::write(d.join(FAV_FILE), self.favs.join("\n")));
    }

    fn open_naming(&mut self, start: String) {
        self.naming = Some(start);
        self.naming_new = true;
        self.open = false;
    }

    fn is_bypass(p: &ParamPtr) -> bool {
        unsafe { p.flags() }.contains(ParamFlags::BYPASS)
    }

    /// Every control's normalized value (the power button is left out).
    fn snapshot(params: &dyn Params) -> Vec<f32> {
        params.param_map().iter().map(|(_, p, _)| if Self::is_bypass(p) { 0.0 } else { unsafe { p.unmodulated_normalized_value() } }).collect()
    }

    fn apply(setter: &ParamSetter, params: &dyn Params, snap: &[f32]) {
        for ((_, p, _), v) in params.param_map().iter().zip(snap) {
            if Self::is_bypass(p) {
                continue;
            }
            unsafe {
                if (p.unmodulated_normalized_value() - v).abs() > 1e-7 {
                    setter.raw_context.raw_begin_set_parameter(*p);
                    setter.raw_context.raw_set_parameter_normalized(*p, *v);
                    setter.raw_context.raw_end_set_parameter(*p);
                }
            }
        }
    }

    /// Turn on the dice button. `keep` = parameter ids it never changes
    /// (also matched as a suffix "_<id>" for nested groups).
    pub fn with_dice(mut self, keep: &'static [&'static str]) -> Self {
        self.dice = Some(keep);
        self
    }

    /// Dice: click = new random patch, Shift+click = nudge every control a little.
    /// Undo takes it back (the change lands in the history like any other).
    fn roll(&mut self, ui: &egui::Ui, setter: &ParamSetter, params: &dyn Params, keep: &[&str], gentle: bool) {
        let mut s: u64 = (ui.input(|i| i.time).to_bits() ^ 0x9E37_79B9_7F4A_7C15) | 1;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 40) as f32 / (1u64 << 24) as f32
        };
        for (id, p, _) in params.param_map() {
            if Self::is_bypass(&p) || unsafe { p.flags() }.contains(ParamFlags::HIDDEN) {
                continue;
            }
            if keep.iter().any(|k| id == *k || id.ends_with(&format!("_{k}"))) {
                continue;
            }
            unsafe {
                let cur = p.unmodulated_normalized_value();
                let mut n = if gentle { (cur + (rnd() - 0.5) * 0.3).clamp(0.0, 1.0) } else { rnd() };
                if let Some(steps) = p.step_count() {
                    if steps > 0 {
                        n = (n * steps as f32).round() / steps as f32;
                    }
                }
                setter.raw_context.raw_begin_set_parameter(p);
                setter.raw_context.raw_set_parameter_normalized(p, n);
                setter.raw_context.raw_end_set_parameter(p);
            }
        }
        self.say(ui, if gentle { "NUDGED  ·  undo to go back" } else { "RANDOM  ·  undo to go back" });
    }

    /// The dice button: a rounded square with five pips.
    fn dice_btn(&self, ui: &mut egui::Ui, r: Rect, skin: &Skin) -> egui::Response {
        let resp = ui.interact(r, ui.scope_id().with("pb-dice"), Sense::click())
            .on_hover_text("Random patch  (Shift+click: small nudge)");
        let col = if resp.hovered() { skin.acc } else { skin.text_dim() };
        let p = ui.painter();
        let b = Rect::from_center_size(r.center(), vec2(15.0, 15.0));
        p.rect_stroke(b, 3.0, Stroke::new(1.5_f32, col), egui::StrokeKind::Inside);
        for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (0.0, 0.0), (-1.0, 1.0), (1.0, 1.0)] {
            p.circle_filled(b.center() + vec2(dx * 3.6, dy * 3.6), 1.4, col);
        }
        resp
    }

    fn say(&mut self, ui: &egui::Ui, text: impl Into<String>) {
        self.msg = Some((text.into(), ui.input(|i| i.time) + 2.5));
    }

    fn load(&mut self, ui: &egui::Ui, setter: &ParamSetter, params: &dyn Params, name: &str) {
        if name == INIT {
            for (_, p, _) in params.param_map() {
                if Self::is_bypass(&p) {
                    continue;
                }
                unsafe {
                    setter.raw_context.raw_begin_set_parameter(p);
                    setter.raw_context.raw_set_parameter_normalized(p, p.default_normalized_value());
                    setter.raw_context.raw_end_set_parameter(p);
                }
            }
            self.current = INIT.into();
            self.dirty_check = None;
            return;
        }
        let path = self.path_of(name);
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| serde_json::from_str::<PluginState>(&s).map_err(|e| e.to_string())) {
            Ok(state) => {
                // Set each control the way a knob move does. (Restoring the whole
                // state from the GUI blocks until the audio thread answers, and
                // FL froze when that plugin's audio was paused.)
                for (id, ptr, _) in params.param_map() {
                    if Self::is_bypass(&ptr) {
                        continue;
                    }
                    // A control the preset doesn't mention (one added after it was
                    // saved, like MIX) goes to its default, so a preset always sounds the same.
                    let plain = match state.params.get(&id) {
                        Some(ParamValue::F32(v)) => Some(*v),
                        Some(ParamValue::I32(v)) => Some(*v as f32),
                        Some(ParamValue::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
                        _ => None,
                    };
                    unsafe {
                        let n = match plain {
                            Some(v) => ptr.preview_normalized(v),
                            None => ptr.default_normalized_value(),
                        };
                        setter.raw_context.raw_begin_set_parameter(ptr);
                        setter.raw_context.raw_set_parameter_normalized(ptr, n);
                        setter.raw_context.raw_end_set_parameter(ptr);
                    }
                }
                self.pending_fields = Some(state.fields);
                self.current = name.to_string();
                self.dirty_check = None;
            }
            Err(e) => self.say(ui, format!("CAN'T LOAD: {e}")),
        }
    }

    fn save(&mut self, ui: &egui::Ui, setter: &ParamSetter, params: &dyn Params, name: &str) {
        let name: String = name.chars().filter(|c| !"\\/:*?\"<>|".contains(*c)).collect::<String>().trim().to_string();
        if name.is_empty() || name == INIT {
            self.say(ui, "PICK A NAME");
            return;
        }
        // Always into the User folder, and straight into the favourites.
        let d = dir(self.plugin).join(USER_DIR);
        let state = setter.raw_context.get_state();
        let res = std::fs::create_dir_all(&d)
            .map_err(|e| e.to_string())
            .and_then(|_| serde_json::to_string_pretty(&state).map_err(|e| e.to_string()))
            .and_then(|json| std::fs::write(d.join(format!("{name}.{EXT}")), json).map_err(|e| e.to_string()));
        match res {
            Ok(()) => {
                let key = format!("{USER_G}{name}");
                self.current = key.clone();
                self.dirty_check = Some(Self::snapshot(params));
                self.rescan();
                self.set_fav(&key, true);
                self.say(ui, format!("SAVED  {name}  ★"));
            }
            Err(e) => self.say(ui, format!("CAN'T SAVE: {e}")),
        }
    }

    /// Records a new undo step once a change has settled (mouse released).
    fn track_history(&mut self, ui: &egui::Ui, snap: &[f32]) {
        if ui.input(|i| i.pointer.any_down()) {
            return;
        }
        if self.history.is_empty() {
            self.history.push(snap.to_vec());
            self.hist_pos = 0;
            return;
        }
        if self.history[self.hist_pos] != snap {
            self.history.truncate(self.hist_pos + 1);
            self.history.push(snap.to_vec());
            if self.history.len() > HISTORY {
                self.history.remove(0);
            }
            self.hist_pos = self.history.len() - 1;
        }
    }

    fn undo(&mut self, setter: &ParamSetter, params: &dyn Params) {
        if self.hist_pos > 0 {
            self.hist_pos -= 1;
            Self::apply(setter, params, &self.history[self.hist_pos]);
        }
    }
    fn redo(&mut self, setter: &ParamSetter, params: &dyn Params) {
        if self.hist_pos + 1 < self.history.len() {
            self.hist_pos += 1;
            Self::apply(setter, params, &self.history[self.hist_pos]);
        }
    }

    fn switch_ab(&mut self, setter: &ParamSetter, params: &dyn Params, to: usize) {
        if to == self.ab_on {
            return;
        }
        self.ab[self.ab_on] = Some(Self::snapshot(params));
        if let Some(s) = self.ab[to].clone() {
            Self::apply(setter, params, &s);
        }
        self.ab_on = to;
    }

    /// Draws the header across the top of `whole` and returns the area left below
    /// it for the plugin's own UI (nothing can overlap the header).
    #[allow(clippy::too_many_arguments)]
    pub fn strip(&mut self, ui: &mut egui::Ui, setter: &ParamSetter, params: &dyn Params, whole: Rect, skin: &Skin, mix: &FloatParam, bypass: &BoolParam, ui_scale: &std::sync::RwLock<f32>) -> Rect {
        let band = Rect::from_min_max(whole.left_top(), pos2(whole.right(), whole.top() + STRIP));
        let p = ui.painter().clone();
        let (l, r, cy) = (band.left(), band.right(), band.center().y);
        // Band + brand, drawn by a closure: measured first on a painter clipped to nothing, then
        // drawn for real. The small AWK STUDIOS text is left out if the preset browser would get
        // narrower than 150 px (narrow plugins at the SIZE chip's extra width).
        let brand_right = std::cell::Cell::new(l);
        let tx = |p: &egui::Painter, pos: egui::Pos2, a: Align2, s: &str, f: egui::FontId, c: Color32, sp: f32| -> Rect {
            let rr = txt(p, pos, a, s, f, c, sp);
            brand_right.set(brand_right.get().max(rr.right()));
            rr
        };
        let draw_brand = |p: &egui::Painter, awk: bool| {
            brand_right.set(l);
            match skin.look {
                Look::Blackface => {
                    vgrad(&p, band, rgb(0x14, 0x14, 0x16), rgb(0x0b, 0x0b, 0x0c));
                    p.rect_filled(Rect::from_min_size(pos2(l, band.bottom() - 1.0), vec2(band.width(), 1.0)), 0.0, Color32::BLACK);
                    p.rect_filled(Rect::from_min_size(pos2(l, band.bottom()), vec2(band.width(), 1.0)), 0.0, Color32::from_white_alpha(15));
                    let br = if awk { tx(p, pos2(l + 18.0, cy), Align2::LEFT_CENTER, "AWK STUDIOS", disp(9.0), rgb(0xbf, 0xc3, 0xc8), 2.0) } else { Rect::from_min_size(pos2(l + 4.0, cy), vec2(0.0, 0.0)) };
                    tx(p, pos2(br.right() + 14.0, cy + 1.0), Align2::LEFT_CENTER, skin.name, labb(21.0), rgb(0xf2, 0xf2, 0xf2), 2.5);
                }
                Look::Hud => {
                    p.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(2, 6, 10, 235));
                    line(&p, pos2(l, band.bottom() - 0.5), pos2(r, band.bottom() - 0.5), al(CY, 0.5), 1.0);
                    let mut x = l;
                    let mut i = 0;
                    while x < r {
                        line(&p, pos2(x + 0.5, band.bottom()), pos2(x + 0.5, band.bottom() - if i % 5 == 0 { 6.0 } else { 3.0 }), al(CY, 0.5), 1.0);
                        x += 10.0;
                        i += 1;
                    }
                    if awk { tx(p, pos2(l + 18.0, band.top() + 8.0), Align2::LEFT_TOP, "AWK STUDIOS", disp(8.0), CY, 3.0); }
                    tx(p, pos2(l + 18.0, band.top() + 22.0), Align2::LEFT_TOP, &format!("//{}", skin.name), disp(11.0), rgb(0xf0, 0xfb, 0xff), 2.0);
                }
                Look::Machined => {
                    p.rect_filled(band, 0.0, rgb(0x13, 0x15, 0x18));
                    p.rect_filled(Rect::from_min_size(pos2(l, band.bottom() - 1.0), vec2(band.width(), 1.0)), 0.0, Color32::from_white_alpha(10));
                    let dot = pos2(l + 26.0, cy);
                    p.circle_filled(dot, 8.0, skin.acc);
                    p.circle_filled(dot, 3.5, rgb(0x13, 0x15, 0x18));
                    let nr = tx(p, pos2(l + 44.0, cy + 1.0), Align2::LEFT_CENTER, skin.name, labb(21.0), rgb(0xf1, 0xf3, 0xf5), 2.0);
                    if awk { tx(p, pos2(nr.right() + 12.0, cy + 1.0), Align2::LEFT_CENTER, "AWK STUDIOS", disp(8.0), rgb(0x6c, 0x73, 0x7c), 2.0); }
                }
            }
        };
        draw_brand(&p.clone().with_clip_rect(Rect::NOTHING), true);
        let awk = (r - 434.0 - 12.0) - (brand_right.get() + 14.0) >= 150.0;
        draw_brand(&p, awk);
        let brand_r = brand_right.get();

        // Right side, laid out from the right edge.
        let mix_c = pos2(r - 26.0, cy);
        skin.mini_knob(ui, setter, mix_c, 11.0, mix);
        txt(&p, pos2(mix_c.x - 20.0, cy - 1.0), Align2::RIGHT_BOTTOM, "MIX", labb(11.5), skin.text_hi(), 1.5);
        txt(&p, pos2(mix_c.x - 20.0, cy + 1.0), Align2::RIGHT_TOP, mix.normalized_value_to_string(mix.unmodulated_normalized_value(), true), mono(10.0), skin.acc, 0.0);
        skin.power_icon(ui, setter, Rect::from_center_size(pos2(r - 104.0, cy), vec2(26.0, 24.0)), bypass);

        let snap = Self::snapshot(params);
        self.track_history(ui, &snap);
        let (can_undo, can_redo) = (self.hist_pos > 0, self.hist_pos + 1 < self.history.len());
        let ur = Rect::from_center_size(pos2(r - 168.0, cy), vec2(24.0, 22.0));
        let rr = Rect::from_center_size(pos2(r - 140.0, cy), vec2(24.0, 22.0));
        if self.icon(ui, ur, skin, true, can_undo) {
            self.undo(setter, params);
        }
        if self.icon(ui, rr, skin, false, can_redo) {
            self.redo(setter, params);
        }
        let (z, shift) = ui.input(|i| (i.key_pressed(Key::Z) && (i.modifiers.ctrl || i.modifiers.command), i.modifiers.shift));
        if z && self.naming.is_none() {
            if shift {
                self.redo(setter, params);
            } else {
                self.undo(setter, params);
            }
        }
        // SIZE: dropdown of window sizes. Saved per instance (with the project).
        {
            let cur = ui.ctx().zoom_factor();
            let idx = SCALES.iter().position(|s| (s - cur).abs() < 0.01).unwrap_or(0);
            let sr = Rect::from_min_size(pos2(r - 234.0, cy - 10.0), vec2(46.0, 20.0));
            let resp = skin.chip(ui, sr, &format!("{:.0}%", SCALES[idx] * 100.0), self.size_open, "pb-size").on_hover_text("Window size");
            if resp.clicked() {
                self.size_open = !self.size_open;
            }
            if self.size_open {
                let (bg, edge, hi) = match skin.look {
                    Look::Hud => (Color32::from_rgba_unmultiplied(4, 10, 18, 250), CY, al(skin.acc, 0.28)),
                    Look::Blackface => (rgb(0x16, 0x16, 0x18), rgb(0x45, 0x47, 0x4b), al(skin.acc, 0.3)),
                    Look::Machined => (rgb(0x1d, 0x20, 0x24), rgb(0x3a, 0x3f, 0x46), al(skin.acc, 0.25)),
                };
                let mut pick = None;
                let inner = egui::Area::new(Id::new((self.plugin, "size-list"))).order(Order::Foreground).fixed_pos(sr.left_bottom() + vec2(-14.0, 4.0)).show(ui.ctx(), |ui| {
                    egui::Frame::NONE.fill(bg).stroke(Stroke::new(1.0_f32, edge)).inner_margin(4.0).show(ui, |ui| {
                        txt(ui.painter(), ui.max_rect().left_top() + vec2(6.0, 9.0), Align2::LEFT_CENTER, "SIZE", mono(9.5), skin.acc, 1.5);
                        ui.allocate_exact_size(vec2(66.0, 18.0), Sense::hover());
                        for (i, s) in SCALES.iter().enumerate() {
                            let (rr, rs) = ui.allocate_exact_size(vec2(66.0, 22.0), Sense::click());
                            if i == idx {
                                ui.painter().rect_filled(rr, 2.0, hi);
                            } else if rs.hovered() {
                                ui.painter().rect_filled(rr, 2.0, Color32::from_white_alpha(14));
                            }
                            txt(ui.painter(), rr.center(), Align2::CENTER_CENTER, &format!("{:.0}%", s * 100.0), lab(14.0), if i == idx { Color32::WHITE } else { skin.text_hi() }, 0.5);
                            if rs.clicked() {
                                pick = Some(*s);
                            }
                        }
                    });
                });
                if let Some(v) = pick {
                    self.size_open = false;
                    if let Ok(mut w) = ui_scale.write() {
                        *w = v;
                    }
                    ui.ctx().set_zoom_factor(v);
                } else if (ui.input(|i| i.pointer.any_click()) && !inner.response.hovered() && !resp.hovered()) || ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.size_open = false;
                }
            }
        }
        for (i, s) in ["A", "B"].iter().enumerate() {
            let cr = Rect::from_min_size(pos2(r - 294.0 + i as f32 * 27.0, cy - 10.0), vec2(24.0, 20.0));
            if skin.chip(ui, cr, s, self.ab_on == i, ("ab", i)).clicked() {
                self.switch_ab(setter, params, i);
            }
        }

        let dirty = {
            if self.dirty_check.is_none() {
                self.dirty_check = Some(snap.clone());
            }
            self.dirty_check.as_ref().map(|s| *s != snap).unwrap_or(false)
        };
        let save_as = Rect::from_min_size(pos2(r - 380.0, cy - 10.0), vec2(72.0, 20.0));
        let save = Rect::from_min_size(pos2(save_as.left() - 54.0, cy - 10.0), vec2(50.0, 20.0));
        if skin.chip(ui, save, "SAVE", dirty, "pb-save").clicked() {
            if self.current == INIT {
                self.open_naming(String::new());
            } else {
                // Your preset: update it. A factory one: save your copy under the same name.
                let n = self.current.strip_prefix(USER_G).unwrap_or(&self.current).to_string();
                self.save(ui, setter, params, &n);
            }
        }
        if skin.chip(ui, save_as, "SAVE AS", false, "pb-saveas").clicked() {
            let start = if self.current == INIT { String::new() } else { self.current.strip_prefix(USER_G).unwrap_or(&self.current).to_string() };
            self.open_naming(start);
        }

        let pl = (l + 300.0).min(save.left() - 160.0).max(brand_r + 14.0);
        let mut box_right = save.left() - 12.0;
        if let Some(keep) = self.dice {
            let dr = Rect::from_min_size(pos2(save.left() - 36.0, cy - 10.0), vec2(26.0, 20.0));
            if self.dice_btn(ui, dr, skin).clicked() {
                let gentle = ui.input(|i| i.modifiers.shift);
                self.roll(ui, setter, params, keep, gentle);
            }
            box_right = dr.left() - 8.0;
        }
        let preset_r = Rect::from_min_max(pos2(pl, band.top() + 9.0), pos2(box_right, band.bottom() - 9.0));
        self.browser(ui, setter, params, preset_r, skin, dirty);
        Rect::from_min_max(pos2(whole.left(), band.bottom()), whole.right_bottom())
    }

    /// Undo (left) / redo (right) curved-arrow button.
    fn icon(&self, ui: &mut egui::Ui, r: Rect, skin: &Skin, undo: bool, enabled: bool) -> bool {
        let resp = ui.interact(r, ui.scope_id().with(("pb-ur", undo)), Sense::click());
        let p = ui.painter();
        let col = if !enabled {
            skin.text_faint().gamma_multiply(0.6)
        } else if resp.hovered() {
            skin.acc
        } else {
            skin.text_dim()
        };
        let c = r.center() + vec2(0.0, 2.0);
        let rad = 6.0;
        let (a0, a1) = (std::f32::consts::PI * 1.05, std::f32::consts::PI * 2.0);
        let mut pts = arc_pts(c, rad, a0, a1, 16);
        if undo {
            pts.iter_mut().for_each(|q| q.x = 2.0 * c.x - q.x);
        }
        let tip = pts[pts.len() - 1];
        p.add(Shape::line(pts, Stroke::new(1.8_f32, col)));
        let s = if undo { -1.0 } else { 1.0 };
        p.add(Shape::line(vec![tip + vec2(-s * 4.0, -1.0), tip, tip + vec2(s * 1.0, -4.5)], Stroke::new(1.8_f32, col)));
        enabled && resp.clicked()
    }

    fn browser(&mut self, ui: &mut egui::Ui, setter: &ParamSetter, params: &dyn Params, r: Rect, skin: &Skin, dirty: bool) {
        if !self.scanned {
            self.rescan();
        }
        let p = ui.painter().clone();
        let all: Vec<String> = std::iter::once(INIT.to_string()).chain(self.list.iter().cloned()).collect();
        let idx = all.iter().position(|n| *n == self.current).unwrap_or(0);
        let aw = 22.0;
        let prev_r = Rect::from_min_size(r.left_top(), vec2(aw, r.height()));
        let next_r = Rect::from_min_max(pos2(r.right() - aw, r.top()), r.right_bottom());
        let name_r = Rect::from_min_max(pos2(prev_r.right() + 2.0, r.top()), pos2(next_r.left() - 2.0, r.bottom()));
        let resp = ui.interact(name_r, ui.scope_id().with("pb-name"), Sense::click());
        let hot = resp.hovered() || self.open;
        match skin.look {
            Look::Blackface => {
                p.rect_filled(r, 3.0, rgb(0x19, 0x19, 0x1b));
                p.rect_stroke(r, 3.0, Stroke::new(1.0_f32, if hot { rgb(0x55, 0x57, 0x5c) } else { rgb(0x2d, 0x2e, 0x31) }), StrokeKind::Inside);
            }
            Look::Hud => cham_fill(&p, r, 8.0, 8.0, al(CY, if hot { 0.12 } else { 0.07 }), Stroke::new(1.0_f32, al(CY, 0.55))),
            Look::Machined => {
                p.rect_filled(r, r.height() / 2.0, rgb(0x1d, 0x20, 0x24));
                p.rect_stroke(r, r.height() / 2.0, Stroke::new(1.0_f32, if hot { rgb(0x45, 0x4b, 0x53) } else { rgb(0x2c, 0x30, 0x36) }), StrokeKind::Inside);
            }
        }
        let arrow_col = |h: bool| if h { skin.acc } else { skin.text_dim() };
        let pr = ui.interact(prev_r, ui.scope_id().with("pb-prev"), Sense::click());
        let nr = ui.interact(next_r, ui.scope_id().with("pb-next"), Sense::click());
        let (lt, rt) = if skin.look == Look::Hud { ("◂", "▸") } else { ("‹", "›") };
        txt(&p, prev_r.center(), Align2::CENTER_CENTER, lt, if skin.look == Look::Hud { mono(13.0) } else { lab(19.0) }, arrow_col(pr.hovered()), 0.0);
        txt(&p, next_r.center(), Align2::CENTER_CENTER, rt, if skin.look == Look::Hud { mono(13.0) } else { lab(19.0) }, arrow_col(nr.hovered()), 0.0);
        if pr.clicked() {
            let n = all[(idx + all.len() - 1) % all.len()].clone();
            self.load(ui, setter, params, &n);
        }
        if nr.clicked() {
            let n = all[(idx + 1) % all.len()].clone();
            self.load(ui, setter, params, &n);
        }
        let shown = match &self.msg {
            Some((m, until)) if ui.input(|i| i.time) < *until => m.clone(),
            _ => String::new(),
        };
        let mut label = if !shown.is_empty() { shown } else if dirty { format!("{} *", self.current) } else { self.current.clone() };
        if skin.look == Look::Hud {
            label = format!("[ {} ]", label.to_uppercase());
        }
        let font = if skin.look == Look::Hud { mono(12.5) } else { lab(14.5) };
        let np = p.with_clip_rect(name_r.shrink(2.0));
        txt(&np, name_r.center(), Align2::CENTER_CENTER, &label, font, skin.text_hi(), 1.0);
        if resp.clicked() {
            self.open = !self.open;
            if self.open {
                self.rescan();
            }
        }

        // Dropdown list.
        if self.open {
            let mut pick = None;
            let mut fav_toggle: Option<(String, bool)> = None;
            let area = egui::Area::new(Id::new((self.plugin, "preset-list"))).order(Order::Foreground).fixed_pos(r.left_bottom() + vec2(0.0, 4.0));
            let (bg, edge, hi) = match skin.look {
                Look::Hud => (Color32::from_rgba_unmultiplied(4, 10, 18, 250), CY, al(skin.acc, 0.28)),
                Look::Blackface => (rgb(0x16, 0x16, 0x18), rgb(0x45, 0x47, 0x4b), al(skin.acc, 0.3)),
                Look::Machined => (rgb(0x1d, 0x20, 0x24), rgb(0x3a, 0x3f, 0x46), al(skin.acc, 0.25)),
            };
            let inner = area.show(ui.ctx(), |ui| {
                let w = r.width().max(260.0);
                egui::Frame::NONE.fill(bg).stroke(Stroke::new(1.0_f32, edge)).inner_margin(6.0).show(ui, |ui| {
                    ui.set_width(w);
                    egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                        let favs: Vec<String> = self.favs.iter().filter(|f| all.contains(f)).cloned().collect();
                        let mut rows: Vec<(String, bool)> = favs.iter().map(|f| (f.clone(), true)).collect();
                        rows.extend(all.iter().map(|n| (n.clone(), false)));
                        let mut last_group = String::new();
                        for (n, in_favs) in &rows {
                            // "CATEGORY - Name": a small group header when the category changes.
                            let group = if *in_favs { "★ FAVOURITES".to_string() } else { n.split_once(" - ").map(|(g, _)| g.to_string()).unwrap_or_default() };
                            if !group.is_empty() && group != last_group {
                                let (gr, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
                                txt(ui.painter(), gr.left_center() + vec2(6.0, 1.0), Align2::LEFT_CENTER, &group, mono(9.5), skin.acc, 1.5);
                            }
                            last_group = group;
                            let (rr, rs) = ui.allocate_exact_size(vec2(w, 22.0), Sense::click());
                            let sel = *n == self.current;
                            if sel {
                                ui.painter().rect_filled(rr, 2.0, hi);
                            } else if rs.hovered() {
                                ui.painter().rect_filled(rr, 2.0, Color32::from_white_alpha(14));
                            }
                            let shown = if *in_favs { n.strip_prefix(USER_G).unwrap_or(n) } else { n.split_once(" - ").map(|(_, s)| s).unwrap_or(n) };
                            txt(ui.painter(), rr.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, shown, lab(14.0), if sel { Color32::WHITE } else { skin.text_hi() }, 0.5);
                            // Star on the right: click it to (un)favourite without loading.
                            let fav = self.favs.contains(n);
                            let star_r = Rect::from_center_size(pos2(rr.right() - 14.0, rr.center().y), vec2(22.0, 20.0));
                            if *n != INIT {
                                let on_star = rs.hover_pos().map(|q| star_r.contains(q)).unwrap_or(false);
                                txt(ui.painter(), star_r.center(), Align2::CENTER_CENTER, if fav { "★" } else { "☆" }, mono(13.0), if fav { skin.acc } else if on_star { skin.text_hi() } else { skin.text_faint() }, 0.0);
                            }
                            if rs.clicked() {
                                let on_star = rs.interact_pointer_pos().map(|q| star_r.contains(q)).unwrap_or(false);
                                if on_star && *n != INIT {
                                    fav_toggle = Some((n.clone(), !fav));
                                } else {
                                    pick = Some(n.clone());
                                }
                            }
                        }
                        if all.len() == 1 {
                            ui.label(egui::RichText::new("No saved presets yet: hit SAVE AS").color(skin.text_dim()));
                        }
                    });
                });
            });
            if let Some((n, on)) = fav_toggle {
                self.set_fav(&n, on);
            }
            if let Some(n) = pick {
                self.open = false;
                self.load(ui, setter, params, &n);
            } else if ui.input(|i| i.pointer.any_click()) && !inner.response.hovered() && !resp.hovered() {
                self.open = false;
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.open = false;
            }
        }

        // SAVE AS name prompt: type, then Enter or SAVE. Clicking anywhere else cancels.
        if let Some(mut text) = self.naming.take() {
            let mut done = None;
            let first = std::mem::replace(&mut self.naming_new, false);
            let (bg, edge) = match skin.look {
                Look::Hud => (Color32::from_rgba_unmultiplied(4, 10, 18, 250), CY),
                Look::Blackface => (rgb(0x16, 0x16, 0x18), rgb(0x45, 0x47, 0x4b)),
                Look::Machined => (rgb(0x1d, 0x20, 0x24), rgb(0x3a, 0x3f, 0x46)),
            };
            let area = egui::Area::new(Id::new((self.plugin, "preset-name"))).order(Order::Foreground).fixed_pos(r.left_bottom() + vec2(0.0, 4.0)).show(ui.ctx(), |ui| {
                egui::Frame::NONE.fill(bg).stroke(Stroke::new(1.0_f32, skin.acc)).inner_margin(10.0).show(ui, |ui| {
                    ui.label(egui::RichText::new("SAVE PRESET  ·  goes to your User folder + Favourites").color(skin.acc));
                    ui.add_space(4.0);
                    let te = ui.add(egui::TextEdit::singleline(&mut text).desired_width(300.0).hint_text("Preset name"));
                    if first || !te.has_focus() {
                        te.request_focus();
                    }
                    ui.add_space(6.0);
                    let (row, _) = ui.allocate_exact_size(vec2(300.0, 22.0), Sense::hover());
                    let sr = Rect::from_min_size(row.left_top(), vec2(96.0, 22.0));
                    let cr = Rect::from_min_size(row.left_top() + vec2(104.0, 0.0), vec2(96.0, 22.0));
                    if skin.chip(ui, sr, "SAVE", true, "pb-name-save").clicked() {
                        done = Some(true);
                    }
                    if skin.chip(ui, cr, "CANCEL", false, "pb-name-cancel").clicked() {
                        done = Some(false);
                    }
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        done = Some(true);
                    } else if ui.input(|i| i.key_pressed(Key::Escape)) {
                        done = Some(false);
                    }
                });
            });
            if done.is_none() && !first && ui.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer() {
                done = Some(false);
            }
            match done {
                Some(true) => self.save(ui, setter, params, &text),
                Some(false) => {}
                None => self.naming = Some(text),
            }
        }
    }
}

