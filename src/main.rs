#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

mod api;
mod app;
mod capture;
mod capture_workflow;
mod credentials;
mod ocr;
mod ocr_ui;
mod provider_ui;
mod runtime_workers;
mod selection_ui;
mod settings;
mod startup;
mod state;
mod text_layout;
mod toolbar_ui;
mod win_utils;
mod window_mode;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    app::run().await
}
