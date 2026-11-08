// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The audio output: a software mixer with the behavior of `OpenAlSoundEngine.cs`, on ALSA loaded at runtime on
//! Linux and on a cpal output stream elsewhere.

#[cfg(target_os = "linux")]
mod alsa;

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[cfg(not(target_os = "linux"))]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rustora_core::world_types::WPos;
use rustora_engine::sound::{AudioDevice, Clip, PlayParams, VoiceId};

const POOL_SIZE: usize = 256;
const MAX_INSTANCES_PER_FRAME: usize = 3;
const GROUP_DISTANCE: i64 = 2730;
const REFERENCE_DISTANCE: f32 = 6826.0;
const MAX_DISTANCE: f32 = 136533.0;
const LISTENER_HEIGHT: i32 = 2133;
const CENTER_GAIN: f32 = std::f32::consts::FRAC_1_SQRT_2;

struct Voice {
    id: VoiceId,
    clip: Clip,
    position: f64,
    step: f64,
    looping: bool,
    relative: bool,
    pos: WPos,
    volume: f32,
    paused: bool,
    frame_started: u64,
}

impl Voice {
    fn frames(&self) -> usize {
        self.clip.samples.len() / self.clip.channels.max(1) as usize
    }

    fn sample(&self, frame: usize, channel: usize) -> f32 {
        self.clip.samples[frame * self.clip.channels as usize + channel] as f32 / 32768.0
    }

    /// Reads the current stereo frame with linear interpolation.
    fn read(&self) -> (f32, f32) {
        let frames = self.frames();
        let i = self.position as usize;
        let next = if i + 1 < frames {
            i + 1
        } else if self.looping {
            0
        } else {
            i
        };
        let t = (self.position - i as f64) as f32;
        let channel = |c: usize| self.sample(i, c) * (1.0 - t) + self.sample(next, c) * t;
        if self.clip.channels == 1 {
            let v = channel(0);
            (v, v)
        } else {
            (channel(0), channel(1))
        }
    }

    /// Advances one output frame. Returns false at the end of the sound.
    fn advance(&mut self) -> bool {
        self.position += self.step;
        let frames = self.frames() as f64;
        if self.position < frames {
            return true;
        }
        if self.looping && frames > 0.0 {
            self.position %= frames;
            return true;
        }
        false
    }
}

/// The state shared with the audio thread.
struct Mixer {
    voices: Vec<Voice>,
    next_id: VoiceId,
    listener: WPos,
    master: f32,
    all_paused: bool,
    frame: u64,
    output_rate: u32,
}

impl Mixer {
    fn new(output_rate: u32) -> Mixer {
        Mixer {
            voices: Vec::new(),
            next_id: 0,
            listener: WPos::new(0, 0, LISTENER_HEIGHT),
            master: 1.0,
            all_paused: false,
            frame: 0,
            output_rate,
        }
    }

    /// The left and right gain of a voice: OpenAL's clamped inverse distance model, panned by the direction to the listener.
    fn gains(&self, v: &Voice) -> (f32, f32) {
        if v.clip.channels != 1 {
            return (v.volume, v.volume);
        }
        if v.relative {
            return (v.volume * CENTER_GAIN, v.volume * CENTER_GAIN);
        }
        let dx = (v.pos.x - self.listener.x) as f32;
        let dy = (v.pos.y - self.listener.y) as f32;
        let dz = (v.pos.z - self.listener.z) as f32;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();
        let clamped = distance.clamp(REFERENCE_DISTANCE, MAX_DISTANCE);
        let gain = v.volume * REFERENCE_DISTANCE / clamped;
        let pan = if distance > 0.0 { dx / distance } else { 0.0 };
        (gain * (0.5 * (1.0 - pan)).sqrt(), gain * (0.5 * (1.0 + pan)).sqrt())
    }

    /// Mixes into interleaved `out` with `channels` channels per frame.
    fn mix(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        if self.all_paused {
            return;
        }
        let gains: Vec<(f32, f32)> = self.voices.iter().map(|v| self.gains(v)).collect();
        let master = self.master;
        let mut index = 0;
        self.voices.retain_mut(|v| {
            let (left, right) = gains[index];
            index += 1;
            if v.paused {
                return true;
            }
            if v.frames() == 0 {
                return false;
            }
            for frame in out.chunks_exact_mut(channels) {
                let (l, r) = v.read();
                if channels == 1 {
                    frame[0] += (l * left + r * right) * 0.5 * master;
                } else {
                    frame[0] += l * left * master;
                    frame[1] += r * right * master;
                }
                if !v.advance() {
                    return false;
                }
            }
            true
        });
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }

    /// The instance limit and attenuation of `OpenAlSoundEngine.Play2D`: `None` when too many instances started nearby.
    fn group_attenuation(&self, clip: &Clip, relative: bool, pos: WPos) -> Option<f32> {
        let (mut instances, mut active) = (0, 0);
        for s in &self.voices {
            if s.relative != relative {
                continue;
            }
            active += 1;
            if !Arc::ptr_eq(&s.clip, clip) || self.frame - s.frame_started >= 5 {
                continue;
            }
            let (dx, dy, dz) = ((s.pos.x - pos.x) as i64, (s.pos.y - pos.y) as i64, (s.pos.z - pos.z) as i64);
            if dx * dx + dy * dy + dz * dz >= GROUP_DISTANCE * GROUP_DISTANCE {
                continue;
            }
            instances += 1;
            if instances == MAX_INSTANCES_PER_FRAME {
                return None;
            }
        }
        Some(0.66 * ((POOL_SIZE as f32 - active as f32 * 0.5) / POOL_SIZE as f32))
    }
}

/// An [`AudioDevice`] on a software mixer. The output runs until this value is dropped.
pub struct MixerDevice {
    mixer: Arc<Mutex<Mixer>>,
    _output: Output,
}

#[allow(dead_code)]
enum Output {
    #[cfg(target_os = "linux")]
    Alsa(alsa::AlsaOutput),
    #[cfg(not(target_os = "linux"))]
    Stream(cpal::Stream),
    File(FileWriter),
}

impl MixerDevice {
    /// Opens the default output device. `None` when there is none or it fails, and the game then runs silently. The
    /// environment variable `RUSTORA_ALSA_DEVICE` names another ALSA device, such as `null` for a test without sound.
    #[cfg(target_os = "linux")]
    pub fn open() -> Option<MixerDevice> {
        let device = std::env::var("RUSTORA_ALSA_DEVICE").unwrap_or_else(|_| "default".to_string());
        match alsa::AlsaOutput::start(&device, |rate| Arc::new(Mutex::new(Mixer::new(rate)))) {
            Ok((output, mixer)) => Some(MixerDevice {
                mixer,
                _output: Output::Alsa(output),
            }),
            Err(e) => {
                log::warn!("Cannot open the audio output: {e}");
                None
            }
        }
    }

    /// Opens the default output device. `None` when there is none or it fails, and the game then runs silently.
    #[cfg(not(target_os = "linux"))]
    pub fn open() -> Option<MixerDevice> {
        let device = cpal::default_host().default_output_device()?;
        let supported = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("No audio output configuration: {e}");
                return None;
            }
        };
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        let mixer = Arc::new(Mutex::new(Mixer::new(config.sample_rate)));
        let stream = match format {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config, &mixer),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config, &mixer),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config, &mixer),
            other => {
                log::warn!("Unsupported audio sample format {other:?}");
                return None;
            }
        };
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Cannot open the audio output: {e}");
                return None;
            }
        };
        if let Err(e) = stream.play() {
            log::warn!("Cannot start the audio output: {e}");
            return None;
        }
        Some(MixerDevice {
            mixer,
            _output: Output::Stream(stream),
        })
    }

    /// Writes the mixed output in real time to a 16-bit stereo WAV file at `path`, to check audio without a sound card.
    pub fn open_file(path: &Path) -> std::io::Result<MixerDevice> {
        const RATE: u32 = 48000;
        let mixer = Arc::new(Mutex::new(Mixer::new(RATE)));
        let writer = FileWriter::start(path, RATE, mixer.clone())?;
        Ok(MixerDevice {
            mixer,
            _output: Output::File(writer),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Mixer> {
        lock(&self.mixer)
    }

    fn with_voice(&self, id: VoiceId, f: impl FnOnce(&mut Voice)) {
        if let Some(v) = self.lock().voices.iter_mut().find(|v| v.id == id) {
            f(v);
        }
    }
}

fn lock(mixer: &Mutex<Mixer>) -> MutexGuard<'_, Mixer> {
    mixer.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(not(target_os = "linux"))]
fn build_stream<T>(device: &cpal::Device, config: &cpal::StreamConfig, mixer: &Arc<Mutex<Mixer>>) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let mixer = mixer.clone();
    let mut buffer = Vec::<f32>::new();
    device.build_output_stream::<T, _, _>(
        *config,
        move |data: &mut [T], _| {
            buffer.resize(data.len(), 0.0);
            lock(&mixer).mix(&mut buffer, channels);
            for (o, s) in data.iter_mut().zip(&buffer) {
                *o = T::from_sample(*s);
            }
        },
        |e| log::warn!("Audio output error: {e}"),
        None,
    )
}

impl AudioDevice for MixerDevice {
    fn play(&self, clip: &Clip, p: PlayParams) -> Option<VoiceId> {
        let mut m = self.lock();
        let atten = if p.attenuate {
            m.group_attenuation(clip, p.relative, p.pos)?
        } else {
            1.0
        };
        if m.voices.len() >= POOL_SIZE {
            return None;
        }
        m.next_id += 1;
        let id = m.next_id;
        let voice = Voice {
            id,
            clip: clip.clone(),
            position: 0.0,
            step: clip.sample_rate as f64 / m.output_rate as f64,
            looping: p.looping,
            relative: p.relative,
            pos: p.pos,
            volume: p.volume * atten,
            paused: false,
            frame_started: m.frame,
        };
        m.voices.push(voice);
        Some(id)
    }

    fn stop(&self, id: VoiceId) {
        self.lock().voices.retain(|v| v.id != id);
    }

    fn stop_all(&self) {
        self.lock().voices.clear();
    }

    fn set_paused(&self, id: VoiceId, paused: bool) {
        self.with_voice(id, |v| v.paused = paused);
    }

    fn set_all_paused(&self, paused: bool) {
        self.lock().all_paused = paused;
    }

    fn set_volume(&self, id: VoiceId, volume: f32) {
        self.with_voice(id, |v| v.volume = volume);
    }

    fn set_position(&self, id: VoiceId, pos: WPos) {
        self.with_voice(id, |v| v.pos = pos);
    }

    fn set_looping(&self, id: VoiceId, looping: bool) {
        self.with_voice(id, |v| v.looping = looping);
    }

    fn is_complete(&self, id: VoiceId) -> bool {
        self.lock().voices.iter().all(|v| v.id != id)
    }

    fn seek_position(&self, id: VoiceId) -> f32 {
        self.lock()
            .voices
            .iter()
            .find(|v| v.id == id)
            .map_or(0.0, |v| (v.position / v.clip.sample_rate as f64) as f32)
    }

    fn set_listener(&self, pos: WPos) {
        self.lock().listener = WPos::new(pos.x, pos.y, pos.z + LISTENER_HEIGHT);
    }

    fn set_master_volume(&self, volume: f32) {
        self.lock().master = volume;
    }

    fn next_frame(&self) {
        self.lock().frame += 1;
    }
}

/// Mixes on a thread at real-time speed and streams 16-bit PCM into a WAV file.
struct FileWriter {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FileWriter {
    fn start(path: &Path, rate: u32, mixer: Arc<Mutex<Mixer>>) -> std::io::Result<FileWriter> {
        let mut file = std::fs::File::create(path)?;
        file.write_all(&wav_header(rate, u32::MAX - 36))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            let start = Instant::now();
            let mut written = 0u64;
            let mut buffer = vec![0.0f32; 2];
            let mut bytes = Vec::new();
            while !thread_stop.load(Ordering::Relaxed) {
                let due = (start.elapsed().as_secs_f64() * rate as f64) as u64;
                let frames = (due - written) as usize;
                if frames > 0 {
                    buffer.resize(frames * 2, 0.0);
                    lock(&mixer).mix(&mut buffer, 2);
                    bytes.clear();
                    bytes.extend(buffer.iter().flat_map(|s| ((*s * 32767.0) as i16).to_le_bytes()));
                    if file.write_all(&bytes).is_err() {
                        return;
                    }
                    written += frames as u64;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Ok(FileWriter {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for FileWriter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn wav_header(rate: u32, data_size: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend(b"RIFF");
    h.extend((data_size + 36).to_le_bytes());
    h.extend(b"WAVEfmt ");
    h.extend(16u32.to_le_bytes());
    h.extend(1u16.to_le_bytes());
    h.extend(2u16.to_le_bytes());
    h.extend(rate.to_le_bytes());
    h.extend((rate * 4).to_le_bytes());
    h.extend(4u16.to_le_bytes());
    h.extend(16u16.to_le_bytes());
    h.extend(b"data");
    h.extend(data_size.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustora_formats::audio::Pcm;

    fn clip(rate: u32, samples: Vec<i16>) -> Clip {
        Arc::new(Pcm {
            channels: 1,
            sample_rate: rate,
            samples,
        })
    }

    fn device(rate: u32) -> MixerDevice {
        MixerDevice {
            mixer: Arc::new(Mutex::new(Mixer::new(rate))),
            _output: Output::File(FileWriter {
                stop: Arc::new(AtomicBool::new(true)),
                thread: None,
            }),
        }
    }

    fn params(relative: bool, pos: WPos, attenuate: bool) -> PlayParams {
        PlayParams {
            looping: false,
            relative,
            pos,
            volume: 1.0,
            attenuate,
        }
    }

    /// Plays through the runtime-loaded ALSA output on its `null` device. Skipped without ALSA.
    #[cfg(target_os = "linux")]
    #[test]
    fn alsa_output_plays_a_clip() {
        let Ok((output, mixer)) = alsa::AlsaOutput::start("null", |rate| Arc::new(Mutex::new(Mixer::new(rate)))) else {
            return;
        };
        let d = MixerDevice {
            mixer,
            _output: Output::Alsa(output),
        };
        let id = d
            .play(&clip(48000, vec![1000; 4800]), params(true, WPos::new(0, 0, 0), false))
            .unwrap();
        let start = Instant::now();
        while !d.is_complete(id) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(d.is_complete(id));
    }

    #[test]
    fn plays_to_the_end_then_completes() {
        let d = device(100);
        let id = d.play(&clip(100, vec![16384; 4]), params(true, WPos::new(0, 0, 0), false)).unwrap();
        let mut out = [0.0; 6];
        d.lock().mix(&mut out, 2);
        assert!(!d.is_complete(id));
        assert!((out[0] - 0.5 * CENTER_GAIN).abs() < 1e-3);
        let mut out = [0.0; 4];
        d.lock().mix(&mut out, 2);
        assert!(d.is_complete(id));
        assert!(out[2] == 0.0);
    }

    #[test]
    fn pans_toward_the_side_of_the_source() {
        let d = device(100);
        d.set_listener(WPos::new(0, 0, 0));
        d.play(&clip(100, vec![16384; 10]), params(false, WPos::new(5000, 0, 0), false))
            .unwrap();
        let mut out = [0.0; 2];
        d.lock().mix(&mut out, 2);
        assert!(out[1] > out[0] * 2.0);
    }

    #[test]
    fn distance_lowers_the_volume() {
        let d = device(100);
        d.set_listener(WPos::new(0, 0, 0));
        d.play(&clip(100, vec![16384; 10]), params(false, WPos::new(0, 20000, 0), false))
            .unwrap();
        let mut out = [0.0; 2];
        d.lock().mix(&mut out, 2);
        let expected = 0.5 * CENTER_GAIN * REFERENCE_DISTANCE / (20000f32 * 20000.0 + 2133.0 * 2133.0).sqrt();
        assert!((out[0] - expected).abs() < 1e-3);
    }

    #[test]
    fn limits_instances_at_one_place() {
        let d = device(100);
        let c = clip(100, vec![1; 100]);
        let pos = WPos::new(100, 100, 0);
        assert!(d.play(&c, params(false, pos, true)).is_some());
        assert!(d.play(&c, params(false, pos, true)).is_some());
        assert!(d.play(&c, params(false, pos, true)).is_some());
        assert!(d.play(&c, params(false, pos, true)).is_none());
        assert!(d.play(&c, params(false, WPos::new(90000, 0, 0), true)).is_some());
        for _ in 0..5 {
            d.next_frame();
        }
        assert!(d.play(&c, params(false, pos, true)).is_some());
    }

    #[test]
    fn resamples_by_the_rate_ratio() {
        let d = device(200);
        let id = d.play(&clip(100, vec![0, 16384]), params(true, WPos::new(0, 0, 0), false)).unwrap();
        let mut out = [0.0; 4];
        d.lock().mix(&mut out, 2);
        assert!((out[2] - 0.25 * CENTER_GAIN).abs() < 1e-3);
        assert!(d.seek_position(id) > 0.0);
    }
}
