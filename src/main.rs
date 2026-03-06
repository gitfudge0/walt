mod backend;
mod cache;
mod config;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    let mut app = ui::App::new()?;
    app.run()?;
    Ok(())
}
