//! Synthetic shift-sound playback via rodio (CPAL backend).
//!
//! Works on Linux (ALSA / PipeWire-ALSA), Windows (WASAPI), and macOS (CoreAudio).
//! All sounds are generated as PCM samples — no audio files bundled.
//! `play_shift_sound(name)` spawns a short-lived OS thread; the caller never blocks.

use rodio::{OutputStream, Sink, buffer::SamplesBuffer};
use std::time::Duration;

const SR: u32 = 44100;

/// Play a named shift sound on the default system audio output.
///
/// Valid names: `"blip"` `"click"` `"beep"` `"chord"` `"buzz"`.
/// `"none"` or any unknown name is silently ignored.
pub(crate) fn play_shift_sound(name: &str) {
    if name.is_empty() || name == "none" {
        return;
    }
    let name = name.to_owned();
    if let Err(e) = std::thread::Builder::new()
        .name("shift-beep".into())
        .spawn(move || run_audio(&name))
    {
        eprintln!("[audio] failed to spawn audio thread: {e}");
    }
}

fn run_audio(name: &str) {
    let samples = match name {
        "blip"  => gen_blip(),
        "click" => gen_click(),
        "beep"  => gen_beep(),
        "chord" => gen_chord(),
        "buzz"  => gen_buzz(),
        other   => {
            eprintln!("[audio] unknown sound name: {other:?}");
            return;
        }
    };

    let (stream, handle) = match OutputStream::try_default() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("[audio] could not open audio output: {e}");
            eprintln!("[audio] on Linux install 'pipewire-alsa' or 'pulseaudio-alsa' and ensure the user has audio permissions.");
            return;
        }
    };

    let sink = match Sink::try_new(&handle) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[audio] could not create audio sink: {e}");
            return;
        }
    };

    sink.append(SamplesBuffer::new(1, SR, samples));
    sink.sleep_until_end();
    // Keep stream alive a little longer so the OS audio buffer can flush.
    std::thread::sleep(Duration::from_millis(50));
    drop(stream);
}

// ── Sound generators ──────────────────────────────────────────────────────────

/// Falling-pitch sawtooth chirp: 1 400 Hz → 800 Hz over 80 ms, exp decay.
fn gen_blip() -> Vec<f32> {
    let n = (SR * 80 / 1000) as usize;
    let mut phase = 0.0f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / SR as f64;
            let freq = 1400.0 - 600.0 * (t / 0.080);
            phase = (phase + freq / SR as f64).fract();
            let wave = phase as f32 * 2.0 - 1.0;
            wave * (-t as f32 * 35.0).exp() * 0.40
        })
        .collect()
}

/// Sharp square-wave burst at 900 Hz, 40 ms, very fast decay.
fn gen_click() -> Vec<f32> {
    let n = (SR * 40 / 1000) as usize;
    let mut phase = 0.0f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / SR as f64;
            phase = (phase + 900.0 / SR as f64).fract();
            let wave: f32 = if phase < 0.5 { 1.0 } else { -1.0 };
            wave * (-t as f32 * 60.0).exp() * 0.32
        })
        .collect()
}

/// Clean sine beep at 1 200 Hz, 100 ms, moderate decay.
fn gen_beep() -> Vec<f32> {
    let n = (SR * 100 / 1000) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / SR as f64;
            let wave = (2.0 * std::f64::consts::PI * 1200.0 * t).sin() as f32;
            wave * (-t as f32 * 25.0).exp() * 0.35
        })
        .collect()
}

/// Bright major triad (A4 + C5 + E5 = 440 + 523 + 659 Hz), 90 ms.
fn gen_chord() -> Vec<f32> {
    const FREQS: [f64; 3] = [440.0, 523.0, 659.0];
    let n = (SR * 90 / 1000) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / SR as f64;
            let mix = FREQS
                .iter()
                .map(|&f| (2.0 * std::f64::consts::PI * f * t).sin() as f32)
                .sum::<f32>()
                / FREQS.len() as f32;
            mix * (-t as f32 * 22.0).exp() * 0.38
        })
        .collect()
}

/// Aggressive low-pitch sawtooth buzz at 220 Hz, 60 ms.
fn gen_buzz() -> Vec<f32> {
    let n = (SR * 60 / 1000) as usize;
    let mut phase = 0.0f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / SR as f64;
            phase = (phase + 220.0 / SR as f64).fract();
            let wave = phase as f32 * 2.0 - 1.0;
            wave * (-t as f32 * 25.0).exp() * 0.35
        })
        .collect()
}
