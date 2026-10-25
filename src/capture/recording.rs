//! FFmpeg process startup, fallback, pause/stop state, and worker lifecycle.
#[cfg(target_os = "windows")]
mod audio;
#[cfg(target_os = "windows")]
mod audio_format;
mod video;

#[cfg(target_os = "windows")]
use self::audio::{capture_wasapi_loopback, prepare_wasapi_input};
#[cfg(target_os = "windows")]
use self::video::ddagrab_config;
use self::video::record_frames;
use super::{capture_area, CaptureRect};
use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

fn ffmpeg_executable(executable: Option<&std::path::Path>) -> std::path::PathBuf {
    let name = if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    executable
        .and_then(std::path::Path::parent)
        .map(|directory| directory.join(name))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| "ffmpeg".into())
}

/// Lightweight FFmpeg-backed recorder. Windows uses Desktop Duplication with NVENC for normal
/// single-monitor selections; the raw-frame worker remains as a cross-monitor/non-Windows
/// fallback. The UI owns the stop/pause handles and recording lifecycle.
pub struct ScreenRecorder {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// FFmpeg reads video frames from stdin for the GDI path. The ddagrab path captures frames
    /// itself, so stdin is retained here and receives `q` for a graceful MP4 finalization.
    control_stdin: Option<ChildStdin>,
    audio_worker: Option<JoinHandle<()>>,
    audio_error: Arc<Mutex<Option<String>>>,
    ffmpeg_stderr: Option<JoinHandle<String>>,
    child: Option<Arc<Mutex<Child>>>,
}

impl ScreenRecorder {
    pub fn start(rect: CaptureRect, path: PathBuf, fps: u32) -> Result<Self> {
        match Self::start_with_backend(rect, path.clone(), fps, true) {
            Ok(recorder) => Ok(recorder),
            Err(error) => {
                log::warn!("Hardware recording unavailable, retrying with software: {error:?}");
                Self::start_with_backend(rect, path, fps, false).with_context(|| {
                    format!("Software recording failed after initial attempt: {error}")
                })
            }
        }
    }

    fn start_with_backend(
        rect: CaptureRect,
        path: PathBuf,
        fps: u32,
        prefer_hardware: bool,
    ) -> Result<Self> {
        if !rect.valid() {
            anyhow::bail!("The recording area is too small");
        }
        let width = rect.width & !1;
        let height = rect.height & !1;
        if width < 2 || height < 2 {
            anyhow::bail!("The recording area must be at least 2x2");
        }
        let rect = CaptureRect {
            width,
            height,
            ..rect
        };
        let fps = fps.clamp(1, 60);

        #[cfg(target_os = "windows")]
        let ddagrab = if prefer_hardware {
            ddagrab_config(rect, fps)?
        } else {
            None
        };
        #[cfg(target_os = "windows")]
        let use_ddagrab = ddagrab.is_some();
        #[cfg(not(target_os = "windows"))]
        let use_ddagrab = false;

        // Validate the software capture path before reporting "Recording started". Previously
        // all capture errors happened on a detached worker, leaving the toolbar in a recording
        // state even though FFmpeg had received no usable video.
        let first_frame = if use_ddagrab {
            None
        } else {
            let image = capture_area(&rect, &None).context("Failed to capture the first frame")?;
            if image.width() != width as u32 || image.height() != height as u32 {
                anyhow::bail!(
                    "The recording area crosses a monitor boundary (captured {}x{}, expected {width}x{height})",
                    image.width(),
                    image.height()
                );
            }
            Some(image.into_raw())
        };

        let video_size = format!("{}x{}", width, height);
        #[cfg(target_os = "windows")]
        let audio_input = match prepare_wasapi_input() {
            Ok(input) => Some(input),
            Err(error) => {
                // Audio-device setup must not prevent screen recording. This is common on PCs
                // with no active playback endpoint and was the main all-or-nothing failure path.
                log::warn!(
                    "Windows playback audio is unavailable; recording video only: {error:?}"
                );
                None
            }
        };
        let framerate = fps.to_string();
        let mut ffmpeg = Command::new(ffmpeg_executable(std::env::current_exe().ok().as_deref()));
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;

            // CREATE_NO_WINDOW prevents the console-hosted ffmpeg process from opening a
            // terminal window while recording from the toolbar.
            ffmpeg.creation_flags(0x0800_0000);
        }
        ffmpeg.args(["-y", "-loglevel", "error", "-fflags", "+genpts"]);
        #[cfg(target_os = "windows")]
        if let Some(config) = ddagrab.as_ref() {
            ffmpeg.args(["-f", "lavfi", "-i", config.input_filter.as_str()]);
            log::info!(
                "Starting ddagrab/NVENC recording at {}x{} and {fps} fps",
                config.output_width,
                config.output_height
            );
        } else {
            ffmpeg.args([
                "-probesize",
                "32k",
                "-analyzeduration",
                "0",
                "-thread_queue_size",
                "1024",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgba",
                "-video_size",
                video_size.as_str(),
                "-framerate",
                framerate.as_str(),
                "-i",
                "-",
            ]);
        }
        #[cfg(not(target_os = "windows"))]
        ffmpeg.args([
            "-probesize",
            "32k",
            "-analyzeduration",
            "0",
            "-thread_queue_size",
            "1024",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            video_size.as_str(),
            "-framerate",
            framerate.as_str(),
            "-i",
            "-",
        ]);
        #[cfg(target_os = "windows")]
        if let Some((audio_format, _, audio_url)) = audio_input.as_ref() {
            // FFmpeg builds without a WASAPI demuxer are still common on Windows. Feed it the
            // native default-render loopback through localhost, so the recording contains
            // Windows playback audio instead of whichever webcam microphone DirectShow lists.
            let audio_sample_rate = audio_format.sample_rate.to_string();
            let audio_channels = audio_format.channels.to_string();
            ffmpeg.args([
                "-thread_queue_size",
                "1024",
                "-f",
                audio_format.raw_format.ffmpeg_name(),
                "-ar",
                audio_sample_rate.as_str(),
                "-ac",
                audio_channels.as_str(),
                "-i",
            ]);
            ffmpeg.arg(audio_url);
        }

        ffmpeg.args(["-map", "0:v:0"]);
        #[cfg(target_os = "windows")]
        if audio_input.is_some() {
            ffmpeg.args(["-map", "1:a:0"]);
        } else {
            ffmpeg.arg("-an");
        }

        #[cfg(target_os = "windows")]
        if use_ddagrab {
            ffmpeg.args([
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p4",
                "-tune",
                "hq",
                "-rc",
                "vbr",
                "-cq",
                "21",
                "-b:v",
                "0",
            ]);
        } else {
            ffmpeg.args(["-c:v", "libx264", "-preset", "ultrafast"]);
        }
        #[cfg(not(target_os = "windows"))]
        ffmpeg.args(["-an", "-c:v", "libx264", "-preset", "ultrafast"]);

        ffmpeg.args(["-fps_mode", "cfr", "-r", framerate.as_str()]);
        #[cfg(target_os = "windows")]
        if audio_input.is_some() {
            ffmpeg.args([
                "-af",
                "aresample=async=1:min_hard_comp=0.100:first_pts=0",
                "-ar",
                "48000",
                "-ac",
                "2",
                "-channel_layout",
                "stereo",
                "-c:a",
                "aac",
                "-profile:a",
                "aac_low",
                "-b:a",
                "192k",
            ]);
        }
        ffmpeg.args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"]);

        let mut child = ffmpeg
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context(
                "Could not start FFmpeg. Place ffmpeg.exe beside the app or install it on PATH.",
            )?;
        let mut ffmpeg_stderr = child.stderr.take().map(|mut stderr| {
            thread::spawn(move || {
                let mut output = Vec::new();
                let _ = stderr.read_to_end(&mut output);
                String::from_utf8_lossy(&output).trim().to_string()
            })
        });
        let mut stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Failed to open FFmpeg input");
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let audio_error = Arc::new(Mutex::new(None));

        #[cfg(target_os = "windows")]
        let mut audio_worker = audio_input.map(|(audio_format, audio_listener, _)| {
            let audio_stop = stop.clone();
            let audio_error = audio_error.clone();
            thread::spawn(move || {
                capture_wasapi_loopback(audio_listener, audio_format, audio_stop, audio_error)
            })
        });
        #[cfg(not(target_os = "windows"))]
        let mut audio_worker = None;

        let (mut worker, mut control_stdin) = if use_ddagrab {
            (None, Some(stdin))
        } else {
            let initial_frame = first_frame.expect("raw capture always has a validated frame");
            if let Err(error) = stdin.write_all(&initial_frame) {
                stop.store(true, Ordering::Relaxed);
                let _ = child.kill();
                if let Some(audio_worker) = audio_worker.take() {
                    let _ = audio_worker.join();
                }
                let _ = child.wait();
                anyhow::bail!("FFmpeg closed before recording started: {error}");
            }
            let worker_stop = stop.clone();
            let worker_paused = paused.clone();
            let interval = Duration::from_secs_f64(1.0 / fps as f64);
            let worker = thread::spawn(move || {
                record_frames(
                    &mut stdin,
                    rect,
                    interval,
                    worker_stop,
                    worker_paused,
                    Some(initial_frame),
                )
            });
            (Some(worker), None)
        };

        // FFmpeg used to fail asynchronously (missing encoder, bad input, etc.) while start()
        // still returned success. Give it enough time to initialize and surface the real error.
        thread::sleep(Duration::from_millis(350));
        if let Some(status) = child.try_wait().context("Failed to check FFmpeg startup")? {
            stop.store(true, Ordering::Relaxed);
            control_stdin.take();
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
            if let Some(audio_worker) = audio_worker.take() {
                let _ = audio_worker.join();
            }
            let diagnostic = ffmpeg_stderr
                .take()
                .and_then(|reader| reader.join().ok())
                .filter(|message| !message.is_empty())
                .unwrap_or_else(|| format!("FFmpeg exited with {status}"));
            anyhow::bail!("FFmpeg could not start recording: {diagnostic}");
        }

        let child = Arc::new(Mutex::new(child));

        Ok(Self {
            stop,
            paused,
            worker,
            control_stdin,
            audio_worker,
            audio_error,
            ffmpeg_stderr,
            child: Some(child),
        })
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);

        // ddagrab has no raw-video stdin to close. Ask FFmpeg to stop through its normal console
        // control path so it writes the MP4 trailer instead of leaving an unplayable file.
        if let Some(mut stdin) = self.control_stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }

        // A blocked pipe write can otherwise keep a worker alive forever if FFmpeg exited early
        // (for example after an audio-device change). Give the normal graceful close a short
        // window, then terminate only the recorder child as a last resort so closing the app
        // cannot leave an orphaned FFmpeg process behind.
        let child = self.child.take();
        let stop_watchdog = Arc::new(AtomicBool::new(false));
        let watchdog = child.as_ref().map(|child| {
            let child = child.clone();
            let stop_watchdog = stop_watchdog.clone();
            thread::spawn(move || {
                for _ in 0..20 {
                    if stop_watchdog.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                if !stop_watchdog.load(Ordering::Relaxed) {
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                }
            })
        });
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(audio_worker) = self.audio_worker.take() {
            let _ = audio_worker.join();
        }
        stop_watchdog.store(true, Ordering::Relaxed);
        if let Some(watchdog) = watchdog {
            let _ = watchdog.join();
        }
        let audio_error = self
            .audio_error
            .lock()
            .ok()
            .and_then(|mut error| error.take());
        let mut ffmpeg_error = None;
        if let Some(child) = child {
            let status = child
                .lock()
                .map_err(|_| anyhow::anyhow!("FFmpeg process state was poisoned"))?
                .wait()
                .context("Failed to close FFmpeg")?;
            ffmpeg_error = self
                .ffmpeg_stderr
                .take()
                .and_then(|worker| worker.join().ok())
                .filter(|error| !error.is_empty());
            if !status.success() {
                if let Some(error) = ffmpeg_error {
                    anyhow::bail!("FFmpeg could not finish the recording: {error}");
                }
                anyhow::bail!("FFmpeg could not finish the recording");
            }
        }
        if let Some(error) = ffmpeg_error {
            log::warn!("FFmpeg reported a recorder diagnostic: {error}");
        }
        if let Some(error) = audio_error {
            // Preserve the completed video when the playback device changes or disappears.
            log::warn!("Windows playback audio could not be recorded completely: {error}");
        }
        Ok(())
    }
}

impl Drop for ScreenRecorder {
    fn drop(&mut self) {
        // Keep an unexpected drop from orphaning FFmpeg. The normal UI stop path still performs
        // the graceful flush in `stop`; this is only the emergency cleanup path.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(child) = self.child.as_ref() {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }
    }
}
