//! macOS backend: `native/speech_mac.m` (SFSpeechRecognizer + AVAudioEngine), compiled by build.rs.

use super::{Event, Sink};
use std::ffi::{c_char, c_int, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Cb = extern "C" fn(kind: c_int, text: *const c_char, is_final: c_int, session: u64);

extern "C" {
    fn cfh_speech_supported(lang: *const c_char) -> c_int;
    fn cfh_speech_start(lang: *const c_char, interim: c_int, cb: Cb, session: u64);
    fn cfh_speech_stop();
}

const RESULT: c_int = 0;
const END: c_int = 1;
const ERROR: c_int = 2;
const STARTED: c_int = 3;

struct State {
    session: u64,
    sink: Option<Arc<Sink>>,
    started: Option<SyncSender<Result<(), String>>>,
}

static STATE: Mutex<State> = Mutex::new(State { session: 0, sink: None, started: None });
static NEXT: AtomicU64 = AtomicU64::new(0);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

extern "C" fn on_event(kind: c_int, text: *const c_char, is_final: c_int, session: u64) {
    let text =
        if text.is_null() { String::new() } else { unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned() };
    let (sink, started) = {
        let mut st = state();
        if st.session != session || session == 0 {
            return;
        }
        let started = if kind == STARTED || (kind == ERROR && st.started.is_some()) { st.started.take() } else { None };
        if kind == ERROR && started.is_some() {
            // failed before listening: the start promise rejects, no event
            st.session = 0;
            st.sink = None;
        }
        (st.sink.clone(), started)
    };
    if let Some(tx) = started {
        let _ = tx.send(if kind == STARTED { Ok(()) } else { Err(text) });
        return;
    }
    let Some(sink) = sink else { return };
    match kind {
        RESULT => sink(Event::Result { text, is_final: is_final != 0 }),
        END => sink(Event::End),
        ERROR => sink(Event::Error(text)),
        _ => {}
    }
}

pub fn available(lang: &str) -> bool {
    let Ok(c) = CString::new(lang) else { return false };
    unsafe { cfh_speech_supported(c.as_ptr()) != 0 }
}

/// Blocking: returns once the microphone is listening (permission prompts included) or with an error code.
pub fn start(lang: &str, interim: bool, sink: Sink) -> Result<(), String> {
    let lang = CString::new(lang).map_err(|_| "language-not-supported".to_string())?;
    let id = NEXT.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = sync_channel(1);
    {
        let mut st = state();
        st.session = id;
        st.sink = Some(Arc::new(sink));
        st.started = Some(tx);
    }
    unsafe { cfh_speech_start(lang.as_ptr(), c_int::from(interim), on_event, id) };
    match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(r) => r,
        Err(_) => {
            stop();
            Err("start".into())
        }
    }
}

pub fn stop() {
    {
        let mut st = state();
        st.session = 0;
        st.sink = None;
        st.started = None;
    }
    unsafe { cfh_speech_stop() };
}
