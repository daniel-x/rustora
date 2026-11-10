// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! The Linux audio output: ALSA's `libasound.so.2`, loaded when sound starts. The binary does not link it, so it
//! also starts on systems without ALSA, such as minimal servers running `--headless`. Calling a library loaded at
//! runtime needs `unsafe`, which this module allows, like the Windows console module of the client.
#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use libloading::{Library, Symbol};

use crate::{Mixer, lock};

const RATE: u32 = 48000;
const CHANNELS: usize = 2;
const PERIOD_FRAMES: usize = 512;
const LATENCY_US: c_uint = 50_000;
const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

type PcmOpen = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type PcmSetParams = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type PcmWritei = unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long;
type PcmRecover = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type PcmClose = unsafe extern "C" fn(*mut c_void) -> c_int;

/// A playback thread that writes the mixer output to the ALSA `default` device until dropped.
pub struct AlsaOutput {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// The opened library and PCM handle, moved to the playback thread.
struct Pcm {
    library: Library,
    handle: *mut c_void,
}

// SAFETY: the PCM handle is used only by the thread that owns this value.
unsafe impl Send for Pcm {}

impl Pcm {
    fn open(device: &str) -> Result<Pcm, String> {
        // SAFETY: loading libasound runs its initializers, which have no requirements on the caller.
        let library = unsafe { Library::new("libasound.so.2") }.map_err(|e| e.to_string())?;
        let mut handle = std::ptr::null_mut();
        let name = CString::new(device).map_err(|e| e.to_string())?;
        // SAFETY: the signatures match the ALSA C API; `handle` receives the opened PCM.
        unsafe {
            let open: Symbol<PcmOpen> = library.get(b"snd_pcm_open\0").map_err(|e| e.to_string())?;
            let set_params: Symbol<PcmSetParams> = library.get(b"snd_pcm_set_params\0").map_err(|e| e.to_string())?;
            let close: Symbol<PcmClose> = library.get(b"snd_pcm_close\0").map_err(|e| e.to_string())?;
            let err = open(&mut handle, name.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0);
            if err < 0 {
                return Err(format!("snd_pcm_open failed with {err}"));
            }
            let err = set_params(
                handle,
                SND_PCM_FORMAT_S16_LE,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                CHANNELS as c_uint,
                RATE,
                1,
                LATENCY_US,
            );
            if err < 0 {
                close(handle);
                return Err(format!("snd_pcm_set_params failed with {err}"));
            }
        }
        Ok(Pcm { library, handle })
    }

    /// Writes interleaved frames, recovering from underruns. `false` when the device failed for good.
    fn write(&self, samples: &[i16]) -> bool {
        // SAFETY: the signatures match the ALSA C API and `samples` holds whole interleaved frames.
        unsafe {
            let (Ok(writei), Ok(recover)) = (
                self.library.get::<PcmWritei>(b"snd_pcm_writei\0"),
                self.library.get::<PcmRecover>(b"snd_pcm_recover\0"),
            ) else {
                return false;
            };
            let mut rest = samples;
            while !rest.is_empty() {
                let frames = (rest.len() / CHANNELS) as c_ulong;
                let n = writei(self.handle, rest.as_ptr().cast(), frames);
                if n < 0 {
                    if recover(self.handle, n as c_int, 1) < 0 {
                        return false;
                    }
                    continue;
                }
                rest = &rest[n as usize * CHANNELS..];
            }
        }
        true
    }
}

impl Drop for Pcm {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `snd_pcm_open` and is closed once.
        unsafe {
            if let Ok(close) = self.library.get::<PcmClose>(b"snd_pcm_close\0") {
                close(self.handle);
            }
        }
    }
}

impl AlsaOutput {
    /// Opens the ALSA PCM `device`, such as `default`, and starts mixing into a mixer that `make_mixer` creates for
    /// the sample rate.
    pub fn start(device: &str, make_mixer: impl FnOnce(u32) -> Arc<Mutex<Mixer>>) -> Result<(AlsaOutput, Arc<Mutex<Mixer>>), String> {
        let pcm = Pcm::open(device)?;
        let mixer = make_mixer(RATE);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread_mixer = mixer.clone();
        let thread = std::thread::spawn(move || {
            let mut buffer = vec![0.0f32; PERIOD_FRAMES * CHANNELS];
            let mut samples = vec![0i16; PERIOD_FRAMES * CHANNELS];
            while !thread_stop.load(Ordering::Relaxed) {
                lock(&thread_mixer).mix(&mut buffer, CHANNELS);
                for (o, s) in samples.iter_mut().zip(&buffer) {
                    *o = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                }
                if !pcm.write(&samples) {
                    log::warn!("The ALSA audio output failed");
                    return;
                }
            }
        });
        Ok((
            AlsaOutput {
                stop,
                thread: Some(thread),
            },
            mixer,
        ))
    }
}

impl Drop for AlsaOutput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
