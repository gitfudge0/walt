use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    time::{Duration, Instant},
};

use chrono::{Local, TimeZone};
use eframe::egui::{
    self, Align, Align2, Color32, FontData, FontDefinitions, FontFamily, FontId, Id, Key,
    Modifiers, ScrollArea, Stroke, TextEdit, TextStyle, TextureHandle, Ui, Vec2,
};
use log::error;
use tokio_util::sync::CancellationToken;

use crate::{
    backend::hyprpaper::get_active_wallpaper_assignments_if_supported,
    backend::{
        apply_random_plan, disable_rotation_service, download_image_async, enable_rotation_service,
        format_rotation_service_status, get_monitors, get_rotation_service_status,
        install_rotation_service, plan_random_assignments, restart_rotation_service_if_active,
        rotation_service_badge, set_wallpaper, set_wallpaper_for_monitor, uninstall_paths,
        uninstall_rotation_service, uninstall_walt, validate_download_url, DownloadProgressEvent,
        DownloadRequest, RandomMode, RandomPlan, RotationServiceStatus,
    },
    cache::{IndexedWallpaper, ThumbnailCache, ThumbnailProfile, WallpaperIndex},
    config::Config,
    shared::{
        active_wallpaper_paths_from_assignments, default_display_target_selection,
        display_targets_from_names, first_active_visible_index,
        merge_active_wallpaper_assignments_from_random_plan, random_apply_action,
        random_menu_actions, selection_for_random_plan, set_active_wallpaper_assignment,
        set_active_wallpaper_assignments_for_all_monitors,
        set_active_wallpaper_assignments_from_backend, wallpaper_apply_action, DisplayTarget,
        RandomApplyAction, RandomMenuAction, WallpaperApplyAction,
    },
    theme::ThemeKind,
};

use super::preview::{
    spawn_preview_worker, PreviewKey, PreviewRequest, PreviewResponse, PreviewTextures,
};
use super::style::{
    interactive_row, show_popup_shell, GuiChrome, GuiPalette, GuiTextRole, GuiTypography,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SectionKind {
    All,
    Rotation,
}

impl SectionKind {
    const ALL: [SectionKind; 2] = [SectionKind::All, SectionKind::Rotation];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Rotation => "Rotation",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Rotation => "rotation",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ViewMode {
    List,
    Gallery,
}

impl ViewMode {
    fn label(self) -> &'static str {
        match self {
            Self::List => "List",
            Self::Gallery => "Gallery",
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SortMode {
    Name,
    Modified,
}

impl SortMode {
    fn from_name(name: &str) -> Self {
        match name {
            "modified" => Self::Modified,
            _ => Self::Name,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ToastKind {
    Info,
    Success,
    Error,
}

struct Toast {
    id: u64,
    kind: ToastKind,
    message: String,
    created_at: Instant,
}

struct IndexRequest {
    request_id: u64,
    wallpaper_paths: Vec<PathBuf>,
}

struct IndexResponse {
    request_id: u64,
    wallpapers: anyhow::Result<Vec<IndexedWallpaper>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BackendStateSnapshot {
    active_wallpaper_assignments: HashMap<String, PathBuf>,
    rotation_service_state: Option<RotationServiceStatus>,
    rotation_status_text: String,
}

struct BackendStateRequest {
    request_id: u64,
    fallback: BackendStateSnapshot,
}

struct BackendStateResponse {
    request_id: u64,
    snapshot: Option<BackendStateSnapshot>,
}

struct DownloadWorkerRequest {
    request_id: u64,
    request: DownloadRequest,
    cancel: CancellationToken,
}

struct DownloadWorkerResponse {
    request_id: u64,
    event: DownloadProgressEvent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum GuiDownloadStage {
    Edit,
    Downloading,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GuiDownloadState {
    stage: GuiDownloadStage,
    final_path: Option<PathBuf>,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
}

pub struct GuiApp {
    config: Config,
    theme: ThemeKind,
    wallpapers: Vec<IndexedWallpaper>,
    view_mode: ViewMode,
    active_section: SectionKind,
    all_indices: Vec<usize>,
    rotation_indices: Vec<usize>,
    selected_all: Option<usize>,
    selected_rotation: Option<usize>,
    all_filter: String,
    rotation_filter: String,
    active_wallpaper_assignments: HashMap<String, PathBuf>,
    active_wallpaper_paths: HashSet<PathBuf>,
    rotation_paths: HashSet<PathBuf>,
    texture_cache: PreviewTextures,
    thumbnail_cache: Option<ThumbnailCache>,
    desired_preview_key: Option<PreviewKey>,
    current_preview_key: Option<PreviewKey>,
    preview_request_in_flight: bool,
    preview_request_id: u64,
    preview_tx: Sender<PreviewRequest>,
    preview_rx: Receiver<PreviewResponse>,
    index_request_id: u64,
    index_tx: Sender<IndexRequest>,
    index_rx: Receiver<IndexResponse>,
    download_request_id: u64,
    download_tx: Sender<DownloadWorkerRequest>,
    download_rx: Receiver<DownloadWorkerResponse>,
    download_cancel_token: Option<CancellationToken>,
    backend_state_request_id: u64,
    backend_state_tx: Sender<BackendStateRequest>,
    backend_state_rx: Receiver<BackendStateResponse>,
    backend_state_refresh_interval: Duration,
    next_backend_state_refresh_at: Instant,
    backend_state_request_in_flight: bool,
    rotation_service_state: Option<RotationServiceStatus>,
    rotation_status_text: String,
    display_targets: Vec<DisplayTarget>,
    display_target_selection: Option<usize>,
    random_targets: Vec<RandomMenuAction>,
    random_target_selection: Option<usize>,
    pending_wallpaper_path: Option<PathBuf>,
    pending_random_candidates: Vec<PathBuf>,
    pending_downloaded_path: Option<PathBuf>,
    show_paths_dialog: bool,
    show_rotation_dialog: bool,
    show_help_dialog: bool,
    show_uninstall_dialog: bool,
    show_display_picker: bool,
    show_random_dialog: bool,
    show_download_dialog: bool,
    manual_path_input: String,
    interval_buffer: String,
    download_url_input: String,
    download_error: Option<String>,
    download_path_selection: Option<usize>,
    last_download_path_selection: Option<usize>,
    download_state: Option<GuiDownloadState>,
    focus_download_url_input: bool,
    uninstall_confirmed: bool,
    uninstall_summary: Option<String>,
    uninstall_close_deadline: Option<Instant>,
    focus_search: bool,
    toasts: Vec<Toast>,
    next_toast_id: u64,
}

impl GuiApp {
    pub fn try_new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let config = Config::new();
        let theme = ThemeKind::from_name(&config.theme_name);
        let wallpaper_index = WallpaperIndex::new()?;
        let wallpapers = if config.is_empty() {
            vec![]
        } else {
            wallpaper_index.load(&config.wallpaper_paths)
        };
        let thumbnail_cache = ThumbnailCache::new().ok();
        let (preview_tx, preview_rx) = spawn_preview_worker(thumbnail_cache.clone());
        let (index_tx, index_rx) = spawn_index_worker(wallpaper_index);
        let (download_tx, download_rx) = spawn_download_worker();
        let (backend_state_tx, backend_state_rx) = spawn_backend_state_worker();
        let backend_state_refresh_interval = Duration::from_secs(2);

        let mut app = Self {
            config,
            theme,
            wallpapers,
            view_mode: ViewMode::List,
            active_section: SectionKind::All,
            all_indices: vec![],
            rotation_indices: vec![],
            selected_all: None,
            selected_rotation: None,
            all_filter: String::new(),
            rotation_filter: String::new(),
            active_wallpaper_assignments: HashMap::new(),
            active_wallpaper_paths: HashSet::new(),
            rotation_paths: HashSet::new(),
            texture_cache: PreviewTextures::new(),
            thumbnail_cache,
            desired_preview_key: None,
            current_preview_key: None,
            preview_request_in_flight: false,
            preview_request_id: 0,
            preview_tx,
            preview_rx,
            index_request_id: 0,
            index_tx,
            index_rx,
            download_request_id: 0,
            download_tx,
            download_rx,
            download_cancel_token: None,
            backend_state_request_id: 0,
            backend_state_tx,
            backend_state_rx,
            backend_state_refresh_interval,
            next_backend_state_refresh_at: Instant::now() + backend_state_refresh_interval,
            backend_state_request_in_flight: false,
            rotation_service_state: None,
            rotation_status_text: String::new(),
            display_targets: vec![],
            display_target_selection: None,
            random_targets: vec![],
            random_target_selection: None,
            pending_wallpaper_path: None,
            pending_random_candidates: vec![],
            pending_downloaded_path: None,
            show_paths_dialog: false,
            show_rotation_dialog: false,
            show_help_dialog: false,
            show_uninstall_dialog: false,
            show_display_picker: false,
            show_random_dialog: false,
            show_download_dialog: false,
            manual_path_input: String::new(),
            interval_buffer: String::new(),
            download_url_input: String::new(),
            download_error: None,
            download_path_selection: None,
            last_download_path_selection: None,
            download_state: None,
            focus_download_url_input: false,
            uninstall_confirmed: false,
            uninstall_summary: None,
            uninstall_close_deadline: None,
            focus_search: false,
            toasts: vec![],
            next_toast_id: 0,
        };

        app.rebuild_section_cache();
        app.ensure_section_selection();
        app.refresh_active_wallpapers(true);
        app.select_active_wallpaper_in_all();
        app.refresh_rotation_status();
        install_editorial_mono(&cc.egui_ctx);
        app.apply_visuals(&cc.egui_ctx);
        if !app.config.is_empty() {
            app.request_index_refresh();
        }
        app.request_preview_load();

        Ok(app)
    }

    fn apply_visuals(&self, ctx: &egui::Context) {
        let palette = self.palette();
        let mut visuals = egui::Visuals::dark();
        visuals.override_text_color = Some(palette.text);
        visuals.panel_fill = palette.background;
        visuals.window_fill = palette.background;
        visuals.extreme_bg_color = palette.surface_alt;
        visuals.faint_bg_color = palette.surface_alt;
        visuals.code_bg_color = palette.surface_alt;
        visuals.selection.bg_fill = palette.surface_alt;
        visuals.selection.stroke = Stroke::new(1.0, palette.text);
        visuals.widgets.noninteractive.bg_fill = palette.background;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);
        visuals.widgets.inactive.bg_fill = palette.background;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, palette.text);
        visuals.widgets.hovered.bg_fill = palette.surface_alt;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.highlight);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, palette.text);
        visuals.widgets.active.bg_fill = palette.surface_alt;
        visuals.widgets.active.bg_stroke = Stroke::new(1.5, palette.highlight);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, palette.text);
        visuals.widgets.open.bg_fill = Color32::TRANSPARENT;
        visuals.window_stroke = Stroke::new(1.0, palette.border);
        visuals.hyperlink_color = palette.accent;
        visuals.menu_corner_radius = 0.0.into();
        visuals.window_corner_radius = 0.0.into();

        let mut style = (*ctx.style()).clone();
        style.visuals = visuals;
        style.spacing.item_spacing = egui::vec2(8.0, 10.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.window_margin = egui::Margin::same(18);
        style.spacing.menu_margin = egui::Margin::same(14);
        style.text_styles = [
            (TextStyle::Heading, FontId::monospace(22.0)),
            (TextStyle::Body, FontId::monospace(16.0)),
            (TextStyle::Button, FontId::monospace(14.0)),
            (TextStyle::Small, FontId::monospace(12.0)),
            (TextStyle::Monospace, FontId::monospace(14.0)),
        ]
        .into();
        ctx.set_style(style);
    }

    fn palette(&self) -> GuiPalette {
        fn rgb(r: u8, g: u8, b: u8) -> Color32 {
            Color32::from_rgb(r, g, b)
        }

        let accent = match self.theme {
            ThemeKind::System => rgb(205, 214, 255),
            ThemeKind::CatppuccinMocha => rgb(249, 226, 175),
            ThemeKind::TokyoNight => rgb(187, 154, 247),
            ThemeKind::GruvboxDark => rgb(250, 189, 47),
            ThemeKind::Dracula => rgb(255, 184, 108),
            ThemeKind::Nord => rgb(180, 142, 173),
            ThemeKind::SolarizedDark => rgb(181, 137, 0),
            ThemeKind::Kanagawa => rgb(255, 160, 102),
            ThemeKind::OneDark => rgb(97, 175, 239),
            ThemeKind::EverforestDark => rgb(230, 197, 71),
            ThemeKind::RosePine => rgb(235, 188, 186),
        };

        GuiPalette {
            background: rgb(0, 0, 0),
            surface: rgb(6, 6, 6),
            surface_alt: rgb(15, 15, 15),
            border: rgb(28, 28, 28),
            accent,
            highlight: rgb(241, 241, 236),
            text: rgb(236, 236, 232),
            muted: rgb(128, 128, 123),
            danger: rgb(198, 102, 102),
            success: rgb(131, 182, 101),
        }
    }

    fn push_toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.next_toast_id = self.next_toast_id.wrapping_add(1);
        self.toasts.push(Toast {
            id: self.next_toast_id,
            kind,
            message: message.into(),
            created_at: Instant::now(),
        });
    }

    fn info(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::info!("gui info: {message}");
        self.push_toast(ToastKind::Info, message);
    }

    fn success(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::info!("gui success: {message}");
        self.push_toast(ToastKind::Success, message);
    }

    fn error(&mut self, message: impl Into<String>) {
        let message = message.into();
        log::error!("gui error: {message}");
        self.push_toast(ToastKind::Error, message);
    }

    fn expire_toasts(&mut self) {
        self.toasts
            .retain(|toast| toast.created_at.elapsed() < Duration::from_secs(5));
    }

    fn rebuild_section_cache(&mut self) {
        self.rotation_paths = self.config.rotation.iter().cloned().collect();
        self.all_indices.clear();
        self.rotation_indices.clear();

        for (index, wallpaper) in self.wallpapers.iter().enumerate() {
            self.all_indices.push(index);
            if self.rotation_paths.contains(&wallpaper.path) {
                self.rotation_indices.push(index);
            }
        }
    }

    fn sort_mode(&self, section: SectionKind) -> SortMode {
        SortMode::from_name(self.config.sort_name_for_section(section.key()))
    }

    fn filter_query(&self, section: SectionKind) -> &str {
        match section {
            SectionKind::All => &self.all_filter,
            SectionKind::Rotation => &self.rotation_filter,
        }
    }

    fn selected_index(&self, section: SectionKind) -> Option<usize> {
        match section {
            SectionKind::All => self.selected_all,
            SectionKind::Rotation => self.selected_rotation,
        }
    }

    fn set_selected_index(&mut self, section: SectionKind, selected: Option<usize>) {
        match section {
            SectionKind::All => self.selected_all = selected,
            SectionKind::Rotation => self.selected_rotation = selected,
        }
    }

    fn set_view_mode(&mut self, view_mode: ViewMode) {
        self.view_mode = view_mode;
        if self.view_mode == ViewMode::List {
            self.request_preview_load();
        }
    }

    fn cycle_active_section(&mut self, delta: isize) {
        let current = SectionKind::ALL
            .iter()
            .position(|section| *section == self.active_section)
            .unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(SectionKind::ALL.len() as isize) as usize;
        self.active_section = SectionKind::ALL[next];
        self.ensure_section_selection();
        self.request_preview_load();
    }

    fn section_is_informational(&self, section: SectionKind) -> bool {
        section == SectionKind::Rotation && self.config.uses_all_wallpapers_for_rotation()
    }

    fn section_indices(&self, section: SectionKind) -> Vec<usize> {
        if self.section_is_informational(section) {
            return vec![];
        }

        let base = match section {
            SectionKind::All => &self.all_indices,
            SectionKind::Rotation => &self.rotation_indices,
        };
        let filter = self.filter_query(section).to_lowercase();
        let mut indices = base
            .iter()
            .copied()
            .filter(|index| {
                let wallpaper = &self.wallpapers[*index];
                if filter.is_empty() {
                    true
                } else {
                    wallpaper.name.to_lowercase().contains(&filter)
                        || wallpaper
                            .path
                            .to_string_lossy()
                            .to_lowercase()
                            .contains(&filter)
                }
            })
            .collect::<Vec<_>>();

        let sort_mode = self.sort_mode(section);
        indices.sort_by(|left, right| {
            let left = &self.wallpapers[*left];
            let right = &self.wallpapers[*right];
            match sort_mode {
                SortMode::Name => left
                    .name
                    .to_lowercase()
                    .cmp(&right.name.to_lowercase())
                    .then_with(|| left.path.cmp(&right.path)),
                SortMode::Modified => right
                    .modified_unix_secs
                    .cmp(&left.modified_unix_secs)
                    .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase())),
            }
        });
        indices
    }

    fn ensure_section_selection(&mut self) {
        for section in [SectionKind::All, SectionKind::Rotation] {
            let len = self.section_indices(section).len();
            let next = if self.section_is_informational(section) || len == 0 {
                None
            } else {
                Some(self.selected_index(section).unwrap_or(0).min(len - 1))
            };
            self.set_selected_index(section, next);
        }
    }

    fn current_selected_wallpaper(&self) -> Option<&IndexedWallpaper> {
        let indices = self.section_indices(self.active_section);
        let selected = self.selected_index(self.active_section)?;
        let wallpaper_index = *indices.get(selected)?;
        self.wallpapers.get(wallpaper_index)
    }

    fn current_selected_path(&self) -> Option<PathBuf> {
        self.current_selected_wallpaper()
            .map(|wallpaper| wallpaper.path.clone())
    }

    fn select_path_in_section(&mut self, section: SectionKind, path: &PathBuf) -> bool {
        let indices = self.section_indices(section);
        if let Some(selected) = indices.iter().position(|index| {
            self.wallpapers
                .get(*index)
                .map(|wallpaper| &wallpaper.path == path)
                .unwrap_or(false)
        }) {
            self.set_selected_index(section, Some(selected));
            true
        } else {
            false
        }
    }

    fn select_active_wallpaper_in_all(&mut self) {
        let indices = self.section_indices(SectionKind::All);
        if let Some(selected) =
            first_active_visible_index(&indices, &self.wallpapers, &self.active_wallpaper_paths)
        {
            self.selected_all = Some(selected);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let section = self.active_section;
        let len = self.section_indices(section).len();
        if len == 0 {
            self.set_selected_index(section, None);
            return;
        }

        let current = self.selected_index(section).unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, (len - 1) as isize) as usize;
        self.set_selected_index(section, Some(next));
        self.request_preview_load();
    }

    fn gallery_texture(&mut self, ctx: &egui::Context, path: &PathBuf) -> Option<TextureHandle> {
        let key = PreviewKey {
            path: path.clone(),
            profile: ThumbnailProfile::GuiList,
        };
        if let Some(texture) = self.texture_cache.get_cloned(&key) {
            return Some(texture);
        }

        let preview_path = self
            .thumbnail_cache
            .as_ref()
            .and_then(|cache| cache.generate_thumbnail(path, ThumbnailProfile::GuiList).ok())
            .unwrap_or_else(|| path.clone());
        let rgba = image::open(&preview_path).ok()?.to_rgba8();
        let width = usize::try_from(rgba.width()).ok()?;
        let height = usize::try_from(rgba.height()).ok()?;
        self.texture_cache.insert(
            ctx,
            key.clone(),
            egui::ColorImage::from_rgba_unmultiplied([width, height], rgba.as_raw()),
        );
        self.texture_cache.get_cloned(&key)
    }

    fn refresh_active_wallpapers(&mut self, quiet: bool) {
        match get_active_wallpaper_assignments_if_supported() {
            Ok(Some(assignments)) => {
                log::debug!("gui active wallpaper refresh used backend state");
                set_active_wallpaper_assignments_from_backend(
                    &mut self.active_wallpaper_assignments,
                    &assignments,
                );
                self.active_wallpaper_paths =
                    active_wallpaper_paths_from_assignments(&self.active_wallpaper_assignments);
            }
            Ok(None) => {
                log::debug!(
                    "gui active wallpaper refresh kept local state because backend status query is unsupported"
                );
            }
            Err(error) if !quiet => {
                self.error(format!("Failed to refresh active wallpapers: {error}"))
            }
            Err(_) => {}
        }
    }

    fn refresh_rotation_status(&mut self) {
        match get_rotation_service_status() {
            Ok(status) => {
                self.rotation_status_text = format_rotation_service_status(&status);
                self.rotation_service_state = Some(status);
            }
            Err(error) => {
                self.rotation_service_state = None;
                self.rotation_status_text =
                    format!("Rotation Service\nStatus:   error\nError:    {error}");
            }
        }
    }

    fn current_backend_state_snapshot(&self) -> BackendStateSnapshot {
        BackendStateSnapshot {
            active_wallpaper_assignments: self.active_wallpaper_assignments.clone(),
            rotation_service_state: self.rotation_service_state.clone(),
            rotation_status_text: self.rotation_status_text.clone(),
        }
    }

    fn apply_backend_state_snapshot(&mut self, snapshot: BackendStateSnapshot) -> bool {
        let changed = self.active_wallpaper_assignments != snapshot.active_wallpaper_assignments
            || self.rotation_service_state != snapshot.rotation_service_state
            || self.rotation_status_text != snapshot.rotation_status_text;

        if changed {
            self.active_wallpaper_assignments = snapshot.active_wallpaper_assignments;
            self.active_wallpaper_paths =
                active_wallpaper_paths_from_assignments(&self.active_wallpaper_assignments);
            self.rotation_service_state = snapshot.rotation_service_state;
            self.rotation_status_text = snapshot.rotation_status_text;
        }

        changed
    }

    fn maybe_request_backend_state_refresh(&mut self) {
        let now = Instant::now();
        if now < self.next_backend_state_refresh_at || self.backend_state_request_in_flight {
            return;
        }

        self.backend_state_request_id = self.backend_state_request_id.wrapping_add(1);
        if self
            .backend_state_tx
            .send(BackendStateRequest {
                request_id: self.backend_state_request_id,
                fallback: self.current_backend_state_snapshot(),
            })
            .is_ok()
        {
            self.backend_state_request_in_flight = true;
            self.next_backend_state_refresh_at = now + self.backend_state_refresh_interval;
        }
    }

    fn drain_backend_state_updates(&mut self, ctx: &egui::Context) {
        loop {
            match self.backend_state_rx.try_recv() {
                Ok(response) => {
                    if response.request_id != self.backend_state_request_id {
                        continue;
                    }

                    self.backend_state_request_in_flight = false;
                    if let Some(snapshot) = response.snapshot {
                        if self.apply_backend_state_snapshot(snapshot) {
                            ctx.request_repaint();
                        }
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn request_preview_load(&mut self) {
        let Some(key) = self.selected_preview_key() else {
            self.desired_preview_key = None;
            self.current_preview_key = None;
            self.preview_request_in_flight = false;
            return;
        };

        if self.texture_cache.contains(&key) {
            self.desired_preview_key = Some(key.clone());
            self.current_preview_key = Some(key);
            self.preview_request_in_flight = false;
            return;
        }

        if self.preview_request_in_flight && self.desired_preview_key.as_ref() == Some(&key) {
            return;
        }

        self.desired_preview_key = Some(key.clone());
        self.preview_request_in_flight = true;
        self.preview_request_id = self.preview_request_id.wrapping_add(1);
        let _ = self.preview_tx.send(PreviewRequest {
            request_id: self.preview_request_id,
            key,
        });
    }

    fn selected_preview_key(&self) -> Option<PreviewKey> {
        let path = self.current_selected_path()?;
        Some(PreviewKey {
            path,
            profile: ThumbnailProfile::GuiPreview,
        })
    }

    fn ensure_selected_preview_is_available(&mut self) {
        let Some(key) = self.selected_preview_key() else {
            self.desired_preview_key = None;
            self.current_preview_key = None;
            self.preview_request_in_flight = false;
            return;
        };

        self.desired_preview_key = Some(key.clone());
        if self.texture_cache.contains(&key) {
            self.current_preview_key = Some(key);
            self.preview_request_in_flight = false;
            return;
        }

        if !self.preview_request_in_flight {
            self.request_preview_load();
        }
    }

    fn drain_preview_updates(&mut self, ctx: &egui::Context) {
        loop {
            match self.preview_rx.try_recv() {
                Ok(response) => {
                    ctx.request_repaint();
                    if response.request_id != self.preview_request_id {
                        continue;
                    }

                    self.preview_request_in_flight = false;
                    match response.image {
                        Ok(image) => {
                            self.texture_cache.insert(ctx, response.key.clone(), image);
                            self.current_preview_key = Some(response.key);
                        }
                        Err(error) => self.error(format!("Failed to load preview: {error}")),
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn request_index_refresh(&mut self) {
        self.index_request_id = self.index_request_id.wrapping_add(1);
        let _ = self.index_tx.send(IndexRequest {
            request_id: self.index_request_id,
            wallpaper_paths: self.config.wallpaper_paths.clone(),
        });
    }

    fn drain_index_updates(&mut self, ctx: &egui::Context) {
        loop {
            match self.index_rx.try_recv() {
                Ok(response) => {
                    ctx.request_repaint();
                    if response.request_id != self.index_request_id {
                        continue;
                    }

                    match response.wallpapers {
                        Ok(wallpapers) => {
                            let selected_path = self.current_selected_path();
                            self.wallpapers = wallpapers;
                            self.rebuild_section_cache();
                            self.ensure_section_selection();
                            if self.select_downloaded_wallpaper_in_all() {
                                // Selection already synchronized to the newly downloaded path.
                            } else if let Some(path) = selected_path {
                                let _ = self.select_path_in_section(self.active_section, &path);
                            } else {
                                self.select_active_wallpaper_in_all();
                            }
                            self.request_preview_load();
                        }
                        Err(error) => {
                            self.error(format!("Failed to refresh wallpaper index: {error}"))
                        }
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn select_downloaded_wallpaper_in_all(&mut self) -> bool {
        let Some(path) = self.pending_downloaded_path.clone() else {
            return false;
        };

        self.active_section = SectionKind::All;
        if self.select_path_in_section(SectionKind::All, &path) {
            self.pending_downloaded_path = None;
            true
        } else {
            false
        }
    }

    fn open_download_dialog(&mut self) {
        self.show_download_dialog = true;
        self.focus_download_url_input = true;
        self.download_error = None;
        self.download_state
            .get_or_insert_with(|| GuiDownloadState {
                stage: GuiDownloadStage::Edit,
                final_path: None,
                downloaded_bytes: 0,
                total_bytes: None,
            })
            .stage = GuiDownloadStage::Edit;
        self.normalize_download_path_selection();
    }

    fn normalize_download_path_selection(&mut self) {
        let next = if self.config.wallpaper_paths.is_empty() {
            None
        } else {
            self.last_download_path_selection
                .filter(|index| *index < self.config.wallpaper_paths.len())
                .or(Some(0))
        };
        self.download_path_selection = next;
        self.last_download_path_selection = next;
    }

    fn start_download(&mut self) {
        match validate_download_url(&self.download_url_input) {
            Ok(_) => {}
            Err(error) => {
                self.download_error = Some(error.to_string());
                self.download_state = Some(GuiDownloadState {
                    stage: GuiDownloadStage::Edit,
                    final_path: None,
                    downloaded_bytes: 0,
                    total_bytes: None,
                });
                return;
            }
        }

        self.normalize_download_path_selection();
        let Some(index) = self.download_path_selection else {
            self.download_error =
                Some("No wallpaper paths configured. Add a path first.".to_string());
            self.download_state = Some(GuiDownloadState {
                stage: GuiDownloadStage::Edit,
                final_path: None,
                downloaded_bytes: 0,
                total_bytes: None,
            });
            return;
        };
        let Some(destination_dir) = self.config.wallpaper_paths.get(index).cloned() else {
            self.download_error = Some("Choose a valid wallpaper path.".to_string());
            self.download_state = Some(GuiDownloadState {
                stage: GuiDownloadStage::Edit,
                final_path: None,
                downloaded_bytes: 0,
                total_bytes: None,
            });
            return;
        };

        self.last_download_path_selection = Some(index);
        self.download_request_id = self.download_request_id.wrapping_add(1);
        let cancel = CancellationToken::new();
        self.download_cancel_token = Some(cancel.clone());
        self.download_error = None;
        self.download_state = Some(GuiDownloadState {
            stage: GuiDownloadStage::Downloading,
            final_path: None,
            downloaded_bytes: 0,
            total_bytes: None,
        });

        if let Err(send_error) = self.download_tx.send(DownloadWorkerRequest {
            request_id: self.download_request_id,
            request: DownloadRequest {
                url: self.download_url_input.trim().to_string(),
                destination_dir,
            },
            cancel,
        }) {
            error!("failed to send download request to worker: {send_error}");
            self.download_cancel_token = None;
            self.download_error = Some("Failed to start download. Please try again.".to_string());
            self.download_state = Some(GuiDownloadState {
                stage: GuiDownloadStage::Edit,
                final_path: None,
                downloaded_bytes: 0,
                total_bytes: None,
            });
            self.focus_download_url_input = true;
        }
    }

    fn handle_download_event(&mut self, event: DownloadProgressEvent) {
        match event {
            DownloadProgressEvent::Started {
                final_path,
                total_bytes,
            } => {
                self.download_state = Some(GuiDownloadState {
                    stage: GuiDownloadStage::Downloading,
                    final_path: Some(final_path),
                    downloaded_bytes: 0,
                    total_bytes,
                });
            }
            DownloadProgressEvent::Advanced {
                downloaded_bytes,
                total_bytes,
            } => {
                if let Some(state) = self.download_state.as_mut() {
                    state.downloaded_bytes = downloaded_bytes;
                    state.total_bytes = total_bytes;
                }
            }
            DownloadProgressEvent::Finished { saved_path } => {
                self.download_cancel_token = None;
                self.pending_downloaded_path = Some(saved_path.clone());
                self.request_index_refresh();
                self.show_download_dialog = false;
                self.download_state = Some(GuiDownloadState {
                    stage: GuiDownloadStage::Edit,
                    final_path: Some(saved_path.clone()),
                    downloaded_bytes: 0,
                    total_bytes: None,
                });
                if let Err(error) = self.apply_downloaded_wallpaper(saved_path) {
                    self.error(format!("Failed to apply wallpaper: {error}"));
                }
            }
            DownloadProgressEvent::Failed { message } => {
                self.download_cancel_token = None;
                self.download_state = Some(GuiDownloadState {
                    stage: GuiDownloadStage::Failed(message),
                    final_path: self
                        .download_state
                        .as_ref()
                        .and_then(|state| state.final_path.clone()),
                    downloaded_bytes: self
                        .download_state
                        .as_ref()
                        .map(|state| state.downloaded_bytes)
                        .unwrap_or(0),
                    total_bytes: self
                        .download_state
                        .as_ref()
                        .and_then(|state| state.total_bytes),
                });
            }
            DownloadProgressEvent::Cancelled => {
                self.download_cancel_token = None;
                self.download_state = Some(GuiDownloadState {
                    stage: GuiDownloadStage::Cancelled,
                    final_path: self
                        .download_state
                        .as_ref()
                        .and_then(|state| state.final_path.clone()),
                    downloaded_bytes: self
                        .download_state
                        .as_ref()
                        .map(|state| state.downloaded_bytes)
                        .unwrap_or(0),
                    total_bytes: self
                        .download_state
                        .as_ref()
                        .and_then(|state| state.total_bytes),
                });
            }
        }
    }

    fn reset_download_dialog_for_retry(&mut self) {
        self.download_error = None;
        self.normalize_download_path_selection();
        self.download_state = Some(GuiDownloadState {
            stage: GuiDownloadStage::Edit,
            final_path: None,
            downloaded_bytes: 0,
            total_bytes: None,
        });
        self.focus_download_url_input = true;
    }

    fn close_download_dialog(&mut self) {
        let is_downloading = self
            .download_state
            .as_ref()
            .map(|state| state.stage == GuiDownloadStage::Downloading)
            .unwrap_or(false);
        if is_downloading {
            if let Some(cancel_token) = self.download_cancel_token.take() {
                cancel_token.cancel();
            }
            return;
        }

        self.show_download_dialog = false;
        self.download_error = None;
        self.download_state = None;
        self.focus_download_url_input = false;
    }

    fn drain_download_updates(&mut self, ctx: &egui::Context) {
        loop {
            match self.download_rx.try_recv() {
                Ok(response) => {
                    ctx.request_repaint();
                    if response.request_id != self.download_request_id {
                        continue;
                    }

                    self.handle_download_event(response.event);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn toggle_rotation_membership(&mut self) {
        let Some(path) = self.current_selected_path() else {
            return;
        };

        let added = self.config.toggle_rotation(&path);
        match self.config.save() {
            Ok(()) => {
                self.rebuild_section_cache();
                self.ensure_section_selection();
                if self.show_rotation_dialog {
                    self.refresh_rotation_status();
                }
                if added {
                    self.success("Added wallpaper to rotation list.");
                } else {
                    self.info("Removed wallpaper from rotation list.");
                }
            }
            Err(error) => self.error(format!("Failed to save rotation list: {error}")),
        }
    }

    fn add_path(&mut self, path: PathBuf) {
        if !path.is_dir() {
            self.error(format!("Wallpaper path does not exist: {}", path.display()));
            return;
        }

        self.config.add_path(path.clone());
        match self.config.save() {
            Ok(()) => {
                self.manual_path_input.clear();
                self.rebuild_section_cache();
                self.ensure_section_selection();
                self.request_index_refresh();
                self.request_preview_load();
                self.success(format!("Added wallpaper path: {}", path.display()));
            }
            Err(error) => self.error(format!("Failed to save wallpaper path: {error}")),
        }
    }

    fn remove_path(&mut self, path: &PathBuf) {
        self.config.remove_path(path);
        match self.config.save() {
            Ok(()) => {
                self.request_index_refresh();
                self.rebuild_section_cache();
                self.ensure_section_selection();
                self.request_preview_load();
                self.info(format!("Removed wallpaper path: {}", path.display()));
            }
            Err(error) => self.error(format!("Failed to remove wallpaper path: {error}")),
        }
    }

    fn trigger_random_wallpaper(&mut self) {
        let candidates = self.visible_wallpaper_paths();
        if candidates.is_empty() {
            self.info("No wallpapers available in the active view.");
            return;
        }

        match random_apply_action(&get_monitors()) {
            RandomApplyAction::ErrorNoMonitors => self.error("No monitors found."),
            RandomApplyAction::ApplyToSingleDisplay => {
                if let Err(error) =
                    self.apply_random_wallpaper_mode(RandomMode::DisplayIndex(0), &candidates)
                {
                    self.error(format!("Failed to apply random wallpaper: {error}"));
                }
            }
            RandomApplyAction::OpenRandomMenu(monitor_names) => {
                self.random_targets = random_menu_actions(&monitor_names);
                self.random_target_selection = Some(0);
                self.pending_random_candidates = candidates;
                self.show_random_dialog = true;
            }
        }
    }

    fn apply_random_wallpaper_mode(
        &mut self,
        mode: RandomMode,
        candidates: &[PathBuf],
    ) -> anyhow::Result<()> {
        log::info!(
            "gui applying random wallpaper mode={:?} candidates={}",
            mode,
            candidates.len()
        );
        let monitors = get_monitors();
        let plan = plan_random_assignments(&monitors, candidates, mode)?;
        apply_random_plan(&plan)?;
        merge_active_wallpaper_assignments_from_random_plan(
            &mut self.active_wallpaper_assignments,
            &plan,
        );
        self.active_wallpaper_paths =
            active_wallpaper_paths_from_assignments(&self.active_wallpaper_assignments);
        self.refresh_active_wallpapers(false);
        self.sync_selection_with_random_plan(&plan);

        if let Some(assignment) = plan.assignments.first() {
            if matches!(plan.mode, RandomMode::DifferentAll) {
                self.success("Applied different random wallpapers across displays.");
            } else {
                self.success(format!(
                    "Random wallpaper applied: {}",
                    assignment.wallpaper_path.display()
                ));
            }
        }

        Ok(())
    }

    fn sync_selection_with_random_plan(&mut self, plan: &RandomPlan) {
        let indices = self.section_indices(self.active_section);
        if let Some(selected) = selection_for_random_plan(&indices, &self.wallpapers, plan) {
            self.set_selected_index(self.active_section, Some(selected));
            self.request_preview_load();
        }
    }

    fn visible_wallpaper_paths(&self) -> Vec<PathBuf> {
        self.section_indices(self.active_section)
            .into_iter()
            .filter_map(|index| self.wallpapers.get(index))
            .map(|wallpaper| wallpaper.path.clone())
            .collect()
    }

    fn apply_selected_wallpaper(&mut self) {
        let Some(path) = self.current_selected_path() else {
            return;
        };

        match wallpaper_apply_action(&get_monitors()) {
            WallpaperApplyAction::ErrorNoMonitors => self.error("No monitors found."),
            WallpaperApplyAction::ApplyToSingleDisplay(monitor_name) => {
                if let Err(error) = self.apply_wallpaper_to_single_display(&monitor_name, &path) {
                    self.error(format!("Failed to set wallpaper: {error}"));
                }
            }
            WallpaperApplyAction::OpenDisplayPicker(monitor_names) => {
                self.display_targets = display_targets_from_names(&monitor_names);
                self.display_target_selection =
                    default_display_target_selection(&self.display_targets);
                self.pending_wallpaper_path = Some(path);
                self.show_display_picker = true;
            }
        }
    }

    fn apply_downloaded_wallpaper(&mut self, path: PathBuf) -> anyhow::Result<()> {
        match wallpaper_apply_action(&get_monitors()) {
            WallpaperApplyAction::ErrorNoMonitors => Err(anyhow::anyhow!("No monitors found")),
            WallpaperApplyAction::ApplyToSingleDisplay(monitor_name) => {
                self.apply_wallpaper_to_single_display(&monitor_name, &path)
            }
            WallpaperApplyAction::OpenDisplayPicker(monitor_names) => {
                self.display_targets = display_targets_from_names(&monitor_names);
                self.display_target_selection =
                    default_display_target_selection(&self.display_targets);
                self.pending_wallpaper_path = Some(path);
                self.show_display_picker = true;
                Ok(())
            }
        }
    }

    fn apply_wallpaper_to_single_display(
        &mut self,
        monitor_name: &str,
        wallpaper_path: &PathBuf,
    ) -> anyhow::Result<()> {
        let path_str = wallpaper_path.to_string_lossy().to_string();
        log::info!(
            "gui applying wallpaper to single display monitor={} path={}",
            monitor_name,
            path_str
        );
        set_wallpaper_for_monitor(monitor_name, &path_str)?;
        set_active_wallpaper_assignment(
            &mut self.active_wallpaper_assignments,
            monitor_name,
            wallpaper_path,
        );
        self.active_wallpaper_paths =
            active_wallpaper_paths_from_assignments(&self.active_wallpaper_assignments);
        self.refresh_active_wallpapers(false);
        self.success(format!("Wallpaper set on {monitor_name}: {path_str}"));
        Ok(())
    }

    fn apply_wallpaper_to_all_displays(&mut self, wallpaper_path: &PathBuf) -> anyhow::Result<()> {
        let path_str = wallpaper_path.to_string_lossy().to_string();
        log::info!("gui applying wallpaper to all displays path={path_str}");
        set_wallpaper(&path_str)?;
        let monitors = get_monitors();
        set_active_wallpaper_assignments_for_all_monitors(
            &mut self.active_wallpaper_assignments,
            &monitors,
            wallpaper_path,
        );
        self.active_wallpaper_paths =
            active_wallpaper_paths_from_assignments(&self.active_wallpaper_assignments);
        self.refresh_active_wallpapers(false);
        self.success(format!("Wallpaper set on all displays: {path_str}"));
        Ok(())
    }

    fn apply_display_selection(&mut self) {
        let Some(path) = self.pending_wallpaper_path.clone() else {
            return;
        };
        let Some(index) = self.display_target_selection else {
            return;
        };
        let Some(target) = self.display_targets.get(index).cloned() else {
            return;
        };

        let result = match target {
            DisplayTarget::Monitor(name) => self.apply_wallpaper_to_single_display(&name, &path),
            DisplayTarget::AllDisplays => self.apply_wallpaper_to_all_displays(&path),
        };

        match result {
            Ok(()) => {
                self.show_display_picker = false;
                self.display_targets.clear();
                self.pending_wallpaper_path = None;
            }
            Err(error) => self.error(format!("Failed to apply wallpaper: {error}")),
        }
    }

    fn apply_random_selection(&mut self) {
        let Some(index) = self.random_target_selection else {
            return;
        };
        let Some(action) = self.random_targets.get(index).cloned() else {
            return;
        };

        let candidates = self.pending_random_candidates.clone();
        match self.apply_random_wallpaper_mode(action.mode(), &candidates) {
            Ok(()) => {
                self.show_random_dialog = false;
                self.random_targets.clear();
                self.pending_random_candidates.clear();
            }
            Err(error) => self.error(format!("Failed to apply random wallpaper: {error}")),
        }
    }

    fn run_rotation_service_action<F>(&mut self, action: F, success_message: &'static str)
    where
        F: FnOnce() -> anyhow::Result<()>,
    {
        match action() {
            Ok(()) => {
                self.refresh_rotation_status();
                self.success(success_message);
            }
            Err(error) => self.error(format!("Rotation action failed: {error}")),
        }
    }

    fn toggle_rotate_all_wallpapers(&mut self) {
        match self.config.toggle_rotate_all_wallpapers() {
            Ok(enabled) => {
                self.rebuild_section_cache();
                self.ensure_section_selection();
                self.restart_rotation_service_if_active();
                self.refresh_rotation_status();
                self.request_preview_load();
                self.info(if enabled {
                    "Rotation will use all indexed wallpapers."
                } else {
                    "Rotation will use only the selected wallpaper list."
                });
            }
            Err(error) => self.error(format!("Failed to update rotation mode: {error}")),
        }
    }

    fn toggle_rotation_display_mode(&mut self) {
        match self.config.toggle_rotation_same_wallpaper_on_all_displays() {
            Ok(enabled) => {
                self.restart_rotation_service_if_active();
                self.refresh_rotation_status();
                self.info(if enabled {
                    "Rotation now uses the same wallpaper on all displays."
                } else {
                    "Rotation now uses different wallpapers per display."
                });
            }
            Err(error) => self.error(format!("Failed to update display rotation mode: {error}")),
        }
    }

    fn restart_rotation_service_if_active(&mut self) {
        if let Err(error) = restart_rotation_service_if_active() {
            self.error(format!(
                "Failed to restart active rotation service: {error}"
            ));
        }
    }

    fn save_rotation_interval(&mut self) {
        let value = self.interval_buffer.trim();
        let seconds = match value.parse::<u64>() {
            Ok(0) | Err(_) => {
                self.error("Rotation interval must be a positive number of seconds.");
                return;
            }
            Ok(seconds) => seconds,
        };

        match self.config.set_rotation_interval_secs(seconds) {
            Ok(()) => {
                self.restart_rotation_service_if_active();
                self.refresh_rotation_status();
                self.success(format!(
                    "Rotation interval set to {}.",
                    format_interval(seconds)
                ));
            }
            Err(error) => self.error(format!("Failed to set rotation interval: {error}")),
        }
    }

    fn open_uninstall_dialog(&mut self) {
        self.show_uninstall_dialog = true;
        self.uninstall_confirmed = false;
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            if self.close_top_dialog() {
                return;
            }
        }

        if self.show_download_dialog {
            if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::U)) {
                return;
            }
            if ctx.wants_keyboard_input() {
                if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter)) {
                    let is_edit = self
                        .download_state
                        .as_ref()
                        .map(|state| state.stage == GuiDownloadStage::Edit)
                        .unwrap_or(true);
                    if is_edit && !self.config.wallpaper_paths.is_empty() {
                        self.start_download();
                    }
                }
                return;
            }
            return;
        }

        if ctx.wants_keyboard_input() {
            return;
        }

        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            self.move_selection(1);
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::ArrowUp)) {
            self.move_selection(-1);
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::CloseBracket)) {
            self.cycle_active_section(1);
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::OpenBracket)) {
            self.cycle_active_section(-1);
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter)) {
            self.apply_selected_wallpaper();
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::CTRL, Key::R)) {
            self.trigger_random_wallpaper();
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::R)) {
            self.toggle_rotation_membership();
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::SHIFT, Key::R)) {
            self.show_rotation_dialog = true;
            self.interval_buffer = self.config.rotation_interval_secs.to_string();
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::P)) {
            self.show_paths_dialog = true;
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::U)) {
            self.open_download_dialog();
        }
        if ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Slash)) {
            self.focus_search = true;
        }
        if ctx.input(|input| input.modifiers.shift && input.key_pressed(Key::Slash)) {
            self.show_help_dialog = true;
        }
    }

    fn close_top_dialog(&mut self) -> bool {
        if self.show_uninstall_dialog {
            self.show_uninstall_dialog = false;
            return true;
        }
        if self.show_rotation_dialog {
            self.show_rotation_dialog = false;
            return true;
        }
        if self.show_paths_dialog {
            self.show_paths_dialog = false;
            return true;
        }
        if self.show_download_dialog {
            self.close_download_dialog();
            return true;
        }
        if self.show_help_dialog {
            self.show_help_dialog = false;
            return true;
        }
        if self.show_display_picker {
            self.show_display_picker = false;
            self.pending_wallpaper_path = None;
            return true;
        }
        if self.show_random_dialog {
            self.show_random_dialog = false;
            self.pending_random_candidates.clear();
            return true;
        }
        false
    }

    fn render_toolbar(&mut self, ui: &mut Ui) {
        let palette = self.palette();
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(GuiTypography::rich(
                    GuiTextRole::AppEyebrow,
                    "WALT",
                    palette,
                ));
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    ui.add(
                        egui::Button::new(GuiTypography::rich_color(
                            GuiTextRole::MetaLabel,
                            format!("v{}", env!("CARGO_PKG_VERSION")),
                            palette.text,
                        ))
                        .fill(palette.background)
                        .stroke(Stroke::new(1.0, palette.border))
                        .corner_radius(0.0),
                    );
                    ui.add(
                        egui::Button::new(GuiTypography::rich_color(
                            GuiTextRole::MetaLabel,
                            "beta",
                            palette.accent,
                        ))
                        .fill(palette.background)
                        .stroke(Stroke::new(1.0, palette.border))
                        .corner_radius(0.0),
                    );
                });
            });

            ui.horizontal_wrapped(|ui| {
                ui.label(GuiTypography::rich(
                    GuiTextRole::Body,
                    "Wallpaper browser for Hyprland, with rotation and multi-display control.",
                    palette,
                ));
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !self.visible_wallpaper_paths().is_empty(),
                        GuiChrome::button("Random", GuiTextRole::ActionLabel, palette),
                    )
                    .clicked()
                {
                    self.trigger_random_wallpaper();
                }
                if ui
                    .add(GuiChrome::button(
                        "Download and save",
                        GuiTextRole::ActionLabel,
                        palette,
                    ))
                    .clicked()
                {
                    self.open_download_dialog();
                }
                if ui
                    .add(GuiChrome::button(
                        "Rotation",
                        GuiTextRole::ActionLabel,
                        palette,
                    ))
                    .clicked()
                {
                    self.show_rotation_dialog = true;
                    self.interval_buffer = self.config.rotation_interval_secs.to_string();
                }
                if ui
                    .add(GuiChrome::button(
                        "Paths",
                        GuiTextRole::ActionLabel,
                        palette,
                    ))
                    .clicked()
                {
                    self.show_paths_dialog = true;
                }
                if ui
                    .add(GuiChrome::button("Help", GuiTextRole::ActionLabel, palette))
                    .clicked()
                {
                    self.show_help_dialog = true;
                }
                if ui
                    .add(GuiChrome::button_colored(
                        "Uninstall",
                        GuiTextRole::ActionLabel,
                        palette.danger,
                        palette,
                    ))
                    .clicked()
                {
                    self.open_uninstall_dialog();
                }
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    ui.label(GuiTypography::rich(
                        GuiTextRole::MetaLabel,
                        format!("{} indexed", self.wallpapers.len()),
                        palette,
                    ));
                });
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(GuiTypography::rich(GuiTextRole::MetaLabel, "VIEW", palette));
                for view_mode in [ViewMode::List, ViewMode::Gallery] {
                    let selected = self.view_mode == view_mode;
                    let text = if selected {
                        GuiTypography::rich_color(
                            GuiTextRole::MetaValue,
                            view_mode.label().to_uppercase(),
                            palette.highlight,
                        )
                        .strong()
                    } else {
                        GuiTypography::rich(
                            GuiTextRole::MetaValue,
                            view_mode.label().to_uppercase(),
                            palette,
                        )
                    };
                    if ui
                        .add(
                            egui::Button::new(text)
                                .fill(palette.background)
                                .stroke(Stroke::new(1.0, palette.border))
                                .corner_radius(0.0),
                        )
                        .clicked()
                    {
                        self.set_view_mode(view_mode);
                    }
                }
            });
        });
    }

    fn render_section_tabs(&mut self, ui: &mut Ui) {
        let palette = self.palette();
        ui.horizontal_wrapped(|ui| {
            for section in SectionKind::ALL {
                let selected = self.active_section == section;
                let label = if section == SectionKind::Rotation {
                    let badge = self
                        .rotation_service_state
                        .as_ref()
                        .map(rotation_service_badge)
                        .unwrap_or("unknown");
                    format!("{} [{badge}]", section.label().to_uppercase())
                } else {
                    section.label().to_uppercase()
                };
                let text = if selected {
                    GuiTypography::rich_color(GuiTextRole::MetaValue, label, palette.highlight)
                        .strong()
                } else {
                    GuiTypography::rich(GuiTextRole::MetaValue, label, palette)
                };
                if ui
                    .add(
                        egui::Button::new(text)
                            .fill(palette.background)
                            .stroke(Stroke::new(1.0, palette.border))
                            .corner_radius(0.0),
                    )
                    .clicked()
                {
                    self.active_section = section;
                    self.ensure_section_selection();
                    self.request_preview_load();
                }
            }
        });
    }

    fn empty_section_message(&self) -> &'static str {
        if self.filter_query(self.active_section).is_empty() {
            match self.active_section {
                SectionKind::All => "No wallpapers indexed yet.",
                SectionKind::Rotation => "Rotation list is empty.",
            }
        } else {
            "No matches for the current filter."
        }
    }

    fn render_section_browser(&mut self, ui: &mut Ui) -> bool {
        let palette = self.palette();
        self.render_section_tabs(ui);
        subtle_rule(ui, palette);
        let changed = match self.active_section {
            SectionKind::All => {
                render_search_bar(ui, &mut self.all_filter, &mut self.focus_search, palette)
            }
            SectionKind::Rotation => render_search_bar(
                ui,
                &mut self.rotation_filter,
                &mut self.focus_search,
                palette,
            ),
        };
        if changed {
            self.ensure_section_selection();
            self.request_preview_load();
        }
        if self.active_section == SectionKind::Rotation {
            ui.label(GuiTypography::rich(
                GuiTextRole::MetaLabel,
                format!("INTERVAL {}s", self.config.rotation_interval_secs),
                palette,
            ));
        }
        subtle_rule(ui, palette);

        if self.section_is_informational(self.active_section) {
            ui.add_space(24.0);
            ui.label(GuiTypography::rich(
                GuiTextRole::Hero,
                "Rotating all wallpapers",
                palette,
            ));
            ui.label(GuiTypography::rich(
                GuiTextRole::BodyMuted,
                "Manual rotation list is preserved while rotate-all mode is enabled.",
                palette,
            ));
            return true;
        }

        false
    }

    fn render_sidebar(&mut self, ui: &mut Ui) {
        let palette = self.palette();
        section_heading(ui, "LIBRARY", palette);
        if self.render_section_browser(ui) {
            return;
        }
        let indices = self.section_indices(self.active_section);
        if indices.is_empty() {
            ui.add_space(24.0);
            ui.label(GuiTypography::rich(
                GuiTextRole::BodyMuted,
                self.empty_section_message(),
                palette,
            ));
            return;
        }

        let row_height = 20.0;
        ScrollArea::vertical().auto_shrink([false; 2]).show_rows(
            ui,
            row_height,
            indices.len(),
            |ui, row_range| {
                for row in row_range {
                    let Some(index) = indices.get(row).copied() else {
                        continue;
                    };
                    let Some(wallpaper) = self.wallpapers.get(index) else {
                        continue;
                    };
                    let selected = self.selected_index(self.active_section) == Some(row);
                    let badges = wallpaper_badges(
                        self.active_wallpaper_paths.contains(&wallpaper.path),
                        self.rotation_paths.contains(&wallpaper.path),
                    );
                    let row_id = (
                        "wallpaper-row",
                        self.active_section.key(),
                        row,
                        &wallpaper.path,
                    );
                    let (response, _) = interactive_row(ui, row_id, row_height, |ui, _, _| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(GuiTypography::rich_color(
                            GuiTextRole::ListItem,
                            &wallpaper.name,
                            if selected {
                                palette.highlight
                            } else {
                                palette.text
                            },
                        ));
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            if !badges.is_empty() {
                                ui.label(GuiTypography::rich(
                                    GuiTextRole::ListBadge,
                                    badges,
                                    palette,
                                ));
                            }
                        });
                    });

                    if response.clicked() {
                        self.set_selected_index(self.active_section, Some(row));
                        self.request_preview_load();
                    }
                    if response.double_clicked() {
                        self.set_selected_index(self.active_section, Some(row));
                        self.request_preview_load();
                        self.apply_selected_wallpaper();
                    }

                    subtle_rule_compact(ui, palette);
                }
            },
        );
    }

    fn render_gallery(&mut self, ui: &mut Ui) {
        let palette = self.palette();
        section_heading(ui, "GALLERY", palette);
        if self.render_section_browser(ui) {
            return;
        }

        let indices = self.section_indices(self.active_section);
        if indices.is_empty() {
            ui.add_space(24.0);
            ui.label(GuiTypography::rich(
                GuiTextRole::BodyMuted,
                self.empty_section_message(),
                palette,
            ));
            return;
        }

        let spacing = 12.0;
        let min_card_width = 220.0;
        let total_width = ui.available_width().max(min_card_width);
        let columns = ((total_width + spacing) / (min_card_width + spacing))
            .floor()
            .max(1.0) as usize;
        let card_width =
            ((total_width - spacing * (columns.saturating_sub(1)) as f32) / columns as f32)
                .max(min_card_width);
        let image_height = (card_width * 9.0 / 16.0).max(124.0);
        let card_height = image_height + 56.0;

        ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(spacing, spacing);
            ui.horizontal_wrapped(|ui| {
                for (row, index) in indices.iter().copied().enumerate() {
                    let Some(wallpaper) = self.wallpapers.get(index).cloned() else {
                        continue;
                    };
                    let selected = self.selected_index(self.active_section) == Some(row);
                    let badges = wallpaper_badges(
                        self.active_wallpaper_paths.contains(&wallpaper.path),
                        self.rotation_paths.contains(&wallpaper.path),
                    );
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(card_width, card_height),
                        egui::Sense::click(),
                    );
                    ui.painter().rect(
                        rect,
                        0.0,
                        if selected {
                            palette.surface_alt
                        } else {
                            palette.surface
                        },
                        Stroke::new(
                            1.0,
                            if selected {
                                palette.highlight
                            } else {
                                palette.border
                            },
                        ),
                        egui::StrokeKind::Outside,
                    );

                    let inner_rect = rect.shrink2(egui::vec2(10.0, 10.0));
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(inner_rect)
                            .layout(egui::Layout::top_down(Align::Min)),
                    );
                    let image_size = egui::vec2(inner_rect.width(), image_height.min(inner_rect.height()));
                    if let Some(texture) = self.gallery_texture(ui.ctx(), &wallpaper.path) {
                        let size = fit_size(texture.size_vec2(), image_size);
                        ui.put(
                            egui::Rect::from_min_size(
                                egui::pos2(
                                    inner_rect.center().x - size.x * 0.5,
                                    inner_rect.top() + (image_size.y - size.y) * 0.5,
                                ),
                                size,
                            ),
                            egui::Image::new(&texture).fit_to_exact_size(size),
                        );
                    } else {
                        child.allocate_ui_with_layout(
                            image_size,
                            egui::Layout::centered_and_justified(egui::Direction::TopDown),
                            |ui| {
                                ui.label(GuiTypography::rich(
                                    GuiTextRole::BodyMuted,
                                    "Preview unavailable",
                                    palette,
                                ));
                            },
                        );
                    }

                    child.add_space(image_height + 6.0);
                    child.label(
                        GuiTypography::rich_color(
                            GuiTextRole::ListItem,
                            wallpaper.name,
                            if selected {
                                palette.highlight
                            } else {
                                palette.text
                            },
                        )
                        .strong(),
                    );
                    if !badges.is_empty() {
                        child.label(GuiTypography::rich(
                            GuiTextRole::ListBadge,
                            badges,
                            palette,
                        ));
                    }

                    if response.clicked() {
                        self.set_selected_index(self.active_section, Some(row));
                        self.request_preview_load();
                    }
                    if response.double_clicked() {
                        self.set_selected_index(self.active_section, Some(row));
                        self.request_preview_load();
                        self.apply_selected_wallpaper();
                    }
                }
            });
        });
    }

    fn render_preview_panel(&mut self, ui: &mut Ui) {
        let palette = self.palette();
        ui.vertical(|ui| {
            if self.config.is_empty() {
                section_heading(ui, "OVERVIEW", palette);
                ui.add_space(18.0);
                ui.label(GuiTypography::rich(
                    GuiTextRole::Hero,
                    "Start by adding a wallpaper folder",
                    palette,
                ));
                ui.label(GuiTypography::rich(
                    GuiTextRole::BodyMuted,
                    "Use the Paths button to pick a folder or type one manually. Walt will index it in the background.",
                    palette,
                ));
                if ui
                    .add(GuiChrome::button("Open Paths", GuiTextRole::ActionLabel, palette))
                    .clicked()
                {
                    self.show_paths_dialog = true;
                }
                return;
            }

            let Some(selected) = self.current_selected_wallpaper().cloned() else {
                section_heading(ui, "OVERVIEW", palette);
                ui.add_space(18.0);
                ui.label(GuiTypography::rich(
                    GuiTextRole::Hero,
                    "No wallpaper selected",
                    palette,
                ));
                ui.label(GuiTypography::rich(
                    GuiTextRole::BodyMuted,
                    "Choose one from the library to preview it.",
                    palette,
                ));
                return;
            };

            section_heading(ui, "PREVIEW", palette);
            subtle_rule(ui, palette);

            let texture = self.current_preview_texture();
            let frame_height = ui.available_height().max(220.0);
            egui::Frame::new()
                .fill(palette.surface)
                .stroke(Stroke::new(1.0, palette.border))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), frame_height),
                        egui::Sense::hover(),
                    );
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect)
                            .layout(egui::Layout::top_down(Align::Center)),
                    );
                    child.with_layout(
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            if let Some(texture) = texture {
                                let size = fit_size(texture.size_vec2(), rect.size());
                                ui.add(egui::Image::new(&texture).fit_to_exact_size(size));
                            } else {
                                ui.vertical_centered(|ui| {
                                    ui.spinner();
                                    ui.label(GuiTypography::rich(
                                        GuiTextRole::MetaLabel,
                                        format!("Loading preview for {}", selected.name),
                                        palette,
                                    ));
                                });
                            }
                        },
                    );
                });
        });
    }

    fn current_preview_texture(&mut self) -> Option<TextureHandle> {
        let desired = self.desired_preview_key.as_ref()?;
        let texture = self.texture_cache.get_cloned(desired);
        if texture.is_some() {
            self.current_preview_key = Some(desired.clone());
        }
        texture
    }

    fn render_metadata(&mut self, ui: &mut Ui) {
        let palette = self.palette();
        let Some(wallpaper) = self.current_selected_wallpaper().cloned() else {
            ui.label(GuiTypography::rich(
                GuiTextRole::BodyMuted,
                "No wallpaper selected",
                palette,
            ));
            return;
        };

        section_heading(ui, "DETAILS", palette);
        info_row(
            ui,
            "NAME",
            &wallpaper.name,
            palette,
            Some(wallpaper_badges(
                self.active_wallpaper_paths.contains(&wallpaper.path),
                self.rotation_paths.contains(&wallpaper.path),
            )),
        );
        info_row(
            ui,
            "FILE",
            &wallpaper.path.display().to_string(),
            palette,
            None,
        );
        info_row(
            ui,
            "DIR",
            &wallpaper.directory.display().to_string(),
            palette,
            None,
        );
        info_row(
            ui,
            "RESOLUTION",
            &format_resolution(wallpaper.width, wallpaper.height),
            palette,
            Some(format_file_size(wallpaper.file_size)),
        );
        info_row(
            ui,
            "MODIFIED",
            &format_timestamp(wallpaper.modified_unix_secs),
            palette,
            Some(wallpaper.extension.to_uppercase()),
        );

        ui.add_space(2.0);
        section_heading(ui, "ACTIONS", palette);
        let rotation_label = if self.rotation_paths.contains(&wallpaper.path) {
            "Remove from rotation"
        } else {
            "Add to rotation"
        };
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(GuiChrome::button(
                    "Apply",
                    GuiTextRole::ActionLabel,
                    palette,
                ))
                .clicked()
            {
                self.apply_selected_wallpaper();
            }
            if ui
                .add(GuiChrome::button(
                    "All Displays",
                    GuiTextRole::ActionLabel,
                    palette,
                ))
                .clicked()
            {
                if let Err(error) = self.apply_wallpaper_to_all_displays(&wallpaper.path) {
                    self.error(format!("Failed to set wallpaper: {error}"));
                }
            }
            if ui
                .add(GuiChrome::button(
                    rotation_label,
                    GuiTextRole::ActionLabel,
                    palette,
                ))
                .clicked()
            {
                self.toggle_rotation_membership();
            }
        });
    }

    fn render_display_picker_window(&mut self, ctx: &egui::Context) {
        let palette = self.palette();
        self.show_display_picker = show_popup_shell(
            ctx,
            "display-picker",
            "Apply Wallpaper To",
            palette,
            None,
            |ui| {
                ui.label(GuiTypography::rich(
                    GuiTextRole::PopupBody,
                    "Choose one display or use All displays.",
                    palette,
                ));
                GuiChrome::rule(ui, palette, 8.0);

                let mut close_requested = false;
                let mut activate_index = None;
                for (index, target) in self.display_targets.iter().enumerate() {
                    if popup_choice_row(
                        ui,
                        ("display-target", index),
                        self.display_target_selection == Some(index),
                        target.label(),
                        palette,
                    )
                    .clicked()
                    {
                        activate_index = Some(index);
                    }
                }
                if let Some(index) = activate_index {
                    self.display_target_selection = Some(index);
                    self.apply_display_selection();
                    close_requested = true;
                }
                ui.add_space(8.0);
                if ui
                    .add(GuiChrome::button(
                        "Cancel",
                        GuiTextRole::ActionLabel,
                        palette,
                    ))
                    .clicked()
                {
                    self.pending_wallpaper_path = None;
                    close_requested = true;
                }
                close_requested
            },
        );
    }

    fn render_random_window(&mut self, ctx: &egui::Context) {
        let palette = self.palette();
        self.show_random_dialog = show_popup_shell(
            ctx,
            "random-options",
            "Random Options",
            palette,
            None,
            |ui| {
                ui.label(GuiTypography::rich(
                    GuiTextRole::PopupBody,
                    "Choose a random wallpaper strategy.",
                    palette,
                ));
                GuiChrome::rule(ui, palette, 8.0);

                let mut close_requested = false;
                let mut activate_index = None;
                for (index, action) in self.random_targets.iter().enumerate() {
                    if popup_choice_row(
                        ui,
                        ("random-target", index),
                        self.random_target_selection == Some(index),
                        &action.label(),
                        palette,
                    )
                    .clicked()
                    {
                        activate_index = Some(index);
                    }
                }
                if let Some(index) = activate_index {
                    self.random_target_selection = Some(index);
                    self.apply_random_selection();
                    close_requested = true;
                }
                ui.add_space(8.0);
                if ui
                    .add(GuiChrome::button(
                        "Cancel",
                        GuiTextRole::ActionLabel,
                        palette,
                    ))
                    .clicked()
                {
                    self.pending_random_candidates.clear();
                    close_requested = true;
                }
                close_requested
            },
        );
    }

    fn render_rotation_window(&mut self, ctx: &egui::Context) {
        let palette = self.palette();
        self.show_rotation_dialog = show_popup_shell(
            ctx,
            "rotation-service",
            "Rotation Service",
            palette,
            Some(620.0),
            |ui| {
                ui.label(GuiTypography::rich(
                    GuiTextRole::PopupBody,
                    &self.rotation_status_text,
                    palette,
                ));
                GuiChrome::rule(ui, palette, 8.0);

                ui.horizontal_wrapped(|ui| {
                    let installed = self
                        .rotation_service_state
                        .as_ref()
                        .map(|status| status.installed)
                        .unwrap_or(false);
                    let active = self
                        .rotation_service_state
                        .as_ref()
                        .map(|status| status.active == "active")
                        .unwrap_or(false);

                    if !installed {
                        if ui
                            .add(GuiChrome::button(
                                "Install Service",
                                GuiTextRole::ActionLabel,
                                palette,
                            ))
                            .clicked()
                        {
                            self.run_rotation_service_action(
                                install_rotation_service,
                                "Installed and started Walt rotation service.",
                            );
                        }
                    } else {
                        if ui
                            .add(GuiChrome::button(
                                "Uninstall Service",
                                GuiTextRole::ActionLabel,
                                palette,
                            ))
                            .clicked()
                        {
                            self.run_rotation_service_action(
                                uninstall_rotation_service,
                                "Removed Walt rotation service.",
                            );
                        }
                        if active {
                            if ui
                                .add(GuiChrome::button(
                                    "Disable Service",
                                    GuiTextRole::ActionLabel,
                                    palette,
                                ))
                                .clicked()
                            {
                                self.run_rotation_service_action(
                                    disable_rotation_service,
                                    "Disabled Walt rotation service.",
                                );
                            }
                        } else if ui
                            .add(GuiChrome::button(
                                "Enable Service",
                                GuiTextRole::ActionLabel,
                                palette,
                            ))
                            .clicked()
                        {
                            self.run_rotation_service_action(
                                enable_rotation_service,
                                "Enabled and started Walt rotation service.",
                            );
                        }
                    }
                });

                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    let scope_label = if self.config.uses_all_wallpapers_for_rotation() {
                        "Use Selected Wallpapers"
                    } else {
                        "Rotate All Wallpapers"
                    };
                    if ui
                        .add(GuiChrome::button(
                            scope_label,
                            GuiTextRole::ActionLabel,
                            palette,
                        ))
                        .clicked()
                    {
                        self.toggle_rotate_all_wallpapers();
                    }

                    let display_label = if self
                        .config
                        .uses_same_wallpaper_on_all_displays_for_rotation()
                    {
                        "Use Different Wallpapers Per Display"
                    } else {
                        "Use Same Wallpaper On All Displays"
                    };
                    if ui
                        .add(GuiChrome::button(
                            display_label,
                            GuiTextRole::ActionLabel,
                            palette,
                        ))
                        .clicked()
                    {
                        self.toggle_rotation_display_mode();
                    }
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(GuiTypography::rich(
                        GuiTextRole::MetaLabel,
                        "Interval (seconds)",
                        palette,
                    ));
                    ui.add(
                        TextEdit::singleline(&mut self.interval_buffer)
                            .font(GuiTypography::font_id(GuiTextRole::MetaValue))
                            .desired_width(120.0),
                    );
                    if ui
                        .add(GuiChrome::button(
                            "Save Interval",
                            GuiTextRole::ActionLabel,
                            palette,
                        ))
                        .clicked()
                    {
                        self.save_rotation_interval();
                    }
                });

                ui.add_space(8.0);
                ui.label(GuiTypography::rich(
                    GuiTextRole::MetaLabel,
                    "Changes to scope, display mode, or interval restart the service only if it is already active.",
                    palette,
                ));
                false
            },
        );
    }

    fn render_paths_window(&mut self, ctx: &egui::Context) {
        let palette = self.palette();
        self.show_paths_dialog = show_popup_shell(
            ctx,
            "wallpaper-paths",
            "Wallpaper Paths",
            palette,
            Some(700.0),
            |ui| {
                ui.label(GuiTypography::rich(
                    GuiTextRole::PopupBody,
                    "Add a wallpaper folder with the system picker or by typing a path.",
                    palette,
                ));
                ui.horizontal(|ui| {
                    if ui
                        .add(GuiChrome::button(
                            "Add Folder",
                            GuiTextRole::ActionLabel,
                            palette,
                        ))
                        .clicked()
                    {
                        match rfd::FileDialog::new().pick_folder() {
                            Some(path) => self.add_path(path),
                            None => self
                                .info("Folder picker closed. You can still paste a path manually."),
                        }
                    }
                    ui.add(
                        TextEdit::singleline(&mut self.manual_path_input)
                            .font(GuiTypography::font_id(GuiTextRole::MetaValue))
                            .desired_width(360.0),
                    );
                    if ui
                        .add(GuiChrome::button(
                            "Add Typed Path",
                            GuiTextRole::ActionLabel,
                            palette,
                        ))
                        .clicked()
                    {
                        self.add_path(PathBuf::from(self.manual_path_input.trim()));
                    }
                });
                GuiChrome::rule(ui, palette, 8.0);
                if self.config.wallpaper_paths.is_empty() {
                    ui.label(GuiTypography::rich(
                        GuiTextRole::PopupBody,
                        "No wallpaper paths configured.",
                        palette,
                    ));
                } else {
                    let paths = self.config.wallpaper_paths.clone();
                    ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        for path in paths {
                            ui.horizontal(|ui| {
                                ui.label(GuiTypography::rich(
                                    GuiTextRole::MetaValue,
                                    path.display().to_string(),
                                    palette,
                                ));
                                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .add(GuiChrome::button(
                                            "Remove",
                                            GuiTextRole::ActionLabel,
                                            palette,
                                        ))
                                        .clicked()
                                    {
                                        self.remove_path(&path);
                                    }
                                });
                            });
                        }
                    });
                }
                ui.add_space(8.0);
                ui.add(GuiChrome::button(
                    "Close",
                    GuiTextRole::ActionLabel,
                    palette,
                ))
                .clicked()
            },
        );
    }

    fn render_download_window(&mut self, ctx: &egui::Context) {
        let palette = self.palette();
        let stage = self
            .download_state
            .as_ref()
            .map(|state| state.stage.clone())
            .unwrap_or(GuiDownloadStage::Edit);
        self.normalize_download_path_selection();

        self.show_download_dialog = show_popup_shell(
            ctx,
            "download-and-save",
            "Download and Save",
            palette,
            Some(720.0),
            |ui| {
                let mut close_requested = false;

                match stage {
                    GuiDownloadStage::Edit => {
                        if self.config.wallpaper_paths.is_empty() {
                            ui.label(GuiTypography::rich(
                                GuiTextRole::PopupBody,
                                "No wallpaper paths configured.",
                                palette,
                            ));
                            ui.label(GuiTypography::rich(
                                GuiTextRole::MetaLabel,
                                "Open Paths to add a wallpaper folder, then come back here.",
                                palette,
                            ));
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .add(GuiChrome::button(
                                        "Open Paths",
                                        GuiTextRole::ActionLabel,
                                        palette,
                                    ))
                                    .clicked()
                                {
                                    self.show_paths_dialog = true;
                                }
                                if ui
                                    .add(GuiChrome::button(
                                        "Close",
                                        GuiTextRole::ActionLabel,
                                        palette,
                                    ))
                                    .clicked()
                                {
                                    close_requested = true;
                                }
                            });
                            return close_requested;
                        }

                        ui.label(GuiTypography::rich(
                            GuiTextRole::PopupBody,
                            "Enter a direct image URL and choose one configured wallpaper path.",
                            palette,
                        ));
                        ui.add_space(6.0);
                        let response = ui.add(
                            TextEdit::singleline(&mut self.download_url_input)
                                .font(GuiTypography::font_id(GuiTextRole::MetaValue))
                                .desired_width(finite_ui_width(ui))
                                .hint_text("https://example.com/wallpaper.jpg"),
                        );
                        if self.focus_download_url_input {
                            response.request_focus();
                            self.focus_download_url_input = false;
                        }

                        if let Some(error) = &self.download_error {
                            ui.add_space(6.0);
                            ui.label(GuiTypography::rich_color(
                                GuiTextRole::PopupBody,
                                error,
                                palette.danger,
                            ));
                        }

                        GuiChrome::rule(ui, palette, 8.0);
                        ui.label(GuiTypography::rich(
                            GuiTextRole::MetaLabel,
                            "Save into wallpaper path",
                            palette,
                        ));
                        ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            for (index, path) in self.config.wallpaper_paths.iter().enumerate() {
                                if popup_choice_row(
                                    ui,
                                    ("download-path", index),
                                    self.download_path_selection == Some(index),
                                    &path.display().to_string(),
                                    palette,
                                )
                                .clicked()
                                {
                                    self.download_path_selection = Some(index);
                                    self.last_download_path_selection = Some(index);
                                }
                            }
                        });

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(GuiChrome::button(
                                    "Download and Save",
                                    GuiTextRole::ActionLabel,
                                    palette,
                                ))
                                .clicked()
                            {
                                self.start_download();
                            }
                            if ui
                                .add(GuiChrome::button(
                                    "Cancel",
                                    GuiTextRole::ActionLabel,
                                    palette,
                                ))
                                .clicked()
                            {
                                close_requested = true;
                            }
                        });
                    }
                    GuiDownloadStage::Downloading => {
                        let state = self.download_state.clone().unwrap_or(GuiDownloadState {
                            stage: GuiDownloadStage::Downloading,
                            final_path: None,
                            downloaded_bytes: 0,
                            total_bytes: None,
                        });
                        if let Some(path) = state.final_path {
                            ui.label(GuiTypography::rich(
                                GuiTextRole::PopupBody,
                                format!("Saving as {}", path.display()),
                                palette,
                            ));
                            if let Some(parent) = path.parent() {
                                ui.label(GuiTypography::rich(
                                    GuiTextRole::MetaLabel,
                                    format!("Destination: {}", parent.display()),
                                    palette,
                                ));
                            }
                        } else {
                            ui.label(GuiTypography::rich(
                                GuiTextRole::PopupBody,
                                "Preparing download...",
                                palette,
                            ));
                        }

                        ui.add_space(8.0);
                        if let Some(total_bytes) = state.total_bytes {
                            let progress = if total_bytes == 0 {
                                0.0
                            } else {
                                state.downloaded_bytes as f32 / total_bytes as f32
                            };
                            ui.add(
                                egui::ProgressBar::new(progress.clamp(0.0, 1.0))
                                    .desired_width(finite_ui_width(ui))
                                    .text(format!(
                                        "{} / {}",
                                        format_file_size(state.downloaded_bytes),
                                        format_file_size(total_bytes)
                                    )),
                            );
                        } else {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(GuiTypography::rich(
                                    GuiTextRole::PopupBody,
                                    format!(
                                        "Downloaded {}",
                                        format_file_size(state.downloaded_bytes)
                                    ),
                                    palette,
                                ));
                            });
                        }

                        ui.add_space(8.0);
                        if ui
                            .add(GuiChrome::button(
                                "Cancel Download",
                                GuiTextRole::ActionLabel,
                                palette,
                            ))
                            .clicked()
                        {
                            self.close_download_dialog();
                        }
                    }
                    GuiDownloadStage::Failed(message) => {
                        ui.label(GuiTypography::rich_color(
                            GuiTextRole::PopupBody,
                            message,
                            palette.danger,
                        ));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(GuiChrome::button(
                                    "Retry",
                                    GuiTextRole::ActionLabel,
                                    palette,
                                ))
                                .clicked()
                            {
                                self.reset_download_dialog_for_retry();
                            }
                            if ui
                                .add(GuiChrome::button(
                                    "Close",
                                    GuiTextRole::ActionLabel,
                                    palette,
                                ))
                                .clicked()
                            {
                                close_requested = true;
                            }
                        });
                    }
                    GuiDownloadStage::Cancelled => {
                        ui.label(GuiTypography::rich(
                            GuiTextRole::PopupBody,
                            "Download cancelled.",
                            palette,
                        ));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(GuiChrome::button(
                                    "Retry",
                                    GuiTextRole::ActionLabel,
                                    palette,
                                ))
                                .clicked()
                            {
                                self.reset_download_dialog_for_retry();
                            }
                            if ui
                                .add(GuiChrome::button(
                                    "Close",
                                    GuiTextRole::ActionLabel,
                                    palette,
                                ))
                                .clicked()
                            {
                                close_requested = true;
                            }
                        });
                    }
                }

                close_requested
            },
        );
    }

    fn render_help_window(&mut self, ctx: &egui::Context) {
        let palette = self.palette();
        self.show_help_dialog =
            show_popup_shell(ctx, "shortcuts", "Shortcuts", palette, None, |ui| {
                let shortcuts = [
                    ("Arrow Up / Arrow Down", "Move selection"),
                    ("[ / ]", "Switch between All and Rotation tabs"),
                    ("Enter", "Apply selected wallpaper"),
                    ("/", "Focus the search box"),
                    ("Ctrl+R", "Open random wallpaper flow"),
                    ("u", "Open download and save flow"),
                    ("r", "Toggle selected wallpaper in rotation list"),
                    ("R", "Open rotation service dialog"),
                    ("p", "Open wallpaper paths"),
                    ("?", "Open this help"),
                    ("Esc", "Close the current dialog"),
                ];

                for (shortcut, action) in shortcuts {
                    ui.horizontal(|ui| {
                        ui.label(GuiTypography::rich_color(
                            GuiTextRole::MetaValue,
                            shortcut,
                            palette.highlight,
                        ));
                        ui.label(GuiTypography::rich(GuiTextRole::PopupBody, action, palette));
                    });
                }
                ui.add_space(8.0);
                ui.add(GuiChrome::button(
                    "Close",
                    GuiTextRole::ActionLabel,
                    palette,
                ))
                .clicked()
            });
    }

    fn render_uninstall_window(&mut self, ctx: &egui::Context) {
        let palette = self.palette();
        self.show_uninstall_dialog = show_popup_shell(
            ctx,
            "uninstall-walt",
            "Uninstall Walt",
            palette,
            Some(700.0),
            |ui| {
                if let Some(summary) = &self.uninstall_summary {
                    ui.label(
                        GuiTypography::rich_color(
                            GuiTextRole::PopupBody,
                            "Walt uninstall complete.",
                            palette.danger,
                        )
                        .strong(),
                    );
                    GuiChrome::rule(ui, palette, 8.0);
                    ui.label(GuiTypography::rich(
                        GuiTextRole::PopupBody,
                        summary,
                        palette,
                    ));
                    ui.add_space(8.0);
                    ui.label(GuiTypography::rich(
                        GuiTextRole::MetaLabel,
                        "The app will close automatically.",
                        palette,
                    ));
                    return false;
                }

                ui.label(
                    GuiTypography::rich_color(
                        GuiTextRole::PopupBody,
                        "This will remove Walt from this system.",
                        palette.danger,
                    )
                    .strong(),
                );
                GuiChrome::rule(ui, palette, 8.0);
                match uninstall_paths() {
                    Ok(paths) => {
                        ui.label(GuiTypography::rich(
                            GuiTextRole::MetaValue,
                            format!("rotation service: {}", paths.service_file.display()),
                            palette,
                        ));
                        ui.label(GuiTypography::rich(
                            GuiTextRole::MetaValue,
                            format!("config: {}", paths.config_dir.display()),
                            palette,
                        ));
                        ui.label(GuiTypography::rich(
                            GuiTextRole::MetaValue,
                            format!("cache: {}", paths.cache_dir.display()),
                            palette,
                        ));
                        ui.label(GuiTypography::rich(
                            GuiTextRole::MetaValue,
                            format!("binary: {}", paths.binary_path.display()),
                            palette,
                        ));
                    }
                    Err(error) => {
                        ui.label(GuiTypography::rich_color(
                            GuiTextRole::PopupBody,
                            format!("Failed to inspect uninstall paths: {error}"),
                            palette.danger,
                        ));
                    }
                }
                ui.add_space(8.0);
                ui.checkbox(
                    &mut self.uninstall_confirmed,
                    "I understand this removes the Walt installation.",
                );
                let mut close_requested = false;
                ui.horizontal(|ui| {
                    if ui
                        .add(GuiChrome::button(
                            "Cancel",
                            GuiTextRole::ActionLabel,
                            palette,
                        ))
                        .clicked()
                    {
                        close_requested = true;
                    }
                    if ui
                        .add_enabled(
                            self.uninstall_confirmed,
                            GuiChrome::button_colored(
                                "Remove Walt",
                                GuiTextRole::ActionLabel,
                                palette.danger,
                                palette,
                            ),
                        )
                        .clicked()
                    {
                        match uninstall_walt() {
                            Ok(report) => {
                                self.uninstall_summary = Some(report.summary());
                                self.uninstall_close_deadline =
                                    Some(Instant::now() + Duration::from_secs(2));
                                self.success("Walt uninstall complete.");
                            }
                            Err(error) => self.error(format!("Failed to uninstall Walt: {error}")),
                        }
                    }
                });
                close_requested
            },
        );
    }

    fn render_toasts(&self, ctx: &egui::Context) {
        let palette = self.palette();
        for (index, toast) in self.toasts.iter().enumerate() {
            let background = match toast.kind {
                ToastKind::Info => palette.surface_alt,
                ToastKind::Success => Color32::from_rgb(
                    palette.success.r() / 4,
                    palette.success.g() / 4,
                    palette.success.b() / 4,
                ),
                ToastKind::Error => Color32::from_rgb(
                    palette.danger.r() / 4,
                    palette.danger.g() / 4,
                    palette.danger.b() / 4,
                ),
            };
            egui::Area::new(Id::new(("toast", toast.id)))
                .anchor(
                    Align2::RIGHT_TOP,
                    Vec2::new(-18.0, 18.0 + index as f32 * 54.0),
                )
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(background)
                        .stroke(Stroke::new(1.0, palette.border))
                        .corner_radius(8.0)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label(GuiTypography::rich(
                                GuiTextRole::Toast,
                                &toast.message,
                                palette,
                            ));
                        });
                });
        }
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_index_updates(ctx);
        self.drain_preview_updates(ctx);
        self.drain_download_updates(ctx);
        self.drain_backend_state_updates(ctx);
        self.ensure_selected_preview_is_available();
        self.expire_toasts();
        self.handle_shortcuts(ctx);
        self.maybe_request_backend_state_refresh();

        let now = Instant::now();
        if let Some(deadline) = self.uninstall_close_deadline {
            if now >= deadline {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }

        let mut next_repaint_after = None;
        if let Some(deadline) = self.uninstall_close_deadline {
            next_repaint_after = Some(deadline.saturating_duration_since(now));
        }

        let backend_poll_after = if self.backend_state_request_in_flight {
            Duration::from_millis(250)
        } else {
            self.next_backend_state_refresh_at
                .saturating_duration_since(now)
        };
        next_repaint_after = Some(match next_repaint_after {
            Some(current) => current.min(backend_poll_after),
            None => backend_poll_after,
        });

        if !self.toasts.is_empty() {
            let toast_after = Duration::from_millis(250);
            next_repaint_after = Some(match next_repaint_after {
                Some(current) => current.min(toast_after),
                None => toast_after,
            });
        }

        if let Some(duration) = next_repaint_after {
            ctx.request_repaint_after(duration);
        }

        egui::TopBottomPanel::top("toolbar")
            .show_separator_line(false)
            .frame(GuiChrome::panel_frame(self.palette(), 18))
            .show(ctx, |ui| self.render_toolbar(ui));

        if self.view_mode == ViewMode::List {
            egui::SidePanel::left("sidebar")
                .default_width(430.0)
                .resizable(true)
                .show_separator_line(false)
                .frame(GuiChrome::panel_frame(self.palette(), 18))
                .show(ctx, |ui| self.render_sidebar(ui));

            egui::CentralPanel::default()
                .frame(GuiChrome::panel_frame(self.palette(), 18))
                .show(ctx, |ui| {
                    egui::TopBottomPanel::bottom("metadata")
                        .show_separator_line(false)
                        .min_height(212.0)
                        .show_inside(ui, |ui| self.render_metadata(ui));

                    self.render_preview_panel(ui);
                });
        } else {
            egui::CentralPanel::default()
                .frame(GuiChrome::panel_frame(self.palette(), 18))
                .show(ctx, |ui| {
                    egui::TopBottomPanel::bottom("metadata")
                        .show_separator_line(false)
                        .min_height(212.0)
                        .show_inside(ui, |ui| self.render_metadata(ui));

                    self.render_gallery(ui);
                });
        }

        if self.show_display_picker {
            self.render_display_picker_window(ctx);
        }
        if self.show_random_dialog {
            self.render_random_window(ctx);
        }
        if self.show_rotation_dialog {
            self.render_rotation_window(ctx);
        }
        if self.show_download_dialog {
            self.render_download_window(ctx);
        }
        if self.show_paths_dialog {
            self.render_paths_window(ctx);
        }
        if self.show_help_dialog {
            self.render_help_window(ctx);
        }
        if self.show_uninstall_dialog {
            self.render_uninstall_window(ctx);
        }
        self.render_toasts(ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }
}

fn spawn_index_worker(index: WallpaperIndex) -> (Sender<IndexRequest>, Receiver<IndexResponse>) {
    let (request_tx, request_rx) = mpsc::channel::<IndexRequest>();
    let (response_tx, response_rx) = mpsc::channel::<IndexResponse>();

    std::thread::spawn(move || {
        while let Ok(mut request) = request_rx.recv() {
            while let Ok(next_request) = request_rx.try_recv() {
                request = next_request;
            }

            let wallpapers = index.refresh(&request.wallpaper_paths);
            let _ = response_tx.send(IndexResponse {
                request_id: request.request_id,
                wallpapers,
            });
        }
    });

    (request_tx, response_rx)
}

fn spawn_download_worker() -> (
    Sender<DownloadWorkerRequest>,
    Receiver<DownloadWorkerResponse>,
) {
    let (request_tx, request_rx) = mpsc::channel::<DownloadWorkerRequest>();
    let (response_tx, response_rx) = mpsc::channel::<DownloadWorkerResponse>();

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build download runtime");
        while let Ok(request) = request_rx.recv() {
            let progress_tx = progress_tx_to_gui(request.request_id, response_tx.clone());
            let _ = runtime.block_on(download_image_async(
                request.request,
                &progress_tx,
                request.cancel,
            ));
        }
    });

    (request_tx, response_rx)
}

fn progress_tx_to_gui(
    request_id: u64,
    response_tx: Sender<DownloadWorkerResponse>,
) -> Sender<DownloadProgressEvent> {
    let (progress_tx, progress_rx) = mpsc::channel::<DownloadProgressEvent>();

    std::thread::spawn(move || {
        while let Ok(event) = progress_rx.recv() {
            let _ = response_tx.send(DownloadWorkerResponse { request_id, event });
        }
    });

    progress_tx
}

fn spawn_backend_state_worker() -> (Sender<BackendStateRequest>, Receiver<BackendStateResponse>) {
    let (request_tx, request_rx) = mpsc::channel::<BackendStateRequest>();
    let (response_tx, response_rx) = mpsc::channel::<BackendStateResponse>();

    std::thread::spawn(move || {
        while let Ok(mut request) = request_rx.recv() {
            while let Ok(next_request) = request_rx.try_recv() {
                request = next_request;
            }

            let active_wallpaper_assignments = get_active_wallpaper_assignments_if_supported();
            let rotation_status = get_rotation_service_status();

            let mut snapshot = request.fallback;
            let mut has_success = false;

            if let Ok(Some(assignments)) = active_wallpaper_assignments {
                log::debug!("gui backend state worker refreshed active wallpaper paths");
                set_active_wallpaper_assignments_from_backend(
                    &mut snapshot.active_wallpaper_assignments,
                    &assignments,
                );
                has_success = true;
            }

            if let Ok(status) = rotation_status {
                snapshot.rotation_status_text = format_rotation_service_status(&status);
                snapshot.rotation_service_state = Some(status);
                has_success = true;
            }

            let _ = response_tx.send(BackendStateResponse {
                request_id: request.request_id,
                snapshot: has_success.then_some(snapshot),
            });
        }
    });

    (request_tx, response_rx)
}

fn install_editorial_mono(ctx: &egui::Context) {
    let candidates = [
        "/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
        "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf",
        "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
    ];

    let Some(bytes) = candidates.iter().find_map(|path| fs::read(path).ok()) else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("editorial-mono".into(), FontData::from_owned(bytes).into());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "editorial-mono".into());
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "editorial-mono".into());
    ctx.set_fonts(fonts);
}

fn section_heading(ui: &mut Ui, label: &str, palette: GuiPalette) {
    ui.label(GuiTypography::rich(
        GuiTextRole::SectionLabel,
        label,
        palette,
    ));
}

fn subtle_rule(ui: &mut Ui, palette: GuiPalette) {
    GuiChrome::rule(ui, palette, 8.0);
}

fn subtle_rule_compact(ui: &mut Ui, palette: GuiPalette) {
    GuiChrome::rule(ui, palette, 3.0);
}

fn render_search_bar(
    ui: &mut Ui,
    value: &mut String,
    focus_search: &mut bool,
    palette: GuiPalette,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(GuiTypography::rich(
            GuiTextRole::MetaLabel,
            "SEARCH",
            palette,
        ));
        let response = ui.add(
            TextEdit::singleline(value)
                .frame(false)
                .font(GuiTypography::font_id(GuiTextRole::MetaValue))
                .hint_text(GuiTypography::rich(
                    GuiTextRole::MetaLabel,
                    "Filter by name or path",
                    palette,
                ))
                .text_color(palette.text)
                .desired_width(finite_ui_width(ui)),
        );
        changed = response.changed();
        if *focus_search {
            response.request_focus();
            *focus_search = false;
        }
        let rect = response.rect;
        ui.painter().line_segment(
            [
                egui::pos2(rect.left(), rect.bottom()),
                egui::pos2(rect.right(), rect.bottom()),
            ],
            Stroke::new(1.0, palette.border),
        );
    });
    changed
}

fn info_row(ui: &mut Ui, label: &str, value: &str, palette: GuiPalette, trailing: Option<String>) {
    ui.horizontal(|ui| {
        ui.label(GuiTypography::rich(GuiTextRole::MetaLabel, label, palette));
        ui.label(GuiTypography::rich(GuiTextRole::MetaValue, value, palette));
        if let Some(trailing) = trailing {
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if !trailing.is_empty() {
                    ui.label(GuiTypography::rich(
                        GuiTextRole::MetaLabel,
                        trailing,
                        palette,
                    ));
                }
            });
        }
    });
    subtle_rule_compact(ui, palette);
}

fn popup_choice_row(
    ui: &mut Ui,
    id_source: impl std::hash::Hash,
    selected: bool,
    label: &str,
    palette: GuiPalette,
) -> egui::Response {
    let (response, _) = interactive_row(ui, id_source, 24.0, |ui, _, _| {
        let color = if selected {
            palette.highlight
        } else {
            palette.text
        };
        ui.label(GuiTypography::rich_color(
            GuiTextRole::MetaValue,
            label,
            color,
        ));
    });
    response
}

fn wallpaper_badges(is_active: bool, in_rotation: bool) -> String {
    match (is_active, in_rotation) {
        (true, true) => "ACTIVE · ROTATION".to_string(),
        (true, false) => "ACTIVE".to_string(),
        (false, true) => "ROTATION".to_string(),
        (false, false) => String::new(),
    }
}

fn finite_ui_width(ui: &Ui) -> f32 {
    let width = ui.available_width();
    if width.is_finite() {
        width.max(120.0)
    } else {
        120.0
    }
}

fn fit_size(texture_size: Vec2, available: Vec2) -> Vec2 {
    if texture_size.x <= 0.0 || texture_size.y <= 0.0 || available.x <= 0.0 || available.y <= 0.0 {
        return Vec2::new(0.0, 0.0);
    }

    let scale = (available.x / texture_size.x)
        .min(available.y / texture_size.y)
        .min(1.0);
    texture_size * scale
}

fn format_resolution(width: Option<u32>, height: Option<u32>) -> String {
    match (width, height) {
        (Some(width), Some(height)) => format!("{width}x{height}"),
        _ => "unknown".to_string(),
    }
}

fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;

    if bytes as f64 >= MB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else if bytes as f64 >= KB {
        format!("{:.1} KB", bytes as f64 / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_timestamp(unix_secs: u64) -> String {
    Local
        .timestamp_opt(unix_secs as i64, 0)
        .single()
        .map(|datetime| datetime.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| unix_secs.to_string())
}

fn format_interval(seconds: u64) -> String {
    let mut parts = Vec::new();
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if secs > 0 || parts.is_empty() {
        parts.push(format!("{secs}s"));
    }

    if parts.len() == 1 && parts[0] == format!("{seconds}s") {
        parts.remove(0)
    } else {
        format!("{seconds}s ({})", parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackendStateSnapshot, DownloadWorkerRequest, DownloadWorkerResponse, GuiApp,
        GuiDownloadStage, GuiDownloadState, RotationServiceStatus, SectionKind, ViewMode,
    };
    use crate::{
        backend::DownloadProgressEvent,
        cache::{IndexedWallpaper, ThumbnailProfile},
        config::Config,
        gui::preview::PreviewKey,
        theme::ThemeKind,
    };
    use eframe::egui::{ColorImage, Context};
    use std::{
        collections::{HashMap, HashSet},
        path::PathBuf,
        sync::mpsc,
        time::{Duration, Instant},
    };
    use tokio_util::sync::CancellationToken;

    fn test_config() -> Config {
        Config {
            wallpaper_paths: vec![],
            theme_name: "System".to_string(),
            rotation: vec![],
            rotate_all_wallpapers: false,
            rotation_same_wallpaper_on_all_displays: true,
            rotation_interval_secs: 300,
            all_sort: "name".to_string(),
            rotation_sort: "name".to_string(),
        }
    }

    fn test_app() -> GuiApp {
        let (preview_tx, _) = mpsc::channel();
        let (_, preview_rx) = mpsc::channel();
        let (index_tx, _) = mpsc::channel();
        let (_, index_rx) = mpsc::channel();
        let (download_tx, _) = mpsc::channel::<DownloadWorkerRequest>();
        let (_, download_rx) = mpsc::channel::<DownloadWorkerResponse>();
        let (backend_state_tx, _) = mpsc::channel();
        let (_, backend_state_rx) = mpsc::channel();

        GuiApp {
            config: test_config(),
            theme: ThemeKind::System,
            wallpapers: vec![],
            view_mode: ViewMode::List,
            active_section: SectionKind::All,
            all_indices: vec![],
            rotation_indices: vec![],
            selected_all: Some(0),
            selected_rotation: Some(0),
            all_filter: String::new(),
            rotation_filter: String::new(),
            active_wallpaper_assignments: HashMap::new(),
            active_wallpaper_paths: HashSet::new(),
            rotation_paths: HashSet::new(),
            texture_cache: super::PreviewTextures::new(),
            thumbnail_cache: None,
            desired_preview_key: None,
            current_preview_key: None,
            preview_request_in_flight: false,
            preview_request_id: 0,
            preview_tx,
            preview_rx,
            index_request_id: 0,
            index_tx,
            index_rx,
            download_request_id: 0,
            download_tx,
            download_rx,
            download_cancel_token: None,
            backend_state_request_id: 0,
            backend_state_tx,
            backend_state_rx,
            backend_state_refresh_interval: Duration::from_secs(2),
            next_backend_state_refresh_at: Instant::now(),
            backend_state_request_in_flight: false,
            rotation_service_state: None,
            rotation_status_text: String::new(),
            display_targets: vec![],
            display_target_selection: None,
            random_targets: vec![],
            random_target_selection: None,
            pending_wallpaper_path: None,
            pending_random_candidates: vec![],
            pending_downloaded_path: None,
            show_paths_dialog: false,
            show_rotation_dialog: false,
            show_help_dialog: false,
            show_uninstall_dialog: false,
            show_display_picker: false,
            show_random_dialog: false,
            show_download_dialog: false,
            manual_path_input: String::new(),
            interval_buffer: String::new(),
            download_url_input: String::new(),
            download_error: None,
            download_path_selection: None,
            last_download_path_selection: None,
            download_state: None,
            focus_download_url_input: false,
            uninstall_confirmed: false,
            uninstall_summary: None,
            uninstall_close_deadline: None,
            focus_search: false,
            toasts: vec![],
            next_toast_id: 0,
        }
    }

    fn sample_wallpaper(path: &str) -> IndexedWallpaper {
        let path = PathBuf::from(path);
        IndexedWallpaper {
            path: path.clone(),
            name: path
                .file_stem()
                .expect("stem")
                .to_string_lossy()
                .to_string(),
            directory: path.parent().expect("parent").to_path_buf(),
            extension: path
                .extension()
                .expect("extension")
                .to_string_lossy()
                .to_string(),
            modified_unix_secs: 0,
            file_size: 0,
            width: Some(1),
            height: Some(1),
        }
    }

    fn preview_key(path: &str) -> PreviewKey {
        PreviewKey {
            path: PathBuf::from(path),
            profile: ThumbnailProfile::GuiPreview,
        }
    }

    fn image() -> ColorImage {
        ColorImage::from_rgba_unmultiplied([1, 1], &[255, 255, 255, 255])
    }

    #[test]
    fn switching_back_to_list_requests_preview_for_selection() {
        let mut app = test_app();
        app.wallpapers = vec![sample_wallpaper("/tmp/preview.png")];
        app.rebuild_section_cache();
        app.selected_all = Some(0);
        app.view_mode = ViewMode::Gallery;

        app.set_view_mode(ViewMode::List);

        assert_eq!(app.view_mode, ViewMode::List);
        assert_eq!(
            app.desired_preview_key,
            Some(preview_key("/tmp/preview.png"))
        );
        assert!(app.preview_request_in_flight);
    }

    #[test]
    fn cycling_sections_wraps_between_all_and_rotation() {
        let mut app = test_app();
        app.wallpapers = vec![
            sample_wallpaper("/tmp/all.png"),
            sample_wallpaper("/tmp/rotation.png"),
        ];
        app.config.rotation = vec![PathBuf::from("/tmp/rotation.png")];
        app.rebuild_section_cache();
        app.selected_all = Some(0);
        app.selected_rotation = Some(0);

        app.cycle_active_section(1);
        assert!(app.active_section == SectionKind::Rotation);

        app.cycle_active_section(1);
        assert!(app.active_section == SectionKind::All);

        app.cycle_active_section(-1);
        assert!(app.active_section == SectionKind::Rotation);
    }

    #[test]
    fn apply_backend_state_snapshot_reports_changes_without_changing_selection() {
        let mut app = test_app();
        let active_assignments =
            HashMap::from([("HDMI-A-1".to_string(), PathBuf::from("/tmp/active.png"))]);
        let mut active_paths = HashSet::new();
        active_paths.insert(PathBuf::from("/tmp/active.png"));
        let status = RotationServiceStatus {
            installed: true,
            enabled: "enabled".to_string(),
            active: "active".to_string(),
            rotates_all_wallpapers: false,
            same_wallpaper_on_all_displays: true,
            interval_secs: 300,
            rotation_entries: 1,
            service_file: PathBuf::from("/tmp/walt-rotation.service"),
        };

        let changed = app.apply_backend_state_snapshot(BackendStateSnapshot {
            active_wallpaper_assignments: active_assignments.clone(),
            rotation_service_state: Some(status.clone()),
            rotation_status_text: "running".to_string(),
        });

        assert!(changed);
        assert_eq!(app.active_wallpaper_assignments, active_assignments);
        assert_eq!(app.active_wallpaper_paths, active_paths);
        assert_eq!(app.rotation_service_state, Some(status));
        assert_eq!(app.rotation_status_text, "running");
        assert_eq!(app.selected_all, Some(0));
        assert_eq!(app.selected_rotation, Some(0));
    }

    #[test]
    fn apply_backend_state_snapshot_is_noop_for_identical_state() {
        let mut app = test_app();
        let snapshot = BackendStateSnapshot {
            active_wallpaper_assignments: HashMap::new(),
            rotation_service_state: None,
            rotation_status_text: String::new(),
        };

        assert!(!app.apply_backend_state_snapshot(snapshot.clone()));
        assert!(!app.apply_backend_state_snapshot(snapshot));
    }

    #[test]
    fn apply_backend_state_snapshot_derives_paths_from_assignments() {
        let mut app = test_app();
        let snapshot = BackendStateSnapshot {
            active_wallpaper_assignments: HashMap::from([
                ("HDMI-A-1".to_string(), PathBuf::from("/tmp/shared.png")),
                ("DP-1".to_string(), PathBuf::from("/tmp/shared.png")),
            ]),
            rotation_service_state: None,
            rotation_status_text: String::new(),
        };

        assert!(app.apply_backend_state_snapshot(snapshot));
        assert_eq!(
            app.active_wallpaper_paths,
            HashSet::from([PathBuf::from("/tmp/shared.png")])
        );
    }

    #[test]
    fn opening_download_dialog_initializes_edit_state() {
        let mut app = test_app();
        app.config.wallpaper_paths = vec![PathBuf::from("/tmp/wallpapers")];

        app.open_download_dialog();

        assert!(app.show_download_dialog);
        assert_eq!(app.download_path_selection, Some(0));
        assert_eq!(
            app.download_state,
            Some(GuiDownloadState {
                stage: GuiDownloadStage::Edit,
                final_path: None,
                downloaded_bytes: 0,
                total_bytes: None,
            })
        );
    }

    #[test]
    fn invalid_download_url_stays_in_edit_state() {
        let mut app = test_app();
        app.config.wallpaper_paths = vec![PathBuf::from("/tmp/wallpapers")];
        app.download_url_input = "notaurl".to_string();
        app.download_path_selection = Some(0);

        app.start_download();

        assert!(app.download_error.is_some());
        assert_eq!(
            app.download_state.as_ref().map(|state| &state.stage),
            Some(&GuiDownloadStage::Edit)
        );
    }

    #[test]
    fn download_worker_send_failure_restores_edit_state() {
        let mut app = test_app();
        app.config.wallpaper_paths = vec![PathBuf::from("/tmp/wallpapers")];
        app.download_url_input = "https://example.com/image.jpg".to_string();
        app.download_path_selection = Some(0);
        app.show_download_dialog = true;

        app.start_download();

        assert!(app.show_download_dialog);
        assert_eq!(app.download_url_input, "https://example.com/image.jpg");
        assert_eq!(app.download_path_selection, Some(0));
        assert!(app.download_error.is_some());
        assert!(app.download_cancel_token.is_none());
        assert_eq!(
            app.download_state.as_ref().map(|state| &state.stage),
            Some(&GuiDownloadStage::Edit)
        );
    }

    #[test]
    fn closing_download_dialog_cancels_active_token() {
        let mut app = test_app();
        let cancel = CancellationToken::new();
        let waiter = cancel.clone();
        app.download_cancel_token = Some(cancel);
        app.download_state = Some(GuiDownloadState {
            stage: GuiDownloadStage::Downloading,
            final_path: None,
            downloaded_bytes: 0,
            total_bytes: None,
        });

        app.close_download_dialog();

        assert!(waiter.is_cancelled());
        assert!(app.show_download_dialog || app.download_state.is_some());
    }

    #[test]
    fn no_wallpaper_paths_leave_download_without_selection() {
        let mut app = test_app();

        app.open_download_dialog();

        assert_eq!(app.download_path_selection, None);
        assert!(app.show_download_dialog);
    }

    #[test]
    fn download_progress_events_update_state() {
        let mut app = test_app();

        app.handle_download_event(DownloadProgressEvent::Started {
            final_path: PathBuf::from("/tmp/wallpapers/sample.jpg"),
            total_bytes: Some(200),
        });
        app.handle_download_event(DownloadProgressEvent::Advanced {
            downloaded_bytes: 75,
            total_bytes: Some(200),
        });

        let state = app.download_state.expect("download state");
        assert_eq!(
            state.final_path,
            Some(PathBuf::from("/tmp/wallpapers/sample.jpg"))
        );
        assert_eq!(state.downloaded_bytes, 75);
        assert_eq!(state.total_bytes, Some(200));
        assert_eq!(state.stage, GuiDownloadStage::Downloading);
    }

    #[test]
    fn download_failure_and_cancellation_set_terminal_state() {
        let mut app = test_app();
        app.download_state = Some(GuiDownloadState {
            stage: GuiDownloadStage::Downloading,
            final_path: None,
            downloaded_bytes: 10,
            total_bytes: Some(20),
        });

        app.handle_download_event(DownloadProgressEvent::Failed {
            message: "boom".to_string(),
        });
        assert_eq!(
            app.download_state.as_ref().map(|state| &state.stage),
            Some(&GuiDownloadStage::Failed("boom".to_string()))
        );

        app.handle_download_event(DownloadProgressEvent::Cancelled);
        assert_eq!(
            app.download_state.as_ref().map(|state| &state.stage),
            Some(&GuiDownloadStage::Cancelled)
        );
    }

    #[test]
    fn successful_download_marks_pending_path() {
        let mut app = test_app();
        app.download_state = Some(GuiDownloadState {
            stage: GuiDownloadStage::Downloading,
            final_path: None,
            downloaded_bytes: 0,
            total_bytes: None,
        });

        app.handle_download_event(DownloadProgressEvent::Finished {
            saved_path: PathBuf::from("/tmp/wallpapers/new.jpg"),
        });

        assert_eq!(
            app.pending_downloaded_path,
            Some(PathBuf::from("/tmp/wallpapers/new.jpg"))
        );
        assert!(!app.show_download_dialog);
    }

    #[test]
    fn select_downloaded_wallpaper_promotes_all_section_selection() {
        let mut app = test_app();
        app.wallpapers = vec![IndexedWallpaper {
            path: PathBuf::from("/tmp/wallpapers/new.jpg"),
            name: "new".to_string(),
            directory: PathBuf::from("/tmp/wallpapers"),
            extension: "jpg".to_string(),
            modified_unix_secs: 0,
            file_size: 0,
            width: None,
            height: None,
        }];
        app.rebuild_section_cache();
        app.pending_downloaded_path = Some(PathBuf::from("/tmp/wallpapers/new.jpg"));

        let changed = app.select_downloaded_wallpaper_in_all();

        assert!(changed);
        assert!(app.active_section == SectionKind::All);
        assert_eq!(app.selected_all, Some(0));
        assert_eq!(app.pending_downloaded_path, None);
    }

    #[test]
    fn close_top_dialog_prefers_paths_over_download() {
        let mut app = test_app();
        app.show_download_dialog = true;
        app.show_paths_dialog = true;

        assert!(app.close_top_dialog());
        assert!(!app.show_paths_dialog);
        assert!(app.show_download_dialog);
    }

    #[test]
    fn request_preview_load_queues_when_selected_preview_missing() {
        let mut app = test_app();
        app.wallpapers = vec![sample_wallpaper("/tmp/preview.png")];
        app.rebuild_section_cache();
        app.selected_all = Some(0);

        app.request_preview_load();

        assert_eq!(
            app.desired_preview_key,
            Some(preview_key("/tmp/preview.png"))
        );
        assert!(app.preview_request_in_flight);
    }

    #[test]
    fn request_preview_load_is_noop_when_texture_already_cached() {
        let mut app = test_app();
        let ctx = Context::default();
        let key = preview_key("/tmp/preview.png");
        app.wallpapers = vec![sample_wallpaper("/tmp/preview.png")];
        app.rebuild_section_cache();
        app.selected_all = Some(0);
        app.texture_cache.insert(&ctx, key.clone(), image());

        app.request_preview_load();

        assert_eq!(app.desired_preview_key, Some(key.clone()));
        assert_eq!(app.current_preview_key, Some(key));
        assert!(!app.preview_request_in_flight);
    }

    #[test]
    fn ensure_selected_preview_is_available_requeues_missing_selected_texture() {
        let mut app = test_app();
        app.wallpapers = vec![sample_wallpaper("/tmp/preview.png")];
        app.rebuild_section_cache();
        app.selected_all = Some(0);
        app.desired_preview_key = Some(preview_key("/tmp/preview.png"));
        app.preview_request_in_flight = false;

        app.ensure_selected_preview_is_available();

        assert!(app.preview_request_in_flight);
        assert_eq!(
            app.desired_preview_key,
            Some(preview_key("/tmp/preview.png"))
        );
    }

    #[test]
    fn current_preview_texture_prefers_cached_desired_key() {
        let mut app = test_app();
        let ctx = Context::default();
        let desired = preview_key("/tmp/desired.png");
        app.desired_preview_key = Some(desired.clone());
        app.current_preview_key = Some(preview_key("/tmp/stale.png"));
        app.texture_cache.insert(&ctx, desired.clone(), image());

        let texture = app.current_preview_texture();

        assert!(texture.is_some());
        assert_eq!(app.current_preview_key, Some(desired));
    }

    #[test]
    fn ensure_selected_preview_clears_state_when_nothing_selected() {
        let mut app = test_app();
        app.selected_all = None;
        app.selected_rotation = None;
        app.desired_preview_key = Some(preview_key("/tmp/preview.png"));
        app.current_preview_key = Some(preview_key("/tmp/preview.png"));
        app.preview_request_in_flight = true;

        app.ensure_selected_preview_is_available();

        assert_eq!(app.desired_preview_key, None);
        assert_eq!(app.current_preview_key, None);
        assert!(!app.preview_request_in_flight);
    }

    #[test]
    fn drain_preview_updates_clears_in_flight_on_success() {
        let mut app = test_app();
        let ctx = Context::default();
        let (response_tx, response_rx) = mpsc::channel();
        app.preview_rx = response_rx;
        app.preview_request_id = 7;
        app.preview_request_in_flight = true;
        response_tx
            .send(super::PreviewResponse {
                request_id: 7,
                key: preview_key("/tmp/preview.png"),
                image: Ok(image()),
            })
            .expect("send response");
        drop(response_tx);

        app.drain_preview_updates(&ctx);

        assert!(!app.preview_request_in_flight);
        assert_eq!(
            app.current_preview_key,
            Some(preview_key("/tmp/preview.png"))
        );
    }

    #[test]
    fn drain_preview_updates_clears_in_flight_on_failure() {
        let mut app = test_app();
        let ctx = Context::default();
        let (response_tx, response_rx) = mpsc::channel();
        app.preview_rx = response_rx;
        app.preview_request_id = 9;
        app.preview_request_in_flight = true;
        response_tx
            .send(super::PreviewResponse {
                request_id: 9,
                key: preview_key("/tmp/preview.png"),
                image: Err(anyhow::anyhow!("failed")),
            })
            .expect("send response");
        drop(response_tx);

        app.drain_preview_updates(&ctx);

        assert!(!app.preview_request_in_flight);
    }
}
