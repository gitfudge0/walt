use std::{
    collections::HashMap,
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
}

impl PreviewTextures {
    pub fn new() -> Self {
        Self {
            textures: HashMap::new(),
        }
    }

    pub fn contains(&self, key: &PreviewKey) -> bool {
        self.textures.contains_key(key)
    }

    pub fn get(&self, key: &PreviewKey) -> Option<&TextureHandle> {
        self.textures.get(key)
    }

    pub fn insert(&mut self, ctx: &egui::Context, key: PreviewKey, image: ColorImage) {
        let texture = ctx.load_texture(
            format!("preview:{}", key.path.display()),
            image,
            TextureOptions::LINEAR,
        );
        self.textures.insert(key, texture);
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
