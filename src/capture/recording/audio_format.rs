//! WASAPI PCM/float format parsing and format-correct silent samples.
use anyhow::Result;

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WasapiRawAudioFormat {
    Unsigned8,
    Signed16Le,
    Signed24Le,
    Signed32Le,
    Float32Le,
}

#[cfg(target_os = "windows")]
impl WasapiRawAudioFormat {
    pub(super) fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::Unsigned8 => "u8",
            Self::Signed16Le => "s16le",
            Self::Signed24Le => "s24le",
            Self::Signed32Le => "s32le",
            Self::Float32Le => "f32le",
        }
    }

    fn bytes_per_sample(self) -> usize {
        match self {
            Self::Unsigned8 => 1,
            Self::Signed16Le => 2,
            Self::Signed24Le => 3,
            Self::Signed32Le | Self::Float32Le => 4,
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) fn silent_audio_bytes(frames: usize, format: WasapiAudioFormat) -> Vec<u8> {
    let silence_sample = match format.raw_format {
        // Windows PCM 8-bit samples are unsigned, so the zero-amplitude midpoint is 128.
        WasapiRawAudioFormat::Unsigned8 => 0x80,
        WasapiRawAudioFormat::Signed16Le
        | WasapiRawAudioFormat::Signed24Le
        | WasapiRawAudioFormat::Signed32Le
        | WasapiRawAudioFormat::Float32Le => 0,
    };
    vec![silence_sample; frames.saturating_mul(format.block_align)]
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WasapiAudioFormat {
    pub(super) sample_rate: u32,
    pub(super) channels: u16,
    pub(super) block_align: usize,
    pub(super) raw_format: WasapiRawAudioFormat,
}

#[cfg(target_os = "windows")]
pub(super) fn parse_wasapi_audio_format(
    format_ptr: *const windows::Win32::Media::Audio::WAVEFORMATEX,
) -> Result<WasapiAudioFormat> {
    use windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE;

    if format_ptr.is_null() {
        anyhow::bail!("Windows returned an empty WASAPI mix format");
    }

    let format = unsafe { std::ptr::read_unaligned(format_ptr) };
    let format_tag = format.wFormatTag;
    let bits_per_sample = format.wBitsPerSample;
    let (is_pcm, is_float) = match format_tag {
        1 => (true, false), // WAVE_FORMAT_PCM
        3 => (false, true), // WAVE_FORMAT_IEEE_FLOAT
        0xfffe => {
            // WAVE_FORMAT_EXTENSIBLE stores the real PCM/float tag in SubFormat.Data1.
            if format.cbSize < 22 {
                (false, false)
            } else {
                let extensible =
                    unsafe { std::ptr::read_unaligned(format_ptr as *const WAVEFORMATEXTENSIBLE) };
                match extensible.SubFormat.data1 {
                    1 => (true, false),
                    3 => (false, true),
                    _ => (false, false),
                }
            }
        }
        _ => (false, false),
    };
    if !is_pcm && !is_float {
        anyhow::bail!("Unsupported Windows audio format tag: {}", format_tag);
    }
    if format.nChannels == 0 || format.nSamplesPerSec == 0 {
        anyhow::bail!("Windows returned an invalid WASAPI channel or sample rate");
    }

    let raw_format = if is_float {
        if bits_per_sample != 32 {
            anyhow::bail!("Unsupported WASAPI float depth: {} bits", bits_per_sample);
        }
        WasapiRawAudioFormat::Float32Le
    } else {
        match bits_per_sample {
            8 => WasapiRawAudioFormat::Unsigned8,
            16 => WasapiRawAudioFormat::Signed16Le,
            24 => WasapiRawAudioFormat::Signed24Le,
            32 => WasapiRawAudioFormat::Signed32Le,
            bits => anyhow::bail!("Unsupported WASAPI PCM depth: {bits} bits"),
        }
    };
    let expected_block_align = format.nChannels as usize * raw_format.bytes_per_sample();
    let block_align = format.nBlockAlign as usize;
    if block_align != expected_block_align {
        anyhow::bail!(
            "Unsupported WASAPI block alignment: {block_align} (expected {expected_block_align})"
        );
    }

    Ok(WasapiAudioFormat {
        sample_rate: format.nSamplesPerSec,
        channels: format.nChannels,
        block_align,
        raw_format,
    })
}
