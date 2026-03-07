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

#[cfg(test)]
mod tests {
    use super::scan_wallpapers_from_paths;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn make_temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("walt-scanner-test-{unique}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn scans_wallpapers_from_all_configured_paths_as_one_list() {
        let root = make_temp_dir();
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).expect("create first source");
        fs::create_dir_all(&second).expect("create second source");

        let first_wallpaper = first.join("alpha.jpg");
        let second_wallpaper = second.join("beta.png");
        fs::write(&first_wallpaper, b"not-an-image-but-valid-extension").expect("write first");
        fs::write(&second_wallpaper, b"not-an-image-but-valid-extension").expect("write second");

        let wallpapers = scan_wallpapers_from_paths(&[first.clone(), second.clone()]);

        assert_eq!(wallpapers.len(), 2);
        assert!(wallpapers
            .iter()
            .any(|wallpaper| wallpaper.path == first_wallpaper));
        assert!(wallpapers
            .iter()
            .any(|wallpaper| wallpaper.path == second_wallpaper));

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }
}
