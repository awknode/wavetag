# WAVETAG manual

## How it works

```
gain(t) = 1 − DEPTH × shape(phase(t)), smoothed by SMOOTH so nothing clicks
```

The phase follows FL's beat position while the song plays, so it's locked to the grid. It runs free in Hz mode, when the transport is stopped, or in MIDI retrigger mode.

## Controls

| Control | What it does |
|---|---|
| **SHAPE** | Click a chip to pick a shape: 13 classic shapes and 8 BASS shapes. |
| **RATE / RATE HZ** | Note value when TEMPO SYNC is on (4 BAR to 1/64), or Hz when it's off. |
| **DEPTH** | How far the volume dips. |
| **BEND** | Reshapes the curve. 50 % is the normal version of each shape. |
| **SMOOTH** | Rounds the edges (ms) so steps don't click. |
| **PHASE** | Shifts the whole wave along the beat (0 to 360°). |
| **WIDTH** | The right channel's phase offset from the left (0 to 180°), for auto-pan and wide tremolo. |
| **MIDI RETRIGGER** | Restarts the wave on every MIDI note instead of running with the beat. The plugin needs MIDI routed to it. |
| **ZERO-X** | Changes the volume only at zero crossings. |
| **OUTPUT / MIX** | Output level and dry/wet. |

## THE WAVE

The real gain curve over one bar with the current settings, on a beat grid with the gain axis in dB. The right channel is dashed when WIDTH is above 0, and the playhead and live L/R gain readouts move with the song.
