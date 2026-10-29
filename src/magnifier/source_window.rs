//! Keep the source on a monitor behind the viewer, preserving its original placement.
//! Fully off-screen windows can stop presenting frames even while they have focus.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    os::windows::process::CommandExt,
    process::{Child, Command},
    sync::Mutex,
};
use windows::{
    core::w,
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, POINT, RECT},
        Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        System::Threading::{OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE},
        UI::{
            HiDpi::{
                SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            WindowsAndMessaging::*,
        },
    },
};

const WATCHDOG_ARG: &str = "--magnifier-restore-watchdog";
static PARKED: Mutex<Vec<Snapshot>> = Mutex::new(Vec::new());

struct PlacementDpi(DPI_AWARENESS_CONTEXT);
impl PlacementDpi {
    fn enter() -> Self {
        unsafe {
            Self(SetThreadDpiAwarenessContext(
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            ))
        }
    }
}
impl Drop for PlacementDpi {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            unsafe {
                SetThreadDpiAwarenessContext(self.0);
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Snapshot {
    hwnd: isize,
    pid: u32,
    thread: u32,
    owner: u32,
    flags: u32,
    show: u32,
    min: [i32; 2],
    max: [i32; 2],
    normal: [i32; 4],
}

impl Snapshot {
    fn owns_window(&self) -> bool {
        unsafe {
            let window = HWND(self.hwnd as _);
            let mut pid = 0;
            let thread = GetWindowThreadProcessId(window, Some(&mut pid));
            pid == self.pid
                && thread == self.thread
                && GetPropW(window, w!("OCRTrans.Magnifier.ParkedBy")).0 as usize
                    == self.owner as usize
        }
    }

    fn restore(&self) -> bool {
        if !self.owns_window() {
            return true;
        }
        let _dpi = PlacementDpi::enter();
        let window = HWND(self.hwnd as _);
        let placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            flags: WINDOWPLACEMENT_FLAGS(self.flags) | WPF_ASYNCWINDOWPLACEMENT,
            showCmd: self.show,
            ptMinPosition: POINT {
                x: self.min[0],
                y: self.min[1],
            },
            ptMaxPosition: POINT {
                x: self.max[0],
                y: self.max[1],
            },
            rcNormalPosition: RECT {
                left: self.normal[0],
                top: self.normal[1],
                right: self.normal[2],
                bottom: self.normal[3],
            },
        };
        unsafe {
            match SetWindowPlacement(window, &placement) {
                Ok(()) => {
                    let _ = RemovePropW(window, w!("OCRTrans.Magnifier.ParkedBy"));
                    true
                }
                Err(error) => {
                    log::warn!("Unable to restore magnified source window: {error}");
                    false
                }
            }
        }
    }
}

pub(super) struct SourceWindow {
    snapshot: Snapshot,
    watchdog: Option<Child>,
    pub window: HWND,
    viewer: HWND,
    covered: bool,
    last_park: std::time::Instant,
    restart_capture_pending: bool,
    opened_at: std::time::Instant,
}

impl SourceWindow {
    pub fn park(window: HWND, viewer: HWND) -> Result<Self> {
        let _dpi = PlacementDpi::enter();
        unsafe {
            anyhow::ensure!(
                GetPropW(window, w!("OCRTrans.Magnifier.ParkedBy")).is_invalid(),
                "This window is already magnified"
            );
            let mut placement = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            GetWindowPlacement(window, &mut placement)?;
            let mut rect = RECT::default();
            GetWindowRect(window, &mut rect)?;
            let mut pid = 0;
            let thread = GetWindowThreadProcessId(window, Some(&mut pid));
            anyhow::ensure!(
                pid != 0 && pid != std::process::id(),
                "Invalid external source window"
            );
            let owner = std::process::id();
            SetPropW(
                window,
                w!("OCRTrans.Magnifier.ParkedBy"),
                Some(HANDLE(owner as usize as _)),
            )?;
            let snapshot = Snapshot {
                hwnd: window.0 as isize,
                pid,
                thread,
                owner,
                flags: placement.flags.0,
                show: placement.showCmd,
                min: [placement.ptMinPosition.x, placement.ptMinPosition.y],
                max: [placement.ptMaxPosition.x, placement.ptMaxPosition.y],
                normal: [
                    placement.rcNormalPosition.left,
                    placement.rcNormalPosition.top,
                    placement.rcNormalPosition.right,
                    placement.rcNormalPosition.bottom,
                ],
            };
            PARKED.lock().unwrap().push(snapshot.clone());
            let mut this = Self {
                snapshot,
                watchdog: None,
                window,
                viewer,
                covered: false,
                last_park: std::time::Instant::now(),
                restart_capture_pending: true,
                opened_at: std::time::Instant::now(),
            };
            // The helper does not open a UI and restores this exact HWND/PID only
            // if our ownership marker survives an unexpected parent-process exit.
            this.watchdog = Some(
                Command::new(std::env::current_exe()?)
                    .arg(WATCHDOG_ARG)
                    .arg(serde_json::to_string(&this.snapshot)?)
                    .creation_flags(0x08000000) // CREATE_NO_WINDOW
                    .spawn()
                    .context("Unable to start source-window recovery helper")?,
            );
            if IsZoomed(window).as_bool() {
                let _ = ShowWindowAsync(window, SW_RESTORE);
            }
            let mut viewer_rect = RECT::default();
            GetWindowRect(viewer, &mut viewer_rect)?;
            let work = monitor_work_area(viewer)?;
            let (x, y, covered) = source_position(rect, viewer_rect, work);
            this.covered = covered;
            SetWindowPos(
                window,
                None,
                x,
                y,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS,
            )?;
            Ok(this)
        }
    }

    pub fn valid(&self) -> bool {
        self.snapshot.owns_window()
    }

    pub fn covered(&self) -> bool {
        self.covered
    }

    pub fn take_capture_restart(&mut self) -> bool {
        let _dpi = PlacementDpi::enter();
        if !self.restart_capture_pending
            || self.opened_at.elapsed() < std::time::Duration::from_millis(300)
        {
            return false;
        }
        let mut rect = RECT::default();
        let Ok(work) = monitor_work_area(self.viewer) else {
            return false;
        };
        if unsafe { GetWindowRect(self.window, &mut rect) }.is_err()
            || rect.right <= work.left
            || rect.left >= work.right
            || rect.bottom <= work.top
            || rect.top >= work.bottom
        {
            return false;
        }
        self.restart_capture_pending = false;
        true
    }

    pub fn keep_parked(&mut self) {
        let _dpi = PlacementDpi::enter();
        if self.last_park.elapsed() < std::time::Duration::from_millis(250) || !self.valid() {
            return;
        }
        self.last_park = std::time::Instant::now();
        unsafe {
            if IsIconic(self.window).as_bool() {
                return;
            }
            let mut rect = RECT::default();
            if GetWindowRect(self.window, &mut rect).is_err() {
                return;
            }
            let mut viewer_rect = RECT::default();
            if GetWindowRect(self.viewer, &mut viewer_rect).is_err() {
                return;
            }
            let Ok(work) = monitor_work_area(self.viewer) else {
                return;
            };
            let (x, y, covered) = source_position(rect, viewer_rect, work);
            self.covered = covered;
            if rect.left != x || rect.top != y {
                let _ = SetWindowPos(
                    self.window,
                    None,
                    x,
                    y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS,
                );
            }
        }
    }
}

fn monitor_work_area(window: HWND) -> Result<RECT> {
    let _dpi = PlacementDpi::enter();
    unsafe {
        let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        anyhow::ensure!(
            GetMonitorInfoW(monitor, &mut info).as_bool(),
            "Unable to read monitor work area"
        );
        Ok(info.rcWork)
    }
}

/// Center the source behind the viewer, but keep it inside an actual monitor so
/// capture and foreground input continue. Larger sources remain partly visible.
fn source_position(source: RECT, viewer: RECT, work: RECT) -> (i32, i32, bool) {
    let width = (source.right - source.left).max(1);
    let height = (source.bottom - source.top).max(1);
    let desired_x = viewer
        .left
        .saturating_add((viewer.right - viewer.left - width) / 2);
    let desired_y = viewer
        .top
        .saturating_add((viewer.bottom - viewer.top - height) / 2);
    let x = desired_x.clamp(work.left, (work.right - width).max(work.left));
    let y = desired_y.clamp(work.top, (work.bottom - height).max(work.top));
    let covered = x >= viewer.left
        && y >= viewer.top
        && x.saturating_add(width) <= viewer.right
        && y.saturating_add(height) <= viewer.bottom;
    (x, y, covered)
}

impl Drop for SourceWindow {
    fn drop(&mut self) {
        if self.snapshot.restore() {
            if let Some(mut watchdog) = self.watchdog.take() {
                let _ = watchdog.kill();
                let _ = watchdog.wait();
            }
            PARKED.lock().unwrap().retain(|saved| {
                saved.hwnd != self.snapshot.hwnd || saved.owner != self.snapshot.owner
            });
        }
        // On a transient restore error, keep the registry entry and leave the
        // helper waiting so parent shutdown can retry restoration.
    }
}

pub(crate) fn restore_all() {
    let parked = std::mem::take(&mut *PARKED.lock().unwrap());
    for snapshot in parked {
        snapshot.restore();
    }
}

pub(crate) fn run_restore_watchdog() -> Result<bool> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(WATCHDOG_ARG) {
        return Ok(false);
    }
    let snapshot: Snapshot =
        serde_json::from_str(&args.next().context("Missing recovery snapshot")?)?;
    unsafe {
        if let Ok(parent) = OpenProcess(PROCESS_SYNCHRONIZE, false, snapshot.owner) {
            WaitForSingleObject(parent, INFINITE);
            let _ = CloseHandle(parent);
        }
    }
    for _ in 0..20 {
        if snapshot.restore() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_stays_on_monitor_and_is_covered_when_it_fits() {
        let work = RECT {
            left: -1920,
            top: 40,
            right: 0,
            bottom: 1080,
        };
        let viewer = RECT {
            left: -1600,
            top: 100,
            right: -400,
            bottom: 1000,
        };
        let source = RECT {
            left: 0,
            top: 0,
            right: 800,
            bottom: 600,
        };
        assert_eq!(source_position(source, viewer, work), (-1400, 250, true));
        let large = RECT {
            left: 0,
            top: 0,
            right: 1800,
            bottom: 1000,
        };
        let (x, y, covered) = source_position(large, viewer, work);
        assert!(!covered);
        assert!(
            x >= work.left && y >= work.top && x + 1800 <= work.right && y + 1000 <= work.bottom
        );
    }
    #[test]
    fn restoration_snapshot_roundtrips_maximized_and_negative_monitor_geometry() {
        let saved = Snapshot {
            hwnd: 123,
            pid: 9,
            thread: 7,
            owner: 11,
            flags: 2,
            show: 3,
            min: [-1, -1],
            max: [-1920, 0],
            normal: [-1800, 50, -600, 800],
        };
        let restored: Snapshot =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(restored.normal, saved.normal);
        assert_eq!(
            (
                restored.hwnd,
                restored.pid,
                restored.thread,
                restored.show,
                restored.flags
            ),
            (123, 9, 7, 3, 2)
        );
    }
}
