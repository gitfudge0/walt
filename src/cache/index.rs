use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

const INDEX_VERSION: u32 = 1;
const INDEX_DIR: &str = "walt/index";
const INDEX_FILE: &str = "wallpapers.json";
const VALID_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "tiff", "tif", "webp", "ico", "svg", "ppm", "pgm", "pbm",
    "pam", "hdr", "exr", "ff", "avif", "jxl",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexedWallpaper {
    pub path: PathBuf,
    pub name: String,
    pub directory: PathBuf,
    pub extension: String,
    pub modified_unix_secs: u64,
    pub file_size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct WallpaperIndexFile {
    version: u32,
    wallpapers: Vec<IndexedWallpaper>,
}

pub struct WallpaperIndex {
    index_file: PathBuf,
}

impl WallpaperIndex {
    pub fn new() -> anyhow::Result<Self> {
        let index_dir = dirs::cache_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find cache directory"))?
            .join(INDEX_DIR);
        fs::create_dir_all(&index_dir)?;

        Ok(Self {
            index_file: index_dir.join(INDEX_FILE),
        })
    }

    pub fn load(&self, paths: &[PathBuf]) -> Vec<IndexedWallpaper> {
        let configured_paths = canonical_path_set(paths);
        let file = fs::read_to_string(&self.index_file)
            .ok()
            .and_then(|content| serde_json::from_str::<WallpaperIndexFile>(&content).ok());

        let mut wallpapers = file
            .filter(|file| file.version == INDEX_VERSION)
            .map(|file| file.wallpapers)
            .unwrap_or_default();

        wallpapers.retain(|wallpaper| {
            wallpaper.path.exists()
                && canonicalize_parented(&wallpaper.path)
                    .map(|path| starts_with_any(&path, &configured_paths))
                    .unwrap_or(false)
        });
        wallpapers.sort_by(sort_by_name);
        wallpapers
    }

    pub fn refresh(&self, paths: &[PathBuf]) -> anyhow::Result<Vec<IndexedWallpaper>> {
        let existing_by_path = self
            .load(paths)
            .into_iter()
            .map(|wallpaper| (wallpaper.path.clone(), wallpaper))
            .collect::<HashMap<_, _>>();

        let mut wallpapers = Vec::new();

        for directory in paths {
            if !directory.is_dir() {
                continue;
            }

            for entry in WalkDir::new(directory)
                .max_depth(2)
                .into_iter()
                .filter_map(|entry| entry.ok())
            {
                let path = entry.path();
                if !path.is_file() || !has_valid_extension(path) {
                    continue;
                }

                let Some(indexed) = build_indexed_wallpaper(path, existing_by_path.get(path))
                else {
                    continue;
                };
                wallpapers.push(indexed);
            }
        }

        wallpapers.sort_by(sort_by_name);
        self.save(&wallpapers)?;
        Ok(wallpapers)
    }

    fn save(&self, wallpapers: &[IndexedWallpaper]) -> anyhow::Result<()> {
        let payload = WallpaperIndexFile {
            version: INDEX_VERSION,
            wallpapers: wallpapers.to_vec(),
        };
        fs::write(&self.index_file, serde_json::to_vec_pretty(&payload)?)?;
        Ok(())
    }
}

fn build_indexed_wallpaper(
    path: &Path,
    cached: Option<&IndexedWallpaper>,
) -> Option<IndexedWallpaper> {
    let metadata = fs::metadata(path).ok()?;
    let modified_unix_secs = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let file_size = metadata.len();

    if let Some(cached) = cached {
        if cached.file_size == file_size && cached.modified_unix_secs == modified_unix_secs {
            return Some(cached.clone());
        }
    }

    let dimensions = image::image_dimensions(path).ok();
    Some(IndexedWallpaper {
        path: path.to_path_buf(),
        name: path
            .file_stem()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        directory: path.parent().unwrap_or_else(|| Path::new("")).to_path_buf(),
        extension: path
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default(),
        modified_unix_secs,
        file_size,
        width: dimensions.map(|(width, _)| width),
        height: dimensions.map(|(_, height)| height),
    })
}

fn sort_by_name(left: &IndexedWallpaper, right: &IndexedWallpaper) -> std::cmp::Ordering {
    left.name
        .to_lowercase()
        .cmp(&right.name.to_lowercase())
        .then_with(|| left.path.cmp(&right.path))
}

fn has_valid_extension(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .map(|ext| VALID_EXTENSIONS.contains(&ext.as_str()))
        .unwrap_or(false)
}

fn canonical_path_set(paths: &[PathBuf]) -> HashSet<PathBuf> {
    paths
        .iter()
        .filter_map(|path| fs::canonicalize(path).ok())
        .collect()
}

fn canonicalize_parented(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        fs::canonicalize(path).ok()
    } else {
        None
    }
}

fn starts_with_any(path: &Path, roots: &HashSet<PathBuf>) -> bool {
    roots.iter().any(|root| path.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::build_indexed_wallpaper;
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
        let dir = std::env::temp_dir().join(format!("walt-index-test-{unique}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn rebuilds_entry_when_file_size_changes() {
        let root = make_temp_dir();
        let path = root.join("sample.png");
        fs::write(&path, b"first").expect("write first version");
        let initial = build_indexed_wallpaper(&path, None).expect("index first");

        fs::write(&path, b"second version").expect("write second version");
        let refreshed = build_indexed_wallpaper(&path, Some(&initial)).expect("index second");

        assert_ne!(initial.file_size, refreshed.file_size);
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }
}
