# Rust OCR Translator Overlay

A real-time screen OCR and translation tool built with Rust and Slint.

<img src="screenshot.png" alt="screenshot1" width="50%">

## Features
- **Modern UI**: Dark mode, glassmorphism, and Windows 11 Mica backdrop for the main control window.
- **Toggleable Overlay**: Dedicated toggle and logic to hide/show the translation box without stopping the OCR process.
- **Interactive Region Selection**: Drag your mouse to select exactly what you want to translate. Supports **Escape** to cancel.
- **Auto-Automation**: Selecting a region automatically starts the capture process for immediate results.
- **Change Detection**: Intelligent logic avoids redundant API calls by detecting screen changes.
- **Clipboard Sync**: Translated text is automatically copied to the system clipboard for easy use elsewhere.
- **Multi-API Support**: 
  - **Google Gemini**: Supports gemini-3.1-flash-lite-preview (Auto-loads API key from `gemini.txt` in the app directory).
  - **LMStudio / Ollama**: Works with any OpenAI-compatible local AI endpoint.(recommended: gemma-4-31b-it (best), qwen3.5-9b (fast), translategemma-12b-it (fast))
- **Global Hotkey**: Trigger area selection anytime with **Win + `**.

## Prerequisites
- **Rust**: [Install Rust](https://www.rust-lang.org/tools/install)
- **C++ Build Tools**: Required for `windows-rs` and `slint`.

## Getting Started
### 📥 Download
You can download the latest version from the [Releases Page](https://github.com/kirinonakar/ocr_trans/releases).
### Manual build

1. Clone the repository.
2. (Optional) Create a `gemini.txt` file next to the executable and paste your Google Gemini API key.
3. Run `cargo run --release`. Or `cargo build --release` to generate the binary.
4. Open the main window:
   - Select your provider (LMStudio or Google Gemini).
   - Click **SELECT AREA** and drag to select the region you want to translate (e.g. subtitles).
   - Capture starts automatically. Use the **STOP** button to pause, or **Win + `** to re-select the area.
   - Use the **Overlay** checkbox to hide/show the translation text while running.

## Project Structure
- `src/main.rs`: Orchestration, event loop, and UI logic.
- `src/capture.rs`: Screen capture and change detection.
- `src/api.rs`: Multi-modal AI client (Gemini/OpenAI).
- `src/win_utils.rs`: Windows-specific UI hacks (layered windows, click-through, Mica).
- `ui/main.slint`: Modern UI definitions for Slint.

## License
This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

