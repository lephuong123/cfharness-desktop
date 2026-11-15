//! Speech backend for Windows (any non-macOS build with `--features whisper`): whisper.cpp through `whisper-rs`,
//! microphone through `cpal` (WASAPI on Windows).
//!
//! - Model: `ggml-small-q5_1.bin` (multilingual "small", quantized, ~190 MB) from the whisper.cpp repo on Hugging Face,
//!   downloaded on FIRST use into `<app data>/whisper/` only after the user confirms a native dialog; the file is
//!   checked against the pinned SHA-256 before use. Declining → start fails with code `model-declined`.
//! - whisper is not streaming: audio is cut into utterances by a simple energy VAD (end after ~0.8 s of silence, at
//!   most 15 s) and each utterance is transcribed into ONE final result. No interim results (voice.js handles that:
//!   its silence timer starts from the final text).
//!
//! Status: builds with `--features whisper` (checked on macOS with cmake); NOT verified on real Windows hardware here —
//! see README "Giọng nói trên Windows".

use super::{Event, Sink};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

pub const MODEL_FILE: &str = "ggml-small-q5_1.bin";
pub const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin";
pub const MODEL_SHA256: &str = "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb";
pub const MODEL_BYTES: u64 = 190_085_487;
pub const RATE: u32 = 16_000;

const FRAME: usize = (RATE as usize) / 100 * 3; // 30 ms
const SILENCE_END_FRAMES: usize = 27; // ~0.8 s
const MAX_UTTERANCE: usize = RATE as usize * 15;
const MIN_UTTERANCE: usize = RATE as usize / 4;

// ------------------------------------------------------------------------------------------------ pure helpers

/// Interleaved samples → mono.
pub fn to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect()
}

/// Linear resampling to 16 kHz (good enough for speech recognition).
pub fn resample(input: &[f32], from: u32) -> Vec<f32> {
    if from == RATE || input.is_empty() || from == 0 {
        return input.to_vec();
    }
    let ratio = from as f64 / RATE as f64;
    let n = (input.len() as f64 / ratio).floor() as usize;
    (0..n)
        .map(|i| {
            let pos = i as f64 * ratio;
            let j = pos.floor() as usize;
            let frac = (pos - j as f64) as f32;
            let a = input[j.min(input.len() - 1)];
            let b = input[(j + 1).min(input.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

pub fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32).sqrt()
}

/// Energy VAD cutting 16 kHz mono audio into utterances.
pub struct Segmenter {
    buf: Vec<f32>,
    pending: Vec<f32>,
    speaking: bool,
    silent_frames: usize,
    voiced_frames: usize,
    noise: f32,
}

impl Default for Segmenter {
    fn default() -> Self {
        Segmenter { buf: vec![], pending: vec![], speaking: false, silent_frames: 0, voiced_frames: 0, noise: 0.01 }
    }
}

impl Segmenter {
    /// Feed samples; returns finished utterances.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<f32>> {
        let mut out = vec![];
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= FRAME {
            let frame: Vec<f32> = self.pending.drain(..FRAME).collect();
            let e = rms(&frame);
            let threshold = (self.noise * 3.0).max(0.012);
            if e > threshold {
                self.speaking = true;
                self.silent_frames = 0;
                self.voiced_frames += 1;
            } else {
                // track background noise while quiet
                self.noise = self.noise * 0.95 + e * 0.05;
                if self.speaking {
                    self.silent_frames += 1;
                }
            }
            if self.speaking {
                self.buf.extend_from_slice(&frame);
            }
            let ended = self.speaking && self.silent_frames >= SILENCE_END_FRAMES;
            if ended || self.buf.len() >= MAX_UTTERANCE {
                let u = std::mem::take(&mut self.buf);
                let voiced = self.voiced_frames * FRAME;
                self.speaking = false;
                self.silent_frames = 0;
                self.voiced_frames = 0;
                if voiced >= MIN_UTTERANCE {
                    out.push(u);
                }
            }
        }
        out
    }
}

// ------------------------------------------------------------------------------------------------ model

fn model_path() -> Option<PathBuf> {
    super::data_dir().map(|d| d.join("whisper").join(MODEL_FILE))
}

fn sha256_file(p: &std::path::Path) -> Option<String> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).ok()?;
    Some(format!("{:x}", h.finalize()))
}

fn confirm_download() -> bool {
    let msg = format!(
        "Nhận dạng giọng nói trên máy này dùng mô hình whisper (≈ {} MB, tải một lần từ huggingface.co, lưu trong thư mục dữ liệu của app). Tải ngay?",
        MODEL_BYTES / 1_000_000
    );
    rfd::MessageDialog::new()
        .set_title("Tải mô hình nhận dạng giọng nói")
        .set_description(msg)
        .set_buttons(rfd::MessageButtons::OkCancelCustom("Tải".into(), "Không".into()))
        .show()
        == rfd::MessageDialogResult::Custom("Tải".into())
}

async fn download(dest: &std::path::Path) -> Result<(), String> {
    // Hugging Face redirects to its CDN: follow https redirects only.
    let policy = reqwest::redirect::Policy::custom(|a| {
        if a.previous().len() >= 5 || a.url().scheme() != "https" {
            a.error("redirect")
        } else {
            a.follow()
        }
    });
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(policy)
        .user_agent(format!("CfharnessDesktop/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "network".to_string())?;
    let mut resp = client.get(MODEL_URL).send().await.map_err(|_| "network".to_string())?;
    if !resp.status().is_success() {
        return Err("network".into());
    }
    let tmp = dest.with_extension("part");
    let mut f = std::fs::File::create(&tmp).map_err(|_| "model-io".to_string())?;
    let mut h = Sha256::new();
    let mut total: u64 = 0;
    while let Some(chunk) = resp.chunk().await.map_err(|_| "network".to_string())? {
        total += chunk.len() as u64;
        if total > MODEL_BYTES + 1024 {
            let _ = std::fs::remove_file(&tmp);
            return Err("model-corrupt".into());
        }
        h.update(&chunk);
        f.write_all(&chunk).map_err(|_| "model-io".to_string())?;
    }
    drop(f);
    if format!("{:x}", h.finalize()) != MODEL_SHA256 {
        let _ = std::fs::remove_file(&tmp);
        return Err("model-corrupt".into());
    }
    std::fs::rename(&tmp, dest).map_err(|_| "model-io".to_string())
}

fn ensure_model() -> Result<PathBuf, String> {
    let p = model_path().ok_or("model-io")?;
    if p.is_file() {
        if sha256_file(&p).as_deref() == Some(MODEL_SHA256) {
            return Ok(p);
        }
        let _ = std::fs::remove_file(&p);
    }
    if !confirm_download() {
        return Err("model-declined".into());
    }
    std::fs::create_dir_all(p.parent().ok_or("model-io")?).map_err(|_| "model-io".to_string())?;
    log::info!("đang tải mô hình whisper ({} MB)…", MODEL_BYTES / 1_000_000);
    tauri::async_runtime::block_on(download(&p))?;
    Ok(p)
}

static CTX: OnceLock<Mutex<Option<Arc<whisper_rs::WhisperContext>>>> = OnceLock::new();

fn context() -> Result<Arc<whisper_rs::WhisperContext>, String> {
    let slot = CTX.get_or_init(|| Mutex::new(None));
    let mut g = slot.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(c) = g.as_ref() {
        return Ok(c.clone());
    }
    let path = ensure_model()?;
    let ctx = whisper_rs::WhisperContext::new_with_params(&path, whisper_rs::WhisperContextParameters::default())
        .map_err(|_| "model-corrupt".to_string())?;
    let c = Arc::new(ctx);
    *g = Some(c.clone());
    Ok(c)
}

fn transcribe(ctx: &whisper_rs::WhisperContext, lang: &str, audio: &[f32]) -> Option<String> {
    let mut state = ctx.create_state().ok()?;
    let mut p = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
    p.set_language(Some(if lang.starts_with("en") { "en" } else { "vi" }));
    p.set_n_threads(std::thread::available_parallelism().map(|n| n.get().min(8) as i32).unwrap_or(4));
    p.set_print_progress(false);
    p.set_print_realtime(false);
    p.set_print_special(false);
    p.set_print_timestamps(false);
    p.set_no_context(true);
    p.set_suppress_blank(true);
    state.full(p, audio).ok()?;
    let mut text = String::new();
    for seg in state.as_iter() {
        if let Ok(s) = seg.to_str_lossy() {
            text.push_str(&s);
        }
    }
    let t = text.trim().to_string();
    (!t.is_empty()).then_some(t)
}

// ------------------------------------------------------------------------------------------------ session

struct Session {
    stop: Arc<AtomicBool>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

pub fn available(_lang: &str) -> bool {
    use cpal::traits::HostTrait;
    cpal::default_host().default_input_device().is_some()
}

pub fn start(lang: &str, _interim: bool, sink: Sink) -> Result<(), String> {
    stop();
    let ctx = context()?;
    let stop_flag = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let (seg_tx, seg_rx) = mpsc::channel::<Vec<f32>>();
    let sink = Arc::new(sink);
    // Capture thread owns the cpal stream (not Send on every platform).
    {
        let stop_flag = stop_flag.clone();
        std::thread::spawn(move || capture(stop_flag, ready_tx, seg_tx));
    }
    match ready_rx.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(())) => {}
        Ok(Err(code)) => return Err(code),
        Err(_) => {
            stop_flag.store(true, Ordering::SeqCst);
            return Err("audio-capture".into());
        }
    }
    {
        let stop_flag = stop_flag.clone();
        let lang = lang.to_string();
        let sink = sink.clone();
        std::thread::spawn(move || {
            while let Ok(u) = seg_rx.recv() {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                if let Some(t) = transcribe(&ctx, &lang, &u) {
                    if !stop_flag.load(Ordering::SeqCst) {
                        sink(Event::Result { text: t, is_final: true });
                    }
                }
            }
        });
    }
    *SESSION.lock().unwrap_or_else(|p| p.into_inner()) = Some(Session { stop: stop_flag });
    Ok(())
}

fn capture(stop: Arc<AtomicBool>, ready: mpsc::SyncSender<Result<(), String>>, out: mpsc::Sender<Vec<f32>>) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let Some(dev) = cpal::default_host().default_input_device() else {
        let _ = ready.send(Err("audio-capture".into()));
        return;
    };
    let Ok(cfg) = dev.default_input_config() else {
        let _ = ready.send(Err("audio-capture".into()));
        return;
    };
    let channels = cfg.channels() as usize;
    let rate = cfg.sample_rate();
    let seg = Arc::new(Mutex::new(Segmenter::default()));
    let feed = move |mono: Vec<f32>| {
        let s = resample(&mono, rate);
        let done = seg.lock().unwrap_or_else(|p| p.into_inner()).push(&s);
        for u in done {
            let _ = out.send(u);
        }
    };
    let err_fn = |e| log::warn!("micro lỗi: {e}");
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => {
            dev.build_input_stream(cfg.config(), move |d: &[f32], _: &_| feed(to_mono(d, channels)), err_fn, None)
        }
        cpal::SampleFormat::I16 => dev.build_input_stream(
            cfg.config(),
            move |d: &[i16], _: &_| {
                let f: Vec<f32> = d.iter().map(|x| *x as f32 / i16::MAX as f32).collect();
                feed(to_mono(&f, channels))
            },
            err_fn,
            None,
        ),
        _ => {
            let _ = ready.send(Err("audio-capture".into()));
            return;
        }
    };
    let Ok(stream) = stream else {
        let _ = ready.send(Err("not-allowed".into()));
        return;
    };
    if stream.play().is_err() {
        let _ = ready.send(Err("audio-capture".into()));
        return;
    }
    let _ = ready.send(Ok(()));
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(stream);
}

pub fn stop() {
    if let Some(s) = SESSION.lock().unwrap_or_else(|p| p.into_inner()).take() {
        s.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_and_resample() {
        assert_eq!(to_mono(&[1.0, 3.0, 2.0, 4.0], 2), vec![2.0, 3.0]);
        assert_eq!(resample(&vec![0.5; 48_000], 48_000).len(), 16_000);
        assert_eq!(resample(&[1.0, 2.0], RATE), vec![1.0, 2.0]);
    }

    #[test]
    fn segmenter_cuts_on_silence() {
        let mut s = Segmenter::default();
        let quiet = vec![0.0f32; RATE as usize];
        let loud: Vec<f32> = (0..RATE as usize).map(|i| if i % 2 == 0 { 0.3 } else { -0.3 }).collect();
        assert!(s.push(&quiet).is_empty());
        assert!(s.push(&loud).is_empty());
        let out = s.push(&quiet);
        assert_eq!(out.len(), 1);
        assert!(out[0].len() >= RATE as usize);
        // too short a blip is dropped
        let blip = vec![0.3f32; FRAME * 2];
        assert!(s.push(&blip).is_empty());
        assert!(s.push(&quiet).is_empty());
    }
}
