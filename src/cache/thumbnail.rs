use image::imageops::FilterType;
use std::fs;
use std::path::{Path, PathBuf};

const PREVIEW_VERSION: &str = "preview-v4";
const PREVIEW_MAX_WIDTH: u32 = 576;
const PREVIEW_MAX_HEIGHT: u32 = 324;
const CACHE_DIR: &str = "walt/thumbnails";

pub struct ThumbnailCache {
    cache_dir: PathBuf,
}

impl ThumbnailCache {
    pub fn new() -> anyhow::Result<Self> {
        let cache_dir = dirs::cache_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find cache directory"))?
            .join(CACHE_DIR);

        fs::create_dir_all(&cache_dir)
            .map_err(|e| anyhow::anyhow!("Failed to create cache directory: {}", e))?;

        Ok(Self { cache_dir })
    }

    pub fn get_thumbnail_path<P: AsRef<Path>>(&self, original_path: P) -> PathBuf {
        let path = original_path.as_ref();
        let hash = Self::hash_path(path);
        let ext = path.extension().unwrap_or_default();
        self.cache_dir
            .join(format!("{}.{}", hash, ext.to_string_lossy()))
    }

    pub fn generate_thumbnail<P: AsRef<Path>>(&self, original_path: P) -> anyhow::Result<PathBuf> {
        let original_path = original_path.as_ref();
        let thumb_path = self.get_thumbnail_path(original_path);

        if thumb_path.exists() {
            return Ok(thumb_path);
        }

        // Generate a preview-sized image that preserves the original aspect ratio.
        let img = image::open(original_path)
            .map_err(|e| anyhow::anyhow!("Failed to open image: {}", e))?;

        let resized = img.resize(PREVIEW_MAX_WIDTH, PREVIEW_MAX_HEIGHT, FilterType::Lanczos3);

        // Save thumbnail
        resized
            .save(&thumb_path)
            .map_err(|e| anyhow::anyhow!("Failed to save thumbnail: {}", e))?;

        Ok(thumb_path)
    }

    fn hash_path(path: &Path) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let path_str = path.to_string_lossy();
        let mut hasher = DefaultHasher::new();
        PREVIEW_VERSION.hash(&mut hasher);
        path_str.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}
