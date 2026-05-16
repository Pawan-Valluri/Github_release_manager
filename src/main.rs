mod api;
mod app;
mod config;
mod manifest;
mod nu_worker;
mod task_runner;
mod types;

use app::GrmApp;
use std::sync::mpsc;
use types::AsyncMessage;

fn main() -> eframe::Result<()> {
    // --- 1. MULTI-CALL BINARY INTERCEPTOR ---
    // If the binary is called with --nu-worker, we bypass the GUI entirely.
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--nu-worker" {
        nu_worker::run();
        // nu_worker::run() calls process::exit(), so execution will never reach here.
        return Ok(());
    }

    // --- 2. NORMAL GUI BOOTUP ---
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");

    let (tx, rx) = mpsc::channel::<AsyncMessage>();

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
