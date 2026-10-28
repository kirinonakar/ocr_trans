use crate::shortcuts::{HotkeyState, ToolbarAction};
use crate::state::AppState;
use crate::text_layout::{calculate_font_size, clean_text};
use crate::{
    api, capture, CaptureToolbarWindow, MainWindow, OverlayWindow, SelectionWindow, TextboxWindow,
};
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn start(
    main_window: &MainWindow,
    overlay_window: &OverlayWindow,
    selection_window: &SelectionWindow,
    textbox_window: &TextboxWindow,
    capture_toolbar: &CaptureToolbarWindow,
    state: Arc<Mutex<AppState>>,
    http_client: reqwest::Client,
    hotkey_state: Arc<Mutex<HotkeyState>>,
    esc_hotkey: HotKey,
) {
    let state_for_worker = state.clone();
    // Keep capture, OCR and request waits off the UI thread.
    // Uses slint::invoke_from_event_loop() instead of tokio channels to guarantee
    // the slint event loop wakes up for every UI update (tokio wakers don't reliably
    // wake the slint event loop, causing freezes after prolonged use).
    let http_worker = http_client.clone();
    let runtime_handle = tokio::runtime::Handle::current();
    let overlay_weak_worker = overlay_window.as_weak();
    let main_weak_worker = main_window.as_weak();
    let textbox_weak_worker = textbox_window.as_weak();
    std::thread::spawn(move || {
        let control = state_for_worker.lock().unwrap().worker_control.clone();
        let mut observed_revision = u64::MAX;
        let mut gate = ChangeGate::default();
        let mut failures = 0u32;
        let mut history = Vec::new();
        let mut halted = false;
        loop {
            // Subscribe before the snapshot, so a stop between the snapshot and select cannot
            // be lost. Revisions also reject callbacks that were already queued on Slint.
            let mut cancel = control.subscribe();
            let (running, rect, client, glossary, interval, revision) = {
                let state = state_for_worker.lock().unwrap();
                (
                    state.is_running,
                    state.capture_rect,
                    api::ApiClient::new(
                        http_worker.clone(),
                        state.api_endpoint.clone(),
                        state.api_key.clone(),
                        state.model_name.clone(),
                        state.system_prompt.clone(),
                        state.temperature,
                        state.thinking_level.clone(),
                        state.provider.clone(),
                    ),
                    state.translation_glossary.clone(),
                    state.interval_sec,
                    control.revision(),
                )
            };
            if revision != observed_revision {
                gate = ChangeGate::default();
                failures = 0;
                history.clear();
                halted = false;
                observed_revision = revision;
            }
            if !running || rect.is_none() || halted {
                control.wait(revision, Duration::from_secs(60));
                continue;
            }
            let one_shot = interval <= 0.01;
            let interval = if interval.is_finite() {
                interval.clamp(0.1, 3600.0)
            } else {
                1.0
            };
            let mut delay = Duration::from_secs_f32(interval);
            match capture::capture_area(&rect.unwrap(), &None) {
                Ok(image) => {
                    let (language, skip_empty_ocr) = crate::settings::continuous_ocr_settings();
                    let text = if one_shot {
                        None
                    } else {
                        let pixels = crate::capture_workflow::rgba_to_bgra_bytes(&image);
                        match crate::ocr::recognize_text_with_cancel(
                            &pixels,
                            image.width(),
                            image.height(),
                            &language,
                            || control.revision() != revision,
                        ) {
                            Ok(text) => Some(text.split_whitespace().collect::<Vec<_>>().join(" ")),
                            Err(error) => {
                                log::debug!(
                                    "Continuous OCR unavailable; using pixel comparison: {error:?}"
                                );
                                None
                            }
                        }
                    };
                    if control.revision() != revision {
                        continue;
                    }
                    let decision = gate.observe(text.as_deref(), &image, one_shot, skip_empty_ocr);
                    if decision == GateDecision::Clear {
                        failures = 0;
                        queue_worker_result(
                            &state_for_worker,
                            revision,
                            &overlay_weak_worker,
                            &main_weak_worker,
                            &textbox_weak_worker,
                            String::new(),
                            false,
                            true,
                            false,
                        );
                    }
                    if decision == GateDecision::Translate {
                        let client = client.with_context(&glossary, &history);
                        queue_worker_result(
                            &state_for_worker,
                            revision,
                            &overlay_weak_worker,
                            &main_weak_worker,
                            &textbox_weak_worker,
                            "Searching...".into(),
                            true,
                            false,
                            false,
                        );
                        let mut last_update = None::<std::time::Instant>;
                        let result = runtime_handle.block_on(in_generation(
                            &mut cancel,
                            revision,
                            client.translate_image_stream(image.clone(), |partial| {
                                if last_update
                                    .is_some_and(|time| time.elapsed() < Duration::from_millis(80))
                                {
                                    return;
                                }
                                last_update = Some(std::time::Instant::now());
                                queue_worker_result(
                                    &state_for_worker,
                                    revision,
                                    &overlay_weak_worker,
                                    &main_weak_worker,
                                    &textbox_weak_worker,
                                    partial.to_string(),
                                    false,
                                    false,
                                    false,
                                );
                            }),
                        ));
                        let Some(result) = result else {
                            continue;
                        };
                        if control.revision() != revision {
                            continue;
                        }
                        match result {
                            Ok(translated) => {
                                api::remember_line(
                                    &mut history,
                                    text.as_deref().unwrap_or_default(),
                                    &translated,
                                );
                                gate.accept(text, image);
                                failures = 0;
                                queue_worker_result(
                                    &state_for_worker,
                                    revision,
                                    &overlay_weak_worker,
                                    &main_weak_worker,
                                    &textbox_weak_worker,
                                    translated,
                                    false,
                                    true,
                                    one_shot,
                                );
                            }
                            Err(error) => {
                                log::error!("API Error: {error:?}");
                                gate.retry();
                                failures = failures.saturating_add(1);
                                let retry = continuous_retry_delay(&error, failures);
                                halted = retry.is_none();
                                delay = retry.unwrap_or(Duration::from_secs(60));
                                let summary = api::error_summary(&error);
                                let message = if halted {
                                    format!("Stopped after {failures} failure(s): {summary}")
                                } else {
                                    format!("Retry in {}s: {summary}", delay.as_secs())
                                };
                                queue_worker_result(
                                    &state_for_worker,
                                    revision,
                                    &overlay_weak_worker,
                                    &main_weak_worker,
                                    &textbox_weak_worker,
                                    message,
                                    false,
                                    false,
                                    halted,
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    log::warn!("Continuous capture failed: {error:?}");
                    failures = failures.saturating_add(1);
                    let retry = continuous_retry_delay(&error, failures);
                    halted = retry.is_none();
                    delay = retry.unwrap_or(Duration::from_secs(60));
                    let summary = api::error_summary(&error);
                    let message = if halted {
                        format!("Stopped after {failures} failure(s): {summary}")
                    } else {
                        format!("Retry in {}s: {summary}", delay.as_secs())
                    };
                    queue_worker_result(
                        &state_for_worker,
                        revision,
                        &overlay_weak_worker,
                        &main_weak_worker,
                        &textbox_weak_worker,
                        message,
                        false,
                        false,
                        halted,
                    );
                }
            }
            control.wait(revision, delay);
        }
    });

    // Hotkey Event Loop - Dedicated Thread for Responsiveness
    let main_weak_hk = main_window.as_weak();
    let selection_weak_hk = selection_window.as_weak();
    let toolbar_weak_hk = capture_toolbar.as_weak();
    let scroll_state_hk = state.clone();
    let state_hk = hotkey_state;
    let esc_id = esc_hotkey.id();
    std::thread::spawn(move || loop {
        let Ok(event) = GlobalHotKeyEvent::receiver().recv() else {
            continue;
        };
        if event.state != global_hotkey::HotKeyState::Pressed {
            continue;
        }

        // Resolve the current bindings on every event so shortcuts stay in sync after the
        // settings window re-registers them.
        let (select_id, start_id, tool1, tool1_action, tool2, tool2_action, tool3, tool3_action) = {
            let state = state_hk.lock().unwrap();
            (
                state.registered.select_area.map(|hotkey| hotkey.id()),
                state.registered.start.map(|hotkey| hotkey.id()),
                state.registered.toolbar1.map(|hotkey| hotkey.id()),
                ToolbarAction::from_token(&state.config.toolbar1_action),
                state.registered.toolbar2.map(|hotkey| hotkey.id()),
                ToolbarAction::from_token(&state.config.toolbar2_action),
                state.registered.toolbar3.map(|hotkey| hotkey.id()),
                ToolbarAction::from_token(&state.config.toolbar3_action),
            )
        };

        if Some(event.id) == select_id {
            let main_weak = main_weak_hk.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(main) = main_weak.upgrade() {
                    main.invoke_select_area_clicked();
                }
            });
        } else if Some(event.id) == start_id {
            let main_weak = main_weak_hk.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(main) = main_weak.upgrade() {
                    main.invoke_start_stop_clicked();
                }
            });
        } else if Some(event.id) == tool1 {
            if let Some(action) = tool1_action {
                dispatch_toolbar_action(&toolbar_weak_hk, action);
            }
        } else if Some(event.id) == tool2 {
            if let Some(action) = tool2_action {
                dispatch_toolbar_action(&toolbar_weak_hk, action);
            }
        } else if Some(event.id) == tool3 {
            if let Some(action) = tool3_action {
                dispatch_toolbar_action(&toolbar_weak_hk, action);
            }
        } else if event.id == esc_id {
            if let Some(cancel) = scroll_state_hk.lock().unwrap().scroll_cancel.as_ref() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                continue;
            }
            let selection_weak = selection_weak_hk.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(selection) = selection_weak.upgrade() {
                    selection.invoke_closed();
                }
            });
        }
    });
}

/// Runs a capture-toolbar action from a global shortcut by invoking the same callback that the
/// toolbar button uses, so the behavior stays identical.
fn dispatch_toolbar_action(toolbar: &slint::Weak<CaptureToolbarWindow>, action: ToolbarAction) {
    let toolbar = toolbar.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(toolbar) = toolbar.upgrade() else {
            return;
        };
        match action {
            ToolbarAction::Fullscreen => toolbar.invoke_fullscreen_clicked(),
            ToolbarAction::Window => toolbar.invoke_window_clicked(),
            ToolbarAction::Region => toolbar.invoke_region_clicked(),
            ToolbarAction::Ocr => toolbar.invoke_ocr_clicked(),
            ToolbarAction::OcrTranslate => toolbar.invoke_translate_clicked(),
            ToolbarAction::Vlm => toolbar.invoke_vlm_clicked(),
            ToolbarAction::ColorPicker => toolbar.invoke_color_picker_clicked(),
            ToolbarAction::Ruler => toolbar.invoke_ruler_clicked(),
        }
    });
}

const MAX_CONSECUTIVE_FAILURES: u32 = 5;

fn continuous_retry_delay(error: &anyhow::Error, failures: u32) -> Option<Duration> {
    if failures >= MAX_CONSECUTIVE_FAILURES {
        None
    } else {
        api::retry_delay(error, failures)
    }
}

async fn in_generation<T>(
    cancel: &mut tokio::sync::watch::Receiver<u64>,
    revision: u64,
    request: impl std::future::Future<Output = T>,
) -> Option<T> {
    if *cancel.borrow_and_update() != revision {
        return None;
    }
    tokio::select! {
        biased;
        _ = cancel.changed() => None,
        result = request => Some(result),
    }
}

/// A successful empty OCR result remains distinct from an OCR error (None).
/// Users can opt out of pixel fallback on empty OCR when detecting subtitle disappearance.
#[derive(Debug, PartialEq, Eq)]
enum GateDecision {
    Wait,
    Translate,
    Clear,
}

#[derive(Default)]
struct ChangeGate {
    candidate_text: Option<String>,
    candidate_image: Option<image::RgbaImage>,
    accepted_text: Option<String>,
    accepted_image: Option<image::RgbaImage>,
}

impl ChangeGate {
    fn observe(
        &mut self,
        text: Option<&str>,
        image: &image::RgbaImage,
        one_shot: bool,
        skip_empty_ocr: bool,
    ) -> GateDecision {
        if one_shot {
            return GateDecision::Translate;
        }
        if text == Some("") && skip_empty_ocr {
            if self.accepted_text.as_deref() == Some("")
                && self.candidate_text.as_deref() == Some("")
            {
                return GateDecision::Wait;
            }
            *self = Self::default();
            self.accepted_text = Some(String::new());
            self.candidate_text = Some(String::new());
            return GateDecision::Clear;
        }
        // Compatibility mode allows outlined/unrecognized text to use stable pixels.
        let text = text.filter(|text| !text.is_empty());
        let stable = if text.is_none() {
            self.candidate_text.is_none()
                && self.candidate_image.is_some()
                && !capture::is_changed(&self.candidate_image, image, 0.01)
        } else {
            self.candidate_text.as_deref() == text
        };
        let changed = if text.is_none() {
            capture::is_changed(&self.accepted_image, image, 0.01)
        } else {
            self.accepted_text.as_deref() != text
        };
        if text.is_none() {
            self.candidate_text = None;
            self.candidate_image = Some(image.clone());
        } else {
            self.candidate_text = text.map(str::to_string);
            self.candidate_image = None;
        }
        if stable && changed {
            GateDecision::Translate
        } else {
            GateDecision::Wait
        }
    }

    fn accept(&mut self, text: Option<String>, image: image::RgbaImage) {
        self.accepted_text = text;
        self.accepted_image = Some(image);
    }

    fn retry(&mut self) {
        self.accepted_text = None;
        self.accepted_image = None;
    }
}

fn queue_worker_result(
    state: &Arc<Mutex<AppState>>,
    revision: u64,
    overlay: &slint::Weak<OverlayWindow>,
    main: &slint::Weak<MainWindow>,
    textbox: &slint::Weak<TextboxWindow>,
    text: String,
    searching: bool,
    success: bool,
    one_shot: bool,
) {
    let (state, ow, mw, tw) = (
        state.clone(),
        overlay.clone(),
        main.clone(),
        textbox.clone(),
    );
    let _ = slint::invoke_from_event_loop(move || {
        {
            let mut state = state.lock().unwrap();
            if !state.is_running || state.worker_control.revision() != revision {
                return;
            }
            if success {
                state.last_text = text.clone();
            }
            if one_shot {
                state.is_running = false;
            }
        }
        let Some(main) = mw.upgrade() else {
            return;
        };
        let display_text = clean_text(&text);
        if let Some(overlay) = ow.upgrade() {
            overlay.set_translated_text(display_text.clone().into());
            overlay.set_is_searching(searching);
            overlay.set_font_size(calculate_font_size(
                &display_text,
                overlay.get_window_w(),
                overlay.get_window_h(),
                main.get_base_font_size(),
            ));
            overlay.set_bg_color(main.get_overlay_bg_color());
            overlay.set_text_color(main.get_overlay_text_color());
            overlay.set_bg_opacity(if main.get_use_textbox() {
                0.1
            } else {
                main.get_overlay_bg_opacity()
            });
            overlay.set_hide_text(main.get_use_textbox());
            overlay.set_show_text(main.get_overlay_visible());
        }
        {
            main.set_last_translated_text(display_text.into());
            if let Some(textbox) = tw.upgrade() {
                textbox.set_text(text.clone().into());
                textbox.set_text_color(main.get_overlay_text_color());
                textbox.set_font_size(main.get_base_font_size());
            }
        }
        if success && !text.trim().is_empty() {
            if let Err(error) = capture::copy_text_to_clipboard(&text) {
                log::warn!("Continuous result clipboard failed: {error:?}");
            }
        }
        if one_shot {
            main.set_is_running(false);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn capture_and_api_failures_stop_at_five_and_new_sessions_can_retry() {
        let capture_error = anyhow::anyhow!("capture failed");
        let server_error = api::ApiFailure::test_failure(503);
        let rate_limit = api::ApiFailure::test_failure(429);
        for error in [&capture_error, &server_error, &rate_limit] {
            for failures in 1..MAX_CONSECUTIVE_FAILURES {
                assert!(continuous_retry_delay(error, failures).is_some());
            }
            assert!(continuous_retry_delay(error, MAX_CONSECUTIVE_FAILURES).is_none());
            assert!(continuous_retry_delay(error, u32::MAX).is_none());
            assert!(continuous_retry_delay(error, 1).is_some());
        }
        assert!(continuous_retry_delay(&api::ApiFailure::test_failure(401), 1).is_none());
    }

    #[tokio::test]
    async fn stop_cancels_inflight_request_and_rejects_old_generations() {
        let control = Arc::new(crate::state::WorkerControl::default());
        let revision = control.revision();
        let mut cancel = control.subscribe();
        let stop = control.clone();
        let request = async move {
            stop.signal(); // Stop while the request is pending.
            std::future::pending::<String>().await
        };
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            in_generation(&mut cancel, revision, request),
        )
        .await
        .unwrap();
        assert!(result.is_none());
        // Even an immediately ready response from before Stop cannot be applied.
        assert!(in_generation(&mut cancel, revision, async { "stale" })
            .await
            .is_none());
        assert_eq!(
            in_generation(&mut cancel, control.revision(), async { "fresh" }).await,
            Some("fresh")
        );
    }

    #[test]
    fn waits_for_text_stability_and_ignores_background_motion() {
        let a = RgbaImage::from_pixel(20, 20, Rgba([0, 0, 0, 255]));
        let b = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
        let mut gate = ChangeGate::default();
        assert_eq!(
            gate.observe(Some("he"), &a, false, true),
            GateDecision::Wait
        );
        assert_eq!(
            gate.observe(Some("hello"), &b, false, true),
            GateDecision::Wait
        );
        assert_eq!(
            gate.observe(Some("hello"), &a, false, true),
            GateDecision::Translate
        );
        gate.accept(Some("hello".into()), a.clone());
        assert_eq!(
            gate.observe(Some("hello"), &b, false, true),
            GateDecision::Wait
        );
        gate.retry();
        assert_eq!(
            gate.observe(Some("hello"), &b, false, true),
            GateDecision::Translate
        );
    }

    #[test]
    fn empty_ocr_uses_stable_pixels_and_one_shot_is_immediate() {
        let a = RgbaImage::from_pixel(20, 20, Rgba([0, 0, 0, 255]));
        let b = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
        let mut gate = ChangeGate::default();
        assert_eq!(gate.observe(Some(""), &a, false, false), GateDecision::Wait);
        assert_eq!(gate.observe(Some(""), &b, false, false), GateDecision::Wait);
        assert_eq!(
            gate.observe(Some(""), &b, false, false),
            GateDecision::Translate
        );
        gate.accept(Some("".into()), b.clone());
        assert_eq!(gate.observe(Some(""), &b, false, false), GateDecision::Wait);
        assert_eq!(
            gate.observe(Some(""), &b, true, true),
            GateDecision::Translate
        );
    }

    #[test]
    fn subtitle_disappearance_clears_and_rearms_while_ocr_errors_still_use_pixels() {
        let image = RgbaImage::from_pixel(20, 20, Rgba([0, 0, 0, 255]));
        let mut gate = ChangeGate::default();
        gate.accept(Some("hello".into()), image.clone());
        assert_eq!(
            gate.observe(Some(""), &image, false, true),
            GateDecision::Clear
        );
        assert_eq!(
            gate.observe(Some(""), &image, false, true),
            GateDecision::Wait
        );
        assert_eq!(
            gate.observe(Some("hello"), &image, false, true),
            GateDecision::Wait
        );
        assert_eq!(
            gate.observe(Some("hello"), &image, false, true),
            GateDecision::Translate
        );
        let mut gate = ChangeGate::default();
        assert_eq!(gate.observe(None, &image, false, true), GateDecision::Wait);
        assert_eq!(
            gate.observe(None, &image, false, true),
            GateDecision::Translate
        );
    }
}
