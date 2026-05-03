mod api;
mod app;
mod config;
mod manifest;
mod types;

use app::GrmApp;
use std::sync::mpsc;
use types::AsyncMessage;

fn main() -> eframe::Result<()> {
    // 1. Create the Tokio runtime for background networking tasks
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");

    // 2. Create the message channel (Worker -> UI thread)
    let (tx, rx) = mpsc::channel::<AsyncMessage>();

    // 3. Setup the eframe window
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([900.0, 700.0])
            .with_min_inner_size([400.0, 300.0]),
        ..Default::default()
    };

    eframe::run_native(
        "GRM - GitHub Releases Manager",
        options,
        Box::new(|_cc| Box::new(GrmApp::new(rt, tx, rx))),
    )
}
