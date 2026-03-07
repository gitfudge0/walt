mod backend;
mod cache;
mod config;
mod ui;

use std::env;

use anyhow::{bail, Result};
use rand::seq::SliceRandom;

fn main() -> Result<()> {
    match env::args().nth(1).as_deref() {
        Some("--random") => return print_random_wallpaper(),
        Some("--help") | Some("-h") => {
            print_usage();
            return Ok(());
        }
        Some(arg) => bail!("Unknown argument: {arg}"),
        None => {}
    }

    let mut app = ui::App::new()?;
    app.run()?;
    Ok(())
}

fn print_random_wallpaper() -> Result<()> {
    let config = config::Config::new();

    if config.is_empty() {
        bail!("No wallpaper directories configured. Launch walt once to add paths.")
    }

    let wallpapers = backend::scan_wallpapers_from_paths(&config.wallpaper_paths);
    let wallpaper = wallpapers
        .choose(&mut rand::thread_rng())
        .ok_or_else(|| anyhow::anyhow!("No wallpapers found in configured directories."))?;

    let wallpaper_path = wallpaper.path.to_string_lossy();
    backend::set_wallpaper(&wallpaper_path)?;
    Ok(())
}

fn print_usage() {
    println!("Usage: walt [--random]");
    println!("  --random    Apply a random wallpaper from the configured list");
}
