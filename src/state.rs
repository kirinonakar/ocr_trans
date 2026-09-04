use crate::capture;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct AppState {
    pub(crate) is_running: bool,
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
