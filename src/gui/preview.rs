use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use anyhow::Result;
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use crate::cache::{ThumbnailCache, ThumbnailProfile};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct PreviewKey {
    pub path: PathBuf,
    pub profile: ThumbnailProfile,
}

pub struct PreviewRequest {
    pub request_id: u64,
    pub key: PreviewKey,
}

pub struct PreviewResponse {
    pub request_id: u64,
    pub key: PreviewKey,
    pub image: Result<ColorImage>,
}

pub struct PreviewTextures {
    textures: HashMap<PreviewKey, TextureHandle>,
    lru: VecDeque<PreviewKey>,
    max_entries: usize,
}

impl PreviewTextures {
    const DEFAULT_MAX_PREVIEW_TEXTURES: usize = 8;

    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_MAX_PREVIEW_TEXTURES)
    }

    pub fn with_capacity(max_entries: usize) -> Self {
        Self {
            textures: HashMap::new(),
            lru: VecDeque::new(),
            max_entries: max_entries.max(1),
        }
    }

    pub fn contains(&self, key: &PreviewKey) -> bool {
        self.textures.contains_key(key)
    }

    pub fn get_cloned(&mut self, key: &PreviewKey) -> Option<TextureHandle> {
        let texture = self.textures.get(key).cloned();
        if texture.is_some() {
            self.touch(key);
        }
        texture
    }

    pub fn insert(&mut self, ctx: &egui::Context, key: PreviewKey, image: ColorImage) {
        let texture = ctx.load_texture(
            format!("preview:{}", key.path.display()),
            image,
            TextureOptions::LINEAR,
        );
        self.touch(&key);
        self.textures.insert(key, texture);
        self.evict_if_needed();
    }

    fn touch(&mut self, key: &PreviewKey) {
        if let Some(index) = self.lru.iter().position(|existing| existing == key) {
            self.lru.remove(index);
        }
        self.lru.push_back(key.clone());
    }

    fn evict_if_needed(&mut self) {
        while self.textures.len() > self.max_entries {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            self.textures.remove(&oldest);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.textures.len()
    }

    #[cfg(test)]
    fn lru_paths(&self) -> Vec<PathBuf> {
        self.lru.iter().map(|key| key.path.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{PreviewKey, PreviewTextures};
    use crate::cache::ThumbnailProfile;
    use eframe::egui::{ColorImage, Context};
    use std::path::PathBuf;

    fn key(name: &str) -> PreviewKey {
        PreviewKey {
            path: PathBuf::from(format!("/tmp/{name}.png")),
            profile: ThumbnailProfile::GuiPreview,
        }
    }

    fn image() -> ColorImage {
        ColorImage::from_rgba_unmultiplied([1, 1], &[255, 255, 255, 255])
    }

    #[test]
    fn evicts_least_recent_preview_when_capacity_is_exceeded() {
        let ctx = Context::default();
        let mut textures = PreviewTextures::with_capacity(2);

        let first = key("first");
        let second = key("second");
        let third = key("third");

        textures.insert(&ctx, first.clone(), image());
        textures.insert(&ctx, second.clone(), image());
        textures.insert(&ctx, third.clone(), image());

        assert_eq!(textures.len(), 2);
        assert!(!textures.contains(&first));
        assert!(textures.contains(&second));
        assert!(textures.contains(&third));
    }

    #[test]
    fn touching_entry_makes_it_recent() {
        let ctx = Context::default();
        let mut textures = PreviewTextures::with_capacity(2);

        let first = key("first");
        let second = key("second");
        let third = key("third");

        textures.insert(&ctx, first.clone(), image());
        textures.insert(&ctx, second.clone(), image());
        let _ = textures.get_cloned(&first);
        textures.insert(&ctx, third.clone(), image());

        assert!(textures.contains(&first));
        assert!(!textures.contains(&second));
        assert!(textures.contains(&third));
    }

    #[test]
    fn inserting_existing_key_does_not_grow_cache() {
        let ctx = Context::default();
        let mut textures = PreviewTextures::with_capacity(2);
        let first = key("first");
        let second = key("second");

        textures.insert(&ctx, first.clone(), image());
        textures.insert(&ctx, second.clone(), image());
        textures.insert(&ctx, first.clone(), image());

        assert_eq!(textures.len(), 2);
        assert_eq!(
            textures.lru_paths(),
            vec![second.path.clone(), first.path.clone()]
        );
    }
}

pub fn spawn_preview_worker(
    thumbnail_cache: Option<ThumbnailCache>,
) -> (Sender<PreviewRequest>, Receiver<PreviewResponse>) {
    let (request_tx, request_rx) = mpsc::channel::<PreviewRequest>();
    let (response_tx, response_rx) = mpsc::channel::<PreviewResponse>();

    thread::spawn(move || {
        while let Ok(mut request) = request_rx.recv() {
            while let Ok(next_request) = request_rx.try_recv() {
                request = next_request;
            }

            let image = load_color_image(&request.key, thumbnail_cache.as_ref());
            let _ = response_tx.send(PreviewResponse {
                request_id: request.request_id,
                key: request.key,
                image,
            });
        }
    });

    (request_tx, response_rx)
}

fn load_color_image(
    key: &PreviewKey,
    thumbnail_cache: Option<&ThumbnailCache>,
) -> anyhow::Result<ColorImage> {
    let preview_path = thumbnail_cache
        .and_then(|cache| cache.generate_thumbnail(&key.path, key.profile).ok())
        .unwrap_or_else(|| key.path.clone());
    let rgba = image::open(&preview_path)?.to_rgba8();
    let width = usize::try_from(rgba.width())?;
    let height = usize::try_from(rgba.height())?;
    Ok(ColorImage::from_rgba_unmultiplied(
        [width, height],
        rgba.as_raw(),
    ))
}
