//! Default playback-device loopback, PCM transport, and audio timeline recovery.
use super::audio_format::{parse_wasapi_audio_format, silent_audio_bytes, WasapiAudioFormat};
use anyhow::{Context, Result};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
fn probe_wasapi_loopback_format() -> Result<WasapiAudioFormat> {
    // COM apartment state belongs to the calling thread. Probe on a short-lived MTA so this
    // remains safe even when Slint/Winit has initialized the UI thread differently.
    thread::spawn(probe_wasapi_loopback_format_on_mta)
        .join()
        .map_err(|_| anyhow::anyhow!("WASAPI format probe thread panicked"))?
}

#[cfg(target_os = "windows")]
fn probe_wasapi_loopback_format_on_mta() -> Result<WasapiAudioFormat> {
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };

    let init_result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if init_result.is_err() {
        anyhow::bail!("Failed to initialize COM for WASAPI: {init_result:?}");
    }

    let result = (|| {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .context("Failed to create the Windows audio device enumerator")?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
            .context("Failed to find the default Windows playback device")?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .context("Failed to activate the default Windows playback device")?;
        let format_ptr = unsafe { client.GetMixFormat() }
            .context("Failed to read the Windows playback mix format")?;
        let result = parse_wasapi_audio_format(format_ptr);
        unsafe {
            CoTaskMemFree(Some(format_ptr as *const std::ffi::c_void));
        }
        result
    })();

    unsafe { CoUninitialize() };
    result
}

#[cfg(target_os = "windows")]
fn accept_audio_stream(listener: TcpListener, stop: &AtomicBool) -> Result<Option<TcpStream>> {
    listener
        .set_nonblocking(true)
        .context("Failed to configure the Windows audio loopback pipe")?;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match listener.accept() {
            Ok((stream, _)) => {
                // accept()한 소켓은 리스너의 non-blocking 모드를 상속받는다.
                // PCM 실시간 전송은 WouldBlock 즉시 실패가 아니라 블로킹
                // 백프레셔로 동작해야 한다. 그렇지 않으면 FFmpeg이 느리게
                // 읽는 시작 구간에 TCP 버퍼가 차자마자 오디오 연결 전체가
                // 끊기고 무음/짧은 오디오 트랙으로 남는다.
                stream
                    .set_nonblocking(false)
                    .context("Failed to configure the Windows audio loopback stream")?;
                stream
                    .set_write_timeout(Some(Duration::from_millis(2000)))
                    .context("Failed to configure the Windows audio loopback stream")?;
                let _ = stream.set_nodelay(true);
                return Ok(Some(stream));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error).context("Failed to accept the Windows audio stream"),
        }
    }
}

#[cfg(target_os = "windows")]
fn write_audio_bytes(stream: &mut TcpStream, bytes: &[u8], stop: &AtomicBool) -> bool {
    use std::io::Write;
    let mut written = 0;
    while written < bytes.len() {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        match stream.write(&bytes[written..]) {
            Ok(0) => {
                thread::sleep(Duration::from_millis(1));
            }
            Ok(n) => written += n,
            Err(error)
                if error.kind() == std::io::ErrorKind::Interrupted
                    || error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                // FFmpeg이 비디오 인코딩에 밀려 오디오 소비가 늦어지면
                // write_timeout(2000ms)이 만료될 수 있다. 이때 오디오
                // 스트림을 끊으면(EOF) 이후 구간이 무음/짧은 트랙으로
                // 남는다. 잠시 쉬고 재시도해서 PCM을 계속 공급한다.
                thread::sleep(Duration::from_millis(1));
            }
            Err(_) => return false,
        }
    }
    true
}

#[cfg(target_os = "windows")]
fn write_paced_silence_packet(
    stream: &mut TcpStream,
    packet: &[u8],
    packet_duration: Duration,
    next_packet: &mut Instant,
    stop: &AtomicBool,
) -> bool {
    let now = Instant::now();
    if now >= *next_packet {
        if !write_audio_bytes(stream, packet, stop) {
            return false;
        }
        *next_packet = now + packet_duration;
    } else {
        thread::sleep((*next_packet - now).min(Duration::from_millis(5)));
    }
    true
}

#[cfg(target_os = "windows")]
fn feed_silence_until_stop(mut stream: TcpStream, format: WasapiAudioFormat, stop: &AtomicBool) {
    let silent_frames = (format.sample_rate as usize / 100).max(1);
    let silent_packet = silent_audio_bytes(silent_frames, format);
    let silent_duration = Duration::from_secs_f64(silent_frames as f64 / format.sample_rate as f64);
    let mut next_silent_packet = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if !write_paced_silence_packet(
            &mut stream,
            &silent_packet,
            silent_duration,
            &mut next_silent_packet,
            stop,
        ) {
            break;
        }
    }
}

#[cfg(target_os = "windows")]
fn write_silence_frames(
    stream: &mut TcpStream,
    mut frames: u64,
    format: WasapiAudioFormat,
    stop: &AtomicBool,
) -> bool {
    let max_chunk_frames = (format.sample_rate as u64 / 100).max(1);
    while frames > 0 {
        let chunk_frames = frames.min(max_chunk_frames) as usize;
        let packet = silent_audio_bytes(chunk_frames, format);
        if !write_audio_bytes(stream, &packet, stop) {
            return false;
        }
        frames -= chunk_frames as u64;
    }
    true
}

#[cfg(target_os = "windows")]
pub(super) fn capture_wasapi_loopback(
    listener: TcpListener,
    expected_format: WasapiAudioFormat,
    stop: Arc<AtomicBool>,
    error_slot: Arc<Mutex<Option<String>>>,
) {
    let stream = match accept_audio_stream(listener, &stop) {
        Ok(Some(stream)) => stream,
        Ok(None) => return,
        Err(error) => {
            log::warn!("Windows audio loopback could not accept FFmpeg: {error:?}");
            if let Ok(mut error_slot) = error_slot.lock() {
                *error_slot = Some(format!("{error:#}"));
            }
            // Do not set the shared stop flag here. A temporary audio setup failure must not
            // turn into a few-frame video recording; the video worker owns the recording stop.
            return;
        }
    };
    let result = capture_wasapi_loopback_on_mta(stream, expected_format, &stop);
    if let Err(error) = result {
        log::warn!("Windows audio loopback stopped: {error:?}");
        if let Ok(mut error_slot) = error_slot.lock() {
            *error_slot = Some(format!("{error:#}"));
        }
    }
}

#[cfg(target_os = "windows")]
fn capture_wasapi_loopback_on_mta(
    mut stream: TcpStream,
    expected_format: WasapiAudioFormat,
    stop: &AtomicBool,
) -> Result<()> {
    use windows::Win32::Media::Audio::{
        eMultimedia, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
        AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };

    let init_result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if init_result.is_err() {
        let error =
            anyhow::anyhow!("Failed to initialize COM for Windows audio capture: {init_result:?}");
        feed_silence_until_stop(stream, expected_format, stop);
        return Err(error);
    }

    let result = (|| {
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .context("Failed to create the Windows audio device enumerator")?;
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
            .context("Failed to open the default Windows playback device")?;
        let client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None) }
            .context("Failed to activate Windows audio loopback")?;
        let format_ptr = unsafe { client.GetMixFormat() }
            .context("Failed to read the Windows playback mix format")?;
        let actual_format = parse_wasapi_audio_format(format_ptr);
        let actual_format = match actual_format {
            Ok(format) => format,
            Err(error) => {
                unsafe {
                    CoTaskMemFree(Some(format_ptr as *const std::ffi::c_void));
                }
                return Err(error);
            }
        };
        if actual_format != expected_format {
            unsafe {
                CoTaskMemFree(Some(format_ptr as *const std::ffi::c_void));
            }
            anyhow::bail!("The default Windows playback format changed while recording started");
        }

        let initialize_result = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                10_000_000,
                0,
                format_ptr,
                None,
            )
        };
        unsafe {
            CoTaskMemFree(Some(format_ptr as *const std::ffi::c_void));
        }
        initialize_result.context("Failed to initialize Windows audio loopback")?;

        let capture_client: IAudioCaptureClient = unsafe { client.GetService() }
            .context("Failed to access the Windows audio capture buffer")?;
        unsafe { client.Start() }.context("Failed to start Windows audio loopback")?;

        let capture_result = (|| {
            let mut consecutive_read_errors = 0u32;
            let capture_started = Instant::now();
            let silence_guard_frames = (expected_format.sample_rate as u64 / 50).max(1);
            let silence_chunk_frames = (expected_format.sample_rate as u64 / 100).max(1);
            let mut written_frames = 0u64;
            let mut device_to_output_offset: Option<i128> = None;
            let mut last_device_packet_at: Option<Instant> = None;
            while !stop.load(Ordering::Relaxed) {
                let packet_frames = match unsafe { capture_client.GetNextPacketSize() } {
                    Ok(packet_frames) => {
                        consecutive_read_errors = 0;
                        packet_frames
                    }
                    Err(error) => {
                        consecutive_read_errors = consecutive_read_errors.saturating_add(1);
                        if consecutive_read_errors <= 3 || consecutive_read_errors.is_power_of_two()
                        {
                            log::warn!(
                                "Windows audio packet read failed (attempt {consecutive_read_errors}): {error:?}"
                            );
                        }
                        // Do not manufacture a packet here. The next WASAPI packet already
                        // covers the device timeline; inserting silence before it duplicates
                        // samples and is heard as crackle/static.
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                };
                if packet_frames == 0 {
                    if last_device_packet_at
                        .is_some_and(|received| received.elapsed() < Duration::from_millis(100))
                    {
                        // Device packets can arrive less frequently than their contained frame
                        // duration. Never insert wall-clock silence between active packets.
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    // Keep a continuous audio track when Windows suppresses loopback packets
                    // for an extended silent period, but stay 20 ms behind the wall clock. If a
                    // real packet resumes for that interval, device-position alignment below
                    // skips the overlap rather than appending duplicate samples.
                    let elapsed_frames = (capture_started.elapsed().as_secs_f64()
                        * expected_format.sample_rate as f64)
                        as u64;
                    let target_frames = elapsed_frames.saturating_sub(silence_guard_frames);
                    let frames_to_fill = target_frames
                        .saturating_sub(written_frames)
                        .min(silence_chunk_frames);
                    if frames_to_fill > 0 {
                        if !write_silence_frames(&mut stream, frames_to_fill, expected_format, stop)
                        {
                            break;
                        }
                        written_frames += frames_to_fill;
                    } else {
                        thread::sleep(Duration::from_millis(1));
                    }
                    continue;
                }

                let mut data_ptr: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut device_position = 0u64;
                let buffer_result = unsafe {
                    capture_client.GetBuffer(
                        &mut data_ptr,
                        &mut frames,
                        &mut flags,
                        Some(&mut device_position),
                        None,
                    )
                }
                .context("Failed to read the Windows audio capture buffer");
                if let Err(error) = buffer_result {
                    consecutive_read_errors = consecutive_read_errors.saturating_add(1);
                    if consecutive_read_errors <= 3 || consecutive_read_errors.is_power_of_two() {
                        log::warn!(
                            "Windows audio buffer read failed (attempt {consecutive_read_errors}): {error:?}"
                        );
                    }
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                last_device_packet_at = Some(Instant::now());

                let byte_len = frames as usize * expected_format.block_align;
                let is_silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let bytes = if is_silent || data_ptr.is_null() {
                    // AUDCLNT_BUFFERFLAGS_SILENT: the loopback engine still advances the
                    // timeline, so write an equally sized silent packet instead of dropping it.
                    silent_audio_bytes(frames as usize, expected_format)
                } else {
                    unsafe { std::slice::from_raw_parts(data_ptr as *const u8, byte_len) }.to_vec()
                };
                if let Err(error) = unsafe { capture_client.ReleaseBuffer(frames) }
                    .context("Failed to release the Windows audio capture buffer")
                {
                    consecutive_read_errors = consecutive_read_errors.saturating_add(1);
                    if consecutive_read_errors <= 3 || consecutive_read_errors.is_power_of_two() {
                        log::warn!(
                            "Windows audio buffer release failed (attempt {consecutive_read_errors}): {error:?}"
                        );
                    }
                    return Err(error);
                }

                let discontinuity = flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0;
                let timestamp_error = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0;
                if device_to_output_offset.is_none() || discontinuity || timestamp_error {
                    // Anchor the device frame counter to the already-written output timeline.
                    // Re-anchor after a device reset instead of treating its new position as a
                    // giant gap or overlap.
                    device_to_output_offset =
                        Some(written_frames as i128 - device_position as i128);
                    if discontinuity || timestamp_error {
                        log::warn!("Windows audio loopback timeline was re-synchronized");
                    }
                }

                let mapped_packet_start = (device_position as i128
                    + device_to_output_offset.unwrap_or_default())
                .max(0) as u64;
                if mapped_packet_start > written_frames {
                    let gap_frames = mapped_packet_start - written_frames;
                    if !write_silence_frames(&mut stream, gap_frames, expected_format, stop) {
                        break;
                    }
                    written_frames += gap_frames;
                }

                let overlap_frames = written_frames.saturating_sub(mapped_packet_start);
                if overlap_frames >= frames as u64 {
                    continue;
                }
                let frames_to_write = frames as u64 - overlap_frames;
                let byte_offset = overlap_frames as usize * expected_format.block_align;
                if !write_audio_bytes(&mut stream, &bytes[byte_offset..], stop) {
                    break;
                }
                written_frames += frames_to_write;
            }
            Ok::<(), anyhow::Error>(())
        })();

        let stop_result = unsafe { client.Stop() };
        stop_result.context("Failed to stop Windows audio loopback")?;
        capture_result
    })();

    unsafe { CoUninitialize() };
    if let Err(error) = result {
        log::warn!("Windows audio setup failed; continuing with silence: {error:?}");
        feed_silence_until_stop(stream, expected_format, stop);
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
pub(super) fn prepare_wasapi_input() -> Result<(WasapiAudioFormat, TcpListener, String)> {
    let format = probe_wasapi_loopback_format()?;
    log::info!("Windows playback loopback format: {format:?}");
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .context("Failed to create the Windows audio loopback stream")?;
    let url = format!(
        "tcp://127.0.0.1:{}",
        listener
            .local_addr()
            .context("Failed to get the Windows audio loopback address")?
            .port()
    );
    Ok((format, listener, url))
}
