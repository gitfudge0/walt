mod app;
mod preview;
mod style;

use std::env;

use anyhow::{bail, Context, Result};
use eframe::egui;

pub fn run() -> Result<()> {
    if env::var_os("DISPLAY").is_none() && env::var_os("WAYLAND_DISPLAY").is_none() {
        bail!("No graphical session found. Use `walt` to launch the terminal UI instead.");
    }

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_title("Walt")
            .with_app_id("walt")
            .with_inner_size([1360.0, 880.0])
            .with_min_inner_size([960.0, 640.0])
            .with_transparent(false),
        ..Default::default()
    };

    eframe::run_native(
        "Walt",
        options,
        Box::new(|cc| Ok(Box::new(app::GuiApp::try_new(cc)?))),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))
    .context("Failed to launch Walt GUI")
}
