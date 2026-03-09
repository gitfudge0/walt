use image::imageops::FilterType;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const PREVIEW_VERSION: &str = "preview-v5";
const CACHE_DIR: &str = "walt/thumbnails";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ThumbnailProfile {
    TuiPreview,
    GuiPreview,
    #[allow(dead_code)]
    GuiList,
}

impl ThumbnailProfile {
    fn dimensions(self) -> (u32, u32) {
        match self {
            Self::TuiPreview => (576, 324),
            Self::GuiPreview => (1600, 900),
            Self::GuiList => (320, 180),
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::TuiPreview => "tui-preview",
            Self::GuiPreview => "gui-preview",
            Self::GuiList => "gui-list",
        }
    }
}

#[derive(Clone)]
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

    pub fn get_thumbnail_path<P: AsRef<Path>>(
        &self,
        original_path: P,
        profile: ThumbnailProfile,
    ) -> PathBuf {
        let path = original_path.as_ref();
        let hash = Self::hash_path(path, profile);
        let ext = path.extension().unwrap_or_default();
        self.cache_dir
            .join(format!("{}.{}", hash, ext.to_string_lossy()))
    }

    pub fn generate_thumbnail<P: AsRef<Path>>(
        &self,
        original_path: P,
        profile: ThumbnailProfile,
    ) -> anyhow::Result<PathBuf> {
        let original_path = original_path.as_ref();
        let thumb_path = self.get_thumbnail_path(original_path, profile);

        if thumb_path.exists() {
            return Ok(thumb_path);
        }

        // Generate a preview-sized image that preserves the original aspect ratio.
        let img = image::open(original_path)
            .map_err(|e| anyhow::anyhow!("Failed to open image: {}", e))?;
        let (max_width, max_height) = profile.dimensions();

        let resized = img.resize(max_width, max_height, FilterType::Lanczos3);

        // Save thumbnail
        resized
            .save(&thumb_path)
            .map_err(|e| anyhow::anyhow!("Failed to save thumbnail: {}", e))?;

        Ok(thumb_path)
    }

    fn hash_path(path: &Path, profile: ThumbnailProfile) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let path_str = path.to_string_lossy();
        let metadata = fs::metadata(path).ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or_default();
        let modified = metadata
            .and_then(|m| m.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let mut hasher = DefaultHasher::new();
        PREVIEW_VERSION.hash(&mut hasher);
        profile.slug().hash(&mut hasher);
        path_str.hash(&mut hasher);
        size.hash(&mut hasher);
        modified.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::ThumbnailProfile;
    use std::path::Path;

    #[test]
    fn thumbnail_profiles_produce_distinct_hashes() {
        let path = Path::new("/tmp/sample.png");
        let tui = super::ThumbnailCache::hash_path(path, ThumbnailProfile::TuiPreview);
        let gui = super::ThumbnailCache::hash_path(path, ThumbnailProfile::GuiPreview);

        assert_ne!(tui, gui);
    }
}
