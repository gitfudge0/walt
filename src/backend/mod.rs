pub mod hyprpaper;
pub mod scanner;

pub use hyprpaper::set_wallpaper;
pub use scanner::{scan_directory, scan_wallpapers_from_paths};
