# Rust OCR Translator Overlay

A real-time game screen OCR and translation tool built with Rust and Slint.

## Features
- **Modern UI**: Dark mode, glassmorphism, and Windows 11 Mica backdrop for the main control window.
- **Transparent Overlay**: Translation text displayed directly over the game screen with a click-through background.
- **Interactive Region Selection**: Drag your mouse to select exactly what you want to translate.
- **Change Detection**: Intelligent logic avoids redundant API calls by detecting screen changes (threshold: 5%).
- **Multi-API Support**: Works with Google Gemini 1.5/2.0 API (Multimodal) and OpenAI-compatible local models (LMStudio, Ollama).
- **Global Hotkey**: Toggle capture state anytime with `Ctrl + Alt + A`.

## Prerequisites
- **Rust**: [Install Rust](https://www.rust-lang.org/tools/install)
- **C++ Build Tools**: Required for `windows-rs` and `slint`.

## Getting Started
1. Clone the repository.
2. Run `cargo build --release`.
3. Open the main window:
   - Provide your **Gemini API Key** (or use local endpoint).
   - Click **Select Capture Area** and drag to select the subtitle region.
   - Click **START** or press `Ctrl + Alt + A`.

## Project Structure
- `src/main.rs`: Orchestration and event loop.
- `src/capture.rs`: Screen capture and change detection logic.
- `src/api.rs`: Multi-modal AI client (Gemini/OpenAI).
- `src/win_utils.rs`: Windows-specific UI hacks (transparency, Mica).
- `ui/main.slint`: Modern UI definitions for all windows.

## License
MIT
