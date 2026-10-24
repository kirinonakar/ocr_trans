use crate::capture;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct AppState {
    pub(crate) is_running: bool,
    pub(crate) worker_control: Arc<WorkerControl>,
    pub(crate) capture_rect: Option<capture::CaptureRect>,
    pub(crate) api_endpoint: String,
    pub(crate) api_key: String,
    pub(crate) model_name: String,
    pub(crate) interval_sec: f32,
    pub(crate) system_prompt: String,
    pub(crate) temperature: f32,
    pub(crate) thinking_level: String,
    pub(crate) provider: String,
    pub(crate) last_text: String,
    pub(crate) base_font_size: f32,
    pub(crate) overlay_bg_color: slint::Color,
    pub(crate) overlay_text_color: slint::Color,
    pub(crate) overlay_bg_opacity: f32,
    pub(crate) use_textbox: bool,
    pub(crate) capture_folder: String,
    pub(crate) selection_origin_x: i32,
    pub(crate) selection_origin_y: i32,
    pub(crate) selection_scale: f32,
    pub(crate) selection_screenshot: Option<Arc<image::RgbaImage>>,
    pub(crate) pending_selection: Option<SelectionPurpose>,
    pub(crate) scroll_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) recording: bool,
    pub(crate) recording_paused: bool,
    pub(crate) recording_started_at: Option<Instant>,
    pub(crate) recording_paused_at: Option<Instant>,
    pub(crate) recording_paused_total: Duration,
    pub(crate) recording_path: Option<std::path::PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionPurpose {
    ContinuousOcr,
    Capture,
    Record,
    ScrollCapture,
    Ocr,
    OcrTranslate,
    Vlm,
    ColorPicker,
    Ruler,
}

/// A single revision wakes interval waits and cancels the previous async request.
pub(crate) struct WorkerControl {
    revision: Mutex<u64>,
    wake: Condvar,
    changed: tokio::sync::watch::Sender<u64>,
}

impl Default for WorkerControl {
    fn default() -> Self {
        Self {
            revision: Mutex::new(0),
            wake: Condvar::new(),
            changed: tokio::sync::watch::channel(0).0,
        }
    }
}

impl WorkerControl {
    pub(crate) fn signal(&self) {
        let mut revision = self.revision.lock().unwrap();
        *revision = revision.wrapping_add(1);
        self.changed.send_replace(*revision);
        self.wake.notify_all();
    }

    pub(crate) fn revision(&self) -> u64 {
        *self.revision.lock().unwrap()
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub(crate) fn wait(&self, revision: u64, duration: Duration) {
        let guard = self.revision.lock().unwrap();
        let _ = self
            .wake
            .wait_timeout_while(guard, duration, |value| *value == revision)
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_wait_is_woken_by_start_and_does_not_lose_early_signals() {
        let control = Arc::new(WorkerControl::default());
        let revision = control.revision();
        let (ready, wait_started) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        let waiter = control.clone();
        let worker = std::thread::spawn(move || {
            ready.send(()).unwrap();
            waiter.wait(revision, Duration::from_secs(60));
            done.send(()).unwrap();
        });
        wait_started.recv().unwrap();
        control.signal();
        finished.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        // Signaling before wait() also returns immediately.
        let started = Instant::now();
        control.wait(revision, Duration::from_secs(60));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
