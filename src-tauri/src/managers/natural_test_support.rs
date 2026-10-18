//! Test support shared by the natural-voice test modules (fake output stream + fake device).
//! Compiled for tests only. Complete: nothing here is a stub.
#![allow(dead_code)]

use crate::managers::natural_sink::{PlaybackCtl, StreamFactory};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Default)]
struct FakeState {
    opens: usize,
    closes: usize,
    ctl: Option<Arc<PlaybackCtl>>,
    fail_open: bool,
    fail_rate: bool,
}

/// A `StreamFactory` that records opens/closes and hands the test the `PlaybackCtl`.
pub struct FakeStreamFactory {
    rate: u32,
    state: Arc<Mutex<FakeState>>,
}

struct CloseGuard {
    state: Arc<Mutex<FakeState>>,
}

impl Drop for CloseGuard {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            s.closes += 1;
        }
    }
}

impl FakeStreamFactory {
    pub fn new(rate: u32) -> Arc<Self> {
        Arc::new(Self {
            rate,
            state: Arc::new(Mutex::new(FakeState::default())),
        })
    }
    pub fn opens(&self) -> usize {
        self.state.lock().unwrap().opens
    }
    pub fn closes(&self) -> usize {
        self.state.lock().unwrap().closes
    }
    /// The control passed to the most recent successful `open`.
    pub fn ctl(&self) -> Option<Arc<PlaybackCtl>> {
        self.state.lock().unwrap().ctl.clone()
    }
    pub fn set_fail_open(&self, fail: bool) {
        self.state.lock().unwrap().fail_open = fail;
    }
    pub fn set_fail_rate(&self, fail: bool) {
        self.state.lock().unwrap().fail_rate = fail;
    }
}

impl StreamFactory for FakeStreamFactory {
    fn device_rate(&self) -> Result<u32, String> {
        if self.state.lock().unwrap().fail_rate {
            Err("no output device".into())
        } else {
            Ok(self.rate)
        }
    }
    fn open(&self, ctl: Arc<PlaybackCtl>) -> Result<Box<dyn Send>, String> {
        let mut s = self.state.lock().unwrap();
        if s.fail_open {
            return Err("cannot open stream".into());
        }
        s.opens += 1;
        s.ctl = Some(ctl);
        drop(s);
        Ok(Box::new(CloseGuard {
            state: self.state.clone(),
        }))
    }
}

/// A fake audio callback: a thread that periodically renders `frames` samples from the
/// factory's current control and records everything it "played".
pub struct FakeDevice {
    out: Arc<Mutex<Vec<f32>>>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeDevice {
    pub fn start(factory: Arc<FakeStreamFactory>, frames: usize, tick: Duration) -> Self {
        let out = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let (o, s, p) = (out.clone(), stop.clone(), paused.clone());
        let handle = std::thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                if !p.load(Ordering::SeqCst) {
                    if let Some(ctl) = factory.ctl() {
                        let mut buf = vec![0.0f32; frames];
                        ctl.render(&mut buf, Instant::now());
                        o.lock().unwrap().extend_from_slice(&buf);
                    }
                }
                std::thread::sleep(tick);
            }
        });
        Self {
            out,
            stop,
            paused,
            handle: Some(handle),
        }
    }
    /// Everything rendered so far (including zeros).
    pub fn rendered(&self) -> Vec<f32> {
        self.out.lock().unwrap().clone()
    }
    /// Number of non-zero samples "played" so far.
    pub fn nonzero(&self) -> usize {
        self.out
            .lock()
            .unwrap()
            .iter()
            .filter(|x| **x != 0.0)
            .count()
    }
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
}

impl Drop for FakeDevice {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
