//! Test-only software rasteriser for egui frames (SHOT_DIR screenshots).
#![cfg(test)]
#![allow(dead_code)]
use crate::editor::{HEIGHT, WIDTH};
use nih_plug::context::gui::GuiContext;
use nih_plug::prelude::*;
use nih_plug::wrapper::state::PluginState;
use nih_plug_egui::egui::{self, epaint, Event, ImageData, Pos2, RawInput, Rect, TextureId, TextureWrapMode, Vec2};
use std::collections::HashMap;

pub struct Mock;
impl GuiContext for Mock {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Vst3
    }
    fn request_resize(&self) -> bool {
        true
    }
    unsafe fn raw_begin_set_parameter(&self, _p: ParamPtr) {}
    unsafe fn raw_set_parameter_normalized(&self, _p: ParamPtr, _n: f32) {}
    unsafe fn raw_end_set_parameter(&self, _p: ParamPtr) {}
    fn get_state(&self) -> PluginState {
        PluginState { version: String::new(), params: Default::default(), fields: Default::default() }
    }
    fn set_state(&self, _s: PluginState) {}
}
pub struct Tex {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
    repeat: bool,
}
pub fn apply_delta(texs: &mut HashMap<TextureId, Tex>, out: &egui::FullOutput) {
    for (id, d) in &out.textures_delta.set {
        let (w, h, px): (usize, usize, Vec<[f32; 4]>) = match &d.image {
            ImageData::Color(c) => (c.size[0], c.size[1], c.pixels.iter().map(|p| [p.r() as f32 / 255.0, p.g() as f32 / 255.0, p.b() as f32 / 255.0, p.a() as f32 / 255.0]).collect()),
            ImageData::Font(f) => (f.size[0], f.size[1], f.srgba_pixels(None).map(|p| [p.r() as f32 / 255.0, p.g() as f32 / 255.0, p.b() as f32 / 255.0, p.a() as f32 / 255.0]).collect()),
        };
        let repeat = d.options.wrap_mode == TextureWrapMode::Repeat;
        if let Some(pos) = d.pos {
            let t = texs.get_mut(id).unwrap();
            for y in 0..h {
                for x in 0..w {
                    t.px[(pos[1] + y) * t.w + pos[0] + x] = px[y * w + x];
                }
            }
        } else {
            texs.insert(*id, Tex { w, h, px, repeat });
        }
    }
}
pub fn render(ctx: &egui::Context, out: egui::FullOutput, texs: &mut HashMap<TextureId, Tex>, path: &str) {
    let img = raster(ctx, out, texs);
    let bytes: Vec<u8> = img.iter().flat_map(|p| p.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8).collect::<Vec<_>>()).collect();
    std::fs::write(path, bytes).unwrap();
}
pub fn raster(ctx: &egui::Context, out: egui::FullOutput, texs: &mut HashMap<TextureId, Tex>) -> Vec<[f32; 3]> {
    apply_delta(texs, &out);
    let (w, h) = (WIDTH as usize, HEIGHT as usize);
    let mut img = vec![[0.0f32; 3]; w * h];
    let prims = ctx.tessellate(out.shapes, 1.0);
    for cp in prims {
        let epaint::Primitive::Mesh(mesh) = cp.primitive else { continue };
        let clip = cp.clip_rect;
        let tex = texs.get(&mesh.texture_id);
        for tri in mesh.indices.chunks(3) {
            let v = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
            let (x0, x1) = (v.iter().map(|q| q.pos.x).fold(f32::MAX, f32::min).max(clip.left()).max(0.0), v.iter().map(|q| q.pos.x).fold(f32::MIN, f32::max).min(clip.right()).min(w as f32));
            let (y0, y1) = (v.iter().map(|q| q.pos.y).fold(f32::MAX, f32::min).max(clip.top()).max(0.0), v.iter().map(|q| q.pos.y).fold(f32::MIN, f32::max).min(clip.bottom()).min(h as f32));
            let area = (v[1].pos.x - v[0].pos.x) * (v[2].pos.y - v[0].pos.y) - (v[2].pos.x - v[0].pos.x) * (v[1].pos.y - v[0].pos.y);
            if area.abs() < 1e-6 {
                continue;
            }
            for py in (y0.floor() as usize)..(y1.ceil() as usize).min(h) {
                for px in (x0.floor() as usize)..(x1.ceil() as usize).min(w) {
                    let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                    let e = |a: &epaint::Vertex, b: &epaint::Vertex| (b.pos.x - a.pos.x) * (fy - a.pos.y) - (fx - a.pos.x) * (b.pos.y - a.pos.y);
                    let (w0, w1, w2) = (e(&v[1], &v[2]) / area, e(&v[2], &v[0]) / area, e(&v[0], &v[1]) / area);
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let mut c = [0.0f32; 4];
                    for i in 0..4 {
                        c[i] = (w0 * v[0].color.to_array()[i] as f32 + w1 * v[1].color.to_array()[i] as f32 + w2 * v[2].color.to_array()[i] as f32) / 255.0;
                    }
                    if let Some(t) = tex {
                        let mut u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                        let mut vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                        if t.repeat {
                            u = u.rem_euclid(1.0);
                            vv = vv.rem_euclid(1.0);
                        }
                        let tx = ((u * t.w as f32) as usize).min(t.w - 1);
                        let ty = ((vv * t.h as f32) as usize).min(t.h - 1);
                        let s = t.px[ty * t.w + tx];
                        for i in 0..4 {
                            c[i] *= s[i];
                        }
                    }
                    let d = &mut img[py * w + px];
                    for i in 0..3 {
                        d[i] = c[i] + d[i] * (1.0 - c[3]);
                    }
                }
            }
        }
    }
    img
}
pub fn raw(i: usize, ev: Vec<Event>) -> RawInput {
    RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(WIDTH as f32, HEIGHT as f32))), time: Some(i as f64 * 0.04), events: ev, ..Default::default() }
}
