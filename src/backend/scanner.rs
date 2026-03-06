use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// All common image formats supported by the image crate
const VALID_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp", "ico", "svg", "ppm", "pgm", "pbm",
    "pam", "hdr", "exr", "ff", "avif", "jxl",
];

#[derive(Debug, Clone)]
pub struct Wallpaper {
    pub path: PathBuf,
    pub name: String,
}

pub fn scan_wallpapers_from_paths(paths: &[PathBuf]) -> Vec<Wallpaper> {
    let mut wallpapers = Vec::new();

    for dir in paths {
        if !dir.is_dir() {
            continue;
        }

        for entry in WalkDir::new(dir)
            .max_depth(2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            if path.is_file() {
                if let Some(ext) = path.extension() {
                    let ext = ext.to_string_lossy().to_lowercase();
                    if VALID_EXTENSIONS.contains(&ext.as_str()) {
                        if let Some(name) = path.file_stem() {
                            wallpapers.push(Wallpaper {
                                path: path.to_path_buf(),
                                name: name.to_string_lossy().to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    wallpapers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    wallpapers
}

pub fn scan_directory(dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            }
        }
    }

    dirs.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });
    dirs
}
