//! WaveTag — awk studios. Tempo-synced LFO volume shaper / tremolo.
//!
//! gain(t) = 1 - DEPTH * shape(phase(t)), smoothed by SMOOTH so nothing clicks.
//! Phase comes from FL's beat position while playing (locked to the grid), or
//! runs free (Hz mode, transport stopped, or MIDI retrigger mode).
//! WIDTH offsets the right channel's phase (0..180°) for auto-pan / wide trem.
//! Header MIX = dry/wet of the whole effect; power = host bypass (20 ms fade).
//! WaveTag has no latency, so the dry path needs no delay.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

mod awk_ui;
mod editor;
mod presets;
pub mod shapes;
pub use shapes::*;

pub const SMOOTH_MIN_MS: f32 = 0.5;

/// What the editor needs to animate the display.
pub struct Shared {
    pub phase_l: AtomicU32,
    pub phase_r: AtomicU32,
    pub gain_l: AtomicU32,
    pub gain_r: AtomicU32,
    /// Transport playing or audio coming in: the editor only animates then.
    pub active: AtomicBool,
    /// L/R in + out peaks for the meters.
    pub meters: awk_ui::Meters,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            phase_l: AtomicU32::new(0f32.to_bits()),
            phase_r: AtomicU32::new(0f32.to_bits()),
            gain_l: AtomicU32::new(1f32.to_bits()),
            gain_r: AtomicU32::new(1f32.to_bits()),
            active: AtomicBool::new(false),
            meters: awk_ui::Meters::default(),
        }
    }
}

#[derive(Params)]
pub struct WaveTagParams {
    #[persist = "editor-state-v2"]
    pub editor_state: Arc<EguiState>,

    #[id = "shape"]
    pub shape: IntParam,
    #[id = "bend"]
    pub bend: FloatParam,
    /// Tempo-synced (note values) or free (Hz).
    #[id = "sync"]
    pub sync: BoolParam,
    #[id = "rate"]
    pub rate: FloatParam,
    #[id = "hz"]
    pub hz: FloatParam,
    #[id = "depth"]
    pub depth: FloatParam,
    #[id = "smooth"]
    pub smooth: FloatParam,
    /// Shifts the whole wave along the beat (0..360°).
    #[id = "phase"]
    pub phase: FloatParam,
    /// Right channel's phase offset from the left (0..180°).
    #[id = "width"]
    pub width: FloatParam,
    /// Restart the wave on every MIDI note (instead of running with the beat).
    #[id = "retrig"]
    pub retrig: BoolParam,
    /// Change the volume only at zero crossings (no clicks on bass).
    #[id = "zerox"]
    pub zero_x: BoolParam,
    #[id = "output"]
    pub output: FloatParam,
    /// Header MIX: dry/wet of the whole effect.
    #[id = "gmix"]
    pub mix: FloatParam,
    /// Header power button (host bypass).
    #[id = "bypass"]
    pub bypass: BoolParam,
    /// SAFE: output limiter at -0.1 dBFS (footer switch, on by default).
    #[id = "safe"]
    pub safe: BoolParam,
}

impl Default for WaveTagParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(editor::WIDTH, editor::HEIGHT),
            shape: IntParam::new("Shape", 5, IntRange::Linear { min: 0, max: SHAPE_NAMES.len() as i32 - 1 })
                .with_value_to_string(Arc::new(|v| SHAPE_NAMES[v.clamp(0, crate::shapes::LAST_SHAPE) as usize].to_string()))
                .with_string_to_value(Arc::new(|s| {
                    SHAPE_NAMES.iter().position(|n| n.eq_ignore_ascii_case(s.trim())).map(|i| i as i32)
                })),
            bend: FloatParam::new("Bend", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            sync: BoolParam::new("Tempo Sync", true),
            rate: FloatParam::new(
                "Rate",
                DEFAULT_RATE as f32,
                FloatRange::Linear { min: 0.0, max: (RATE_NAMES.len() - 1) as f32 },
            )
            .with_step_size(1.0)
            .with_value_to_string(Arc::new(|v| RATE_NAMES[(v.round() as usize).min(17)].to_string()))
            .with_string_to_value(Arc::new(|s| {
                RATE_NAMES.iter().position(|n| n.eq_ignore_ascii_case(s.trim())).map(|i| i as f32)
            })),
            hz: FloatParam::new("Rate Hz", 4.0, FloatRange::Skewed { min: 0.05, max: 30.0, factor: FloatRange::skew_factor(-2.0) })
                .with_value_to_string(Arc::new(|v| if v < 1.0 { format!("{v:.2} Hz") } else { format!("{v:.1} Hz") })),
            depth: FloatParam::new("Depth", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            smooth: FloatParam::new(
                "Smooth",
                5.0,
                FloatRange::Skewed { min: SMOOTH_MIN_MS, max: 50.0, factor: FloatRange::skew_factor(-1.5) },
            )
            .with_unit(" ms")
            .with_value_to_string(Arc::new(|v| if v < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") })),
            phase: FloatParam::new("Phase", 0.0, FloatRange::Linear { min: 0.0, max: 360.0 })
                .with_step_size(1.0)
                .with_unit("°"),
            width: FloatParam::new("Width", 0.0, FloatRange::Linear { min: 0.0, max: 180.0 })
                .with_step_size(1.0)
                .with_unit("°"),
            retrig: BoolParam::new("MIDI Retrigger", false),
            zero_x: BoolParam::new("Zero-X", true),
            output: FloatParam::new(
                "Output",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(12.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 12.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            mix: awk_ui::mix_param(),
            bypass: awk_ui::bypass_param(),
            safe: awk_ui::safe_param(),
        }
    }
}

struct WaveTag {
    params: Arc<WaveTagParams>,
    shared: Arc<Shared>,
    sr: f32,
    /// Free-running phase (cycles), used when not locked to the host grid.
    free_phase: f64,
    /// Samples of silence in a row.
    quiet: u32,
    smooth: [GainSlew; 2],
    zx: [ZeroX; 2],
    bpm: f32,
    byp: awk_ui::BypassRamp,
    meter: awk_ui::PeakFollower,
    ceil: awk_ui::Ceiling,
}

impl Default for WaveTag {
    fn default() -> Self {
        Self {
            params: Arc::new(WaveTagParams::default()),
            shared: Arc::new(Shared::default()),
            sr: 44100.0,
            free_phase: 0.0,
            quiet: u32::MAX,
            smooth: [GainSlew::default(); 2],
            zx: [ZeroX::default(); 2],
            bpm: 120.0,
            byp: awk_ui::BypassRamp::default(),
            meter: awk_ui::PeakFollower::default(),
            ceil: awk_ui::Ceiling::default(),
        }
    }
}

/// Phase (cycles) of the wave at a host beat position.
#[inline]
pub fn synced_phase(beats: f64, cycle_beats: f64, phase_deg: f32) -> f64 {
    (beats / cycle_beats + phase_deg as f64 / 360.0).rem_euclid(1.0)
}

impl Plugin for WaveTag {
    const NAME: &'static str = "WaveTag";
    const VENDOR: &'static str = "awk studios";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];
    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone())
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sr = buffer_config.sample_rate;
        self.byp.set_sr(self.sr);
        self.meter.set_sr(self.sr);
        self.ceil.set_sr(self.sr);
        true
    }

    fn reset(&mut self) {
        self.free_phase = 0.0;
        self.smooth = [GainSlew::default(); 2];
        self.zx = [ZeroX::default(); 2];
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let sr = self.sr;
        let t = context.transport();
        if let Some(b) = t.tempo {
            self.bpm = b as f32;
        }
        let playing = t.playing;
        let host_beats = t.pos_beats();
        let p = &self.params;
        let sync = p.sync.value();
        let retrig = p.retrig.value();
        let cycle_beats = RATE_BEATS[(p.rate.value().round() as usize).min(17)];
        // Cycles per sample when running free.
        let step = if sync { self.bpm as f64 / 60.0 / cycle_beats / sr as f64 } else { p.hz.value() as f64 / sr as f64 };
        let lock_to_grid = sync && playing && !retrig && host_beats.is_some();
        let beat_step = self.bpm as f64 / 60.0 / sr as f64;
        let mut beats = host_beats.unwrap_or(0.0);
        let kind = p.shape.value().clamp(0, crate::shapes::LAST_SHAPE) as usize;
        let nch = buffer.channels();
        let zero_x = p.zero_x.value();
        let max_wait = (ZERO_X_MAX_WAIT_MS * 0.001 * sr) as u32;
        let bypassed = p.bypass.value();
        // Anything coming in? (-80 dB, held for 300 ms so short gaps don't count.)
        let mut peak = 0.0f32;
        for ch in buffer.as_slice_immutable().iter() {
            peak = ch.iter().fold(peak, |m, v| m.max(v.abs()));
        }
        if peak > 1e-4 {
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(buffer.samples() as u32);
        }
        let active = playing || self.quiet < (0.3 * sr) as u32;
        // Nothing playing: hold the free-running LFO still instead of spinning.
        let step = if active { step } else { 0.0 };

        let mut next_event = context.next_event();
        let (mut last_l, mut last_r) = (0.0f32, 0.0f32);
        for (i, mut frame) in buffer.iter_samples().enumerate() {
            while let Some(ev) = next_event {
                if ev.timing() as usize > i {
                    break;
                }
                if let NoteEvent::NoteOn { .. } = ev {
                    if retrig {
                        self.free_phase = 0.0;
                    }
                }
                next_event = context.next_event();
            }
            let bend = p.bend.smoothed.next();
            let depth = p.depth.smoothed.next();
            let phase_deg = p.phase.smoothed.next();
            let width = p.width.smoothed.next();
            let out = p.output.smoothed.next();
            let c = slew_coef(p.smooth.smoothed.next(), sr);
            // Wet amount: header MIX, faded to 0 by the power button.
            let wet = p.mix.smoothed.next() * self.byp.next(bypassed);

            let ph_l = if lock_to_grid {
                synced_phase(beats, cycle_beats, phase_deg)
            } else {
                (self.free_phase + phase_deg as f64 / 360.0).rem_euclid(1.0)
            };
            let ph_r = (ph_l + width as f64 / 360.0).rem_euclid(1.0);
            let targets = [
                1.0 - depth * shape_at(kind, ph_l as f32, bend),
                1.0 - depth * shape_at(kind, ph_r as f32, bend),
            ];
            let mut io = [[0.0f32; 2]; 2];
            for (ch, s) in frame.iter_mut().enumerate().take(2) {
                let k = if nch == 1 { 0 } else { ch };
                let g = if zero_x {
                    self.zx[k].tick(*s, targets[k], &mut self.smooth[k], c, max_wait)
                } else {
                    self.smooth[k].tick(targets[k], c)
                };
                let dry = *s;
                *s = dry + (dry * g * out - dry) * wet;
                io[ch] = [dry, *s];
            }
            if nch == 1 {
                io[1] = io[0];
            }
            // SAFE: never above -0.1 dBFS.
            let (cl, cr) = self.ceil.tick(io[0][1], io[1][1], self.params.safe.value());
            io[0][1] = cl;
            io[1][1] = cr;
            for (ch, s) in frame.iter_mut().enumerate().take(2) {
                *s = io[ch][1];
            }
            self.meter.feed(io[0][0], io[1][0], io[0][1], io[1][1]);
            last_l = ph_l as f32;
            last_r = ph_r as f32;
            self.free_phase = (self.free_phase + step).rem_euclid(1.0);
            beats += beat_step;
        }
        // Keep the free phase in step with the grid so switching modes doesn't jump.
        if lock_to_grid {
            self.free_phase = synced_phase(beats, cycle_beats, 0.0);
        }
        let sh = &self.shared;
        sh.active.store(active, Ordering::Relaxed);
        sh.phase_l.store(last_l.to_bits(), Ordering::Relaxed);
        sh.phase_r.store(last_r.to_bits(), Ordering::Relaxed);
        sh.gain_l.store(self.smooth[0].y.to_bits(), Ordering::Relaxed);
        sh.gain_r.store(self.smooth[1].y.to_bits(), Ordering::Relaxed);
        self.meter.store(&sh.meters);
        self.ceil.store(&sh.meters);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for WaveTag {
    const CLAP_ID: &'static str = "com.awknode.wavetag";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Tempo-synced LFO volume shaper / tremolo");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Tremolo];
}

impl Vst3Plugin for WaveTag {
    const VST3_CLASS_ID: [u8; 16] = *b"AwknodeWaveTag01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

nih_export_clap!(WaveTag);
nih_export_vst3!(WaveTag);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synced_phase_locks_to_the_beat() {
        // 1/4 note cycle: every beat starts a new cycle.
        assert!(synced_phase(3.0, 1.0, 0.0).abs() < 1e-9);
        assert!((synced_phase(3.5, 1.0, 0.0) - 0.5).abs() < 1e-9);
        // 1 bar cycle, 90° offset.
        assert!((synced_phase(0.0, 4.0, 90.0) - 0.25).abs() < 1e-9);
    }

    /// MIX 0 and bypass both pass the input untouched; MIX 1 is the full effect.
    #[test]
    fn mix_and_bypass_math() {
        let (dry, g, out) = (0.7f32, 0.25f32, 1.0f32);
        let f = |wet: f32| dry + (dry * g * out - dry) * wet;
        assert_eq!(f(0.0), dry);
        assert!((f(1.0) - dry * g).abs() < 1e-7);
        let mut b = awk_ui::BypassRamp::default();
        b.set_sr(48000.0);
        let mut w = 1.0;
        for _ in 0..4800 {
            w = b.next(true);
        }
        assert_eq!(w, 0.0, "bypass settles to fully dry within 100 ms");
    }

    #[test]
    fn depth_zero_is_transparent() {
        let mut s = GainSlew::default();
        for i in 0..1000 {
            let g = s.tick(1.0 - 0.0 * shape_at(2, i as f32 / 100.0, 0.5), 0.9);
            assert_eq!(g, 1.0);
        }
    }
}
