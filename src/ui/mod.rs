mod theme;

use chrono::{Local, TimeZone};
use rand::Rng;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    crossterm::{
        event::{
            self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
            KeyModifiers,
        },
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph as Para},
    Frame, Terminal,
};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol, Resize, StatefulImage};
use std::{
    collections::HashSet,
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
};

use crate::backend::{
    disable_rotation_service, enable_rotation_service, get_active_wallpapers,
    get_rotation_service_status, install_rotation_service, rotation_service_badge,
    rotation_service_status, scan_directory, set_wallpaper, uninstall_rotation_service,
    RotationServiceStatus,
};
use crate::cache::{IndexedWallpaper, ThumbnailCache, WallpaperIndex};
use crate::config::Config;
use theme::{ThemeKind, ThemePalette};

#[derive(Clone, Copy, Eq, PartialEq)]
enum AppMode {
    Setup,
    PathManage,
    Wallpaper,
    Search,
    IntervalEdit,
    RotationMenu,
    Keybindings,
    ThemeSelect,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SectionKind {
    All,
    Rotation,
}

impl SectionKind {
    const ALL: [SectionKind; 2] = [SectionKind::All, SectionKind::Rotation];

    fn title(self) -> &'static str {
        match self {
            Self::All => " All ",
            Self::Rotation => " Rotation ",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Rotation => "rotation",
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SortMode {
    Name,
    Modified,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RotationMenuAction {
    InstallOrUninstall,
    EnableOrDisable,
    SetInterval,
}

impl RotationMenuAction {
    const ALL: [RotationMenuAction; 3] = [
        RotationMenuAction::InstallOrUninstall,
        RotationMenuAction::EnableOrDisable,
        RotationMenuAction::SetInterval,
    ];

    fn label(self, status: Option<&RotationServiceStatus>) -> &'static str {
        match self {
            Self::InstallOrUninstall => {
                if status.map(|status| status.installed).unwrap_or(false) {
                    "Uninstall service"
                } else {
                    "Install service"
                }
            }
            Self::EnableOrDisable => {
                if status
                    .map(|status| status.active == "active")
                    .unwrap_or(false)
                {
                    "Disable service"
                } else {
                    "Enable service"
                }
            }
            Self::SetInterval => "Change interval",
        }
    }
}

impl SortMode {
    fn from_name(name: &str) -> Self {
        match name {
            "modified" => Self::Modified,
            _ => Self::Name,
        }
    }

    fn as_name(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Modified => "modified",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Modified => "Modified",
        }
    }

    fn toggle(self) -> Self {
        match self {
            Self::Name => Self::Modified,
            Self::Modified => Self::Name,
        }
    }
}

struct PreviewRequest {
    request_id: u64,
    image_path: PathBuf,
    area: Rect,
}

struct PreviewResponse {
    request_id: u64,
    protocol: anyhow::Result<StatefulProtocol>,
}

struct IndexRequest {
    request_id: u64,
    wallpaper_paths: Vec<PathBuf>,
}

struct IndexResponse {
    request_id: u64,
    wallpapers: anyhow::Result<Vec<IndexedWallpaper>>,
}

pub struct App {
    config: Config,
    theme: ThemeKind,
    theme_before_picker: ThemeKind,
    theme_return_mode: AppMode,
    wallpapers: Vec<IndexedWallpaper>,
    path_state: ListState,
    all_state: ListState,
    rotation_state: ListState,
    theme_state: ListState,
    rotation_menu_state: ListState,
    active_section: SectionKind,
    mode: AppMode,
    interval_return_mode: AppMode,
    preview_area: Rect,
    preview_request_id: u64,
    preview_tx: Sender<PreviewRequest>,
    preview_rx: Receiver<PreviewResponse>,
    prewarm_tx: Sender<Vec<PathBuf>>,
    current_image: Option<StatefulProtocol>,
    wallpaper_index: WallpaperIndex,
    index_request_id: u64,
    index_tx: Sender<IndexRequest>,
    index_rx: Receiver<IndexResponse>,
    search_buffer: String,
    search_before_open: String,
    interval_buffer: String,
    all_indices: Vec<usize>,
    rotation_indices: Vec<usize>,
    active_wallpaper_paths: HashSet<PathBuf>,
    rotation_paths: HashSet<PathBuf>,
    last_preview_target: Option<(PathBuf, Rect)>,
    rotation_service_state: Option<RotationServiceStatus>,
    rotation_status_text: String,
    input_buffer: String,
    all_filter: String,
    rotation_filter: String,
    dir_suggestions: Vec<PathBuf>,
    suggestion_state: ListState,
}

impl App {
    pub fn new() -> anyhow::Result<Self> {
        let config = Config::new();
        let theme = ThemeKind::from_name(&config.theme_name);
        let picker = Picker::from_query_stdio()?;
        let wallpaper_index = WallpaperIndex::new()?;
        let thumbnail_cache = ThumbnailCache::new().ok();
        let wallpapers = if config.is_empty() {
            vec![]
        } else {
            wallpaper_index.load(&config.wallpaper_paths)
        };

        let mut suggestion_state = ListState::default();
        suggestion_state.select(Some(0));
        let mut theme_state = ListState::default();
        theme_state.select(Some(theme.index()));
        let mut rotation_menu_state = ListState::default();
        rotation_menu_state.select(Some(0));

        let mode = if config.is_empty() {
            AppMode::Setup
        } else {
            AppMode::Wallpaper
        };
        let (preview_tx, preview_rx) = spawn_preview_worker(picker, thumbnail_cache.clone());
        let prewarm_tx = spawn_prewarm_worker(thumbnail_cache);
        let (index_tx, index_rx) = spawn_index_worker(WallpaperIndex::new()?);

        let mut app = Self {
            config,
            theme,
            theme_before_picker: theme,
            theme_return_mode: mode,
            wallpapers,
            path_state: ListState::default(),
            all_state: ListState::default(),
            rotation_state: ListState::default(),
            theme_state,
            rotation_menu_state,
            active_section: SectionKind::All,
            mode,
            interval_return_mode: AppMode::Wallpaper,
            preview_area: Rect::default(),
            preview_request_id: 0,
            preview_tx,
            preview_rx,
            prewarm_tx,
            current_image: None,
            wallpaper_index,
            index_request_id: 0,
            index_tx,
            index_rx,
            search_buffer: String::new(),
            search_before_open: String::new(),
            interval_buffer: String::new(),
            all_indices: vec![],
            rotation_indices: vec![],
            active_wallpaper_paths: HashSet::new(),
            rotation_paths: HashSet::new(),
            last_preview_target: None,
            rotation_service_state: None,
            rotation_status_text: String::new(),
            input_buffer: String::new(),
            all_filter: String::new(),
            rotation_filter: String::new(),
            dir_suggestions: vec![],
            suggestion_state,
        };

        app.rebuild_section_cache();
        app.ensure_section_selection();
        app.refresh_active_wallpapers();
        app.select_active_wallpaper_in_all();
        app.refresh_rotation_status();

        if !app.config.is_empty() {
            app.request_index_refresh();
        }

        if app.current_selected_wallpaper().is_some() && app.mode == AppMode::Wallpaper {
            app.request_preview_load();
        }

        Ok(app)
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let res = self.run_app(&mut terminal);

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        if let Err(err) = res {
            eprintln!("Error: {:?}", err);
        }

        Ok(())
    }

    fn run_app<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        loop {
            self.drain_preview_updates();
            self.drain_index_updates();
            terminal.draw(|f| self.ui(f))?;

            if event::poll(std::time::Duration::from_millis(16))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match self.mode {
                            AppMode::Setup => self.handle_setup_key(key.code)?,
                            AppMode::PathManage => self.handle_path_manage_key(key.code),
                            AppMode::Search => self.handle_search_key(key.code),
                            AppMode::IntervalEdit => self.handle_interval_key(key.code),
                            AppMode::RotationMenu => self.handle_rotation_menu_key(key.code),
                            AppMode::Keybindings => self.handle_keybindings_key(key.code),
                            AppMode::ThemeSelect => self.handle_theme_select_key(key.code),
                            AppMode::Wallpaper => {
                                if self.handle_wallpaper_key(key)? {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn handle_setup_key(&mut self, key: KeyCode) -> io::Result<()> {
        match key {
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Home => self.go_to_top(),
            KeyCode::End => self.go_to_bottom(),
            KeyCode::Enter => self.handle_enter()?,
            KeyCode::Backspace => {
                self.input_buffer.pop();
                self.update_suggestions();
            }
            KeyCode::Tab => {
                if let Some(idx) = self.suggestion_state.selected() {
                    if let Some(dir) = self.dir_suggestions.get(idx) {
                        self.input_buffer = dir.to_string_lossy().to_string();
                        self.dir_suggestions = vec![];
                    }
                }
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
                self.update_suggestions();
            }
            _ => {}
        }

        Ok(())
    }

    fn handle_path_manage_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('p') => {
                self.mode = AppMode::Wallpaper;
                self.refresh_wallpapers();
            }
            KeyCode::Char('a') => {
                self.input_buffer.clear();
                self.update_suggestions();
                self.mode = AppMode::Setup;
            }
            KeyCode::Char('d') => {
                if let Some(idx) = self.path_state.selected() {
                    if idx < self.config.wallpaper_paths.len() {
                        let path_clone = self.config.wallpaper_paths[idx].clone();
                        self.config.remove_path(&path_clone);
                        let _ = self.config.save();
                        self.refresh_wallpapers();
                    }
                }
            }
            KeyCode::Char('t') => self.open_theme_picker(),
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Char('g') | KeyCode::Home => self.go_to_top(),
            KeyCode::Char('G') | KeyCode::End => self.go_to_bottom(),
            _ => {}
        }
    }

    fn handle_wallpaper_key(&mut self, key: KeyEvent) -> io::Result<bool> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::NONE) | (KeyCode::Esc, _) => return Ok(true),
            (KeyCode::Char('p'), KeyModifiers::NONE) => self.mode = AppMode::PathManage,
            (KeyCode::Char('r'), KeyModifiers::NONE) => self.toggle_rotation(),
            (KeyCode::Char('r'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
                self.select_random_wallpaper()?
            }
            (KeyCode::Char('t'), KeyModifiers::NONE) => self.open_theme_picker(),
            (KeyCode::Char('i'), KeyModifiers::NONE) => self.open_interval_editor(),
            (KeyCode::Char('R'), KeyModifiers::SHIFT) => self.open_rotation_menu(),
            (KeyCode::Char('s'), KeyModifiers::NONE) => self.toggle_sort_mode(),
            (KeyCode::Char('/'), KeyModifiers::NONE) => self.open_search(),
            (KeyCode::Char('?'), _) | (KeyCode::Char('/'), KeyModifiers::SHIFT) => {
                self.mode = AppMode::Keybindings
            }
            (KeyCode::Tab, _) | (KeyCode::Char('l'), KeyModifiers::NONE) => self.next_section(),
            (KeyCode::BackTab, _) | (KeyCode::Char('h'), KeyModifiers::NONE) => {
                self.previous_section()
            }
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, _) => self.move_down(),
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, _) => self.move_up(),
            (KeyCode::Char('g'), KeyModifiers::NONE) | (KeyCode::Home, _) => self.go_to_top(),
            (KeyCode::Char('G'), KeyModifiers::SHIFT) | (KeyCode::End, _) => self.go_to_bottom(),
            (KeyCode::Enter, _) => self.handle_enter()?,
            _ => {}
        }

        Ok(false)
    }

    fn handle_search_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.set_active_filter(self.search_before_open.clone());
                self.mode = AppMode::Wallpaper;
                self.ensure_section_selection();
                self.request_preview_load();
            }
            KeyCode::Enter => {
                self.set_active_filter(self.search_buffer.clone());
                self.mode = AppMode::Wallpaper;
                self.ensure_section_selection();
                self.request_preview_load();
            }
            KeyCode::Backspace => {
                self.search_buffer.pop();
                self.set_active_filter(self.search_buffer.clone());
                self.ensure_section_selection();
                self.request_preview_load();
            }
            KeyCode::Char(c) => {
                self.search_buffer.push(c);
                self.set_active_filter(self.search_buffer.clone());
                self.ensure_section_selection();
                self.request_preview_load();
            }
            _ => {}
        }
    }

    fn handle_interval_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => self.mode = self.interval_return_mode,
            KeyCode::Enter => {
                if let Ok(seconds) = self.interval_buffer.parse::<u64>() {
                    if self.config.set_rotation_interval_secs(seconds).is_ok() {
                        self.refresh_rotation_status();
                        self.mode = self.interval_return_mode;
                    }
                }
            }
            KeyCode::Backspace => {
                self.interval_buffer.pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() => self.interval_buffer.push(c),
            _ => {}
        }
    }

    fn handle_rotation_menu_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('R') => {
                self.mode = AppMode::Wallpaper;
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Char('g') | KeyCode::Home => self.go_to_top(),
            KeyCode::Char('G') | KeyCode::End => self.go_to_bottom(),
            KeyCode::Enter => self.run_rotation_menu_action(),
            _ => {}
        }
    }

    fn handle_keybindings_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.mode = AppMode::Wallpaper;
            }
            _ => {}
        }
    }

    fn handle_theme_select_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc | KeyCode::Char('q') => self.cancel_theme_picker(),
            KeyCode::Enter => self.confirm_theme_picker(),
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Char('g') | KeyCode::Home => self.go_to_top(),
            KeyCode::Char('G') | KeyCode::End => self.go_to_bottom(),
            _ => {}
        }
    }

    fn open_theme_picker(&mut self) {
        self.theme_before_picker = self.theme;
        self.theme_return_mode = self.mode;
        self.theme_state.select(Some(self.theme.index()));
        self.mode = AppMode::ThemeSelect;
    }

    fn confirm_theme_picker(&mut self) {
        self.config.set_theme(self.theme.name());
        let _ = self.config.save();
        self.mode = self.theme_return_mode;
    }

    fn cancel_theme_picker(&mut self) {
        self.theme = self.theme_before_picker;
        self.theme_state.select(Some(self.theme.index()));
        self.mode = self.theme_return_mode;
    }

    fn handle_enter(&mut self) -> io::Result<()> {
        match self.mode {
            AppMode::Setup => {
                let path = PathBuf::from(&self.input_buffer);
                if path.is_dir() {
                    self.config.add_path(path);
                    let _ = self.config.save();
                    self.input_buffer.clear();
                    self.dir_suggestions.clear();
                    self.mode = AppMode::Wallpaper;
                    self.refresh_wallpapers();
                }
            }
            AppMode::Wallpaper => {
                if let Err(error) = self.apply_wallpaper() {
                    eprintln!("Failed to apply: {error}");
                }
            }
            AppMode::Search => {}
            AppMode::IntervalEdit => {}
            AppMode::RotationMenu => {}
            AppMode::Keybindings => {}
            AppMode::ThemeSelect => self.confirm_theme_picker(),
            AppMode::PathManage => {}
        }
        Ok(())
    }

    fn move_down(&mut self) {
        match self.mode {
            AppMode::Setup => {
                if !self.dir_suggestions.is_empty() {
                    let index = self.suggestion_state.selected().unwrap_or(0);
                    self.suggestion_state
                        .select(Some((index + 1) % self.dir_suggestions.len()));
                }
            }
            AppMode::ThemeSelect => {
                let index = self.theme_state.selected().unwrap_or(self.theme.index());
                let new_index = (index + 1) % ThemeKind::ALL.len();
                self.theme_state.select(Some(new_index));
                self.theme = ThemeKind::ALL[new_index];
            }
            AppMode::PathManage => {
                let len = self.config.wallpaper_paths.len();
                if len > 0 {
                    let index = self.path_state.selected().unwrap_or(0);
                    self.path_state.select(Some((index + 1) % len));
                }
            }
            AppMode::Wallpaper => {
                let len = self.section_indices(self.active_section).len();
                if len > 0 {
                    let state = self.section_state_mut(self.active_section);
                    let index = state.selected().unwrap_or(0);
                    state.select(Some((index + 1) % len));
                    self.request_preview_load();
                }
            }
            AppMode::Search => {}
            AppMode::IntervalEdit => {}
            AppMode::RotationMenu => {
                let len = RotationMenuAction::ALL.len();
                if len > 0 {
                    let index = self.rotation_menu_state.selected().unwrap_or(0);
                    self.rotation_menu_state.select(Some((index + 1) % len));
                }
            }
            AppMode::Keybindings => {}
        }
    }

    fn move_up(&mut self) {
        match self.mode {
            AppMode::Setup => {
                if !self.dir_suggestions.is_empty() {
                    let index = self.suggestion_state.selected().unwrap_or(0);
                    self.suggestion_state.select(Some(if index == 0 {
                        self.dir_suggestions.len() - 1
                    } else {
                        index - 1
                    }));
                }
            }
            AppMode::ThemeSelect => {
                let index = self.theme_state.selected().unwrap_or(self.theme.index());
                let new_index = if index == 0 {
                    ThemeKind::ALL.len() - 1
                } else {
                    index - 1
                };
                self.theme_state.select(Some(new_index));
                self.theme = ThemeKind::ALL[new_index];
            }
            AppMode::PathManage => {
                let len = self.config.wallpaper_paths.len();
                if len > 0 {
                    let index = self.path_state.selected().unwrap_or(0);
                    self.path_state
                        .select(Some(if index == 0 { len - 1 } else { index - 1 }));
                }
            }
            AppMode::Wallpaper => {
                let len = self.section_indices(self.active_section).len();
                if len > 0 {
                    let state = self.section_state_mut(self.active_section);
                    let index = state.selected().unwrap_or(0);
                    state.select(Some(if index == 0 { len - 1 } else { index - 1 }));
                    self.request_preview_load();
                }
            }
            AppMode::Search => {}
            AppMode::IntervalEdit => {}
            AppMode::RotationMenu => {
                let len = RotationMenuAction::ALL.len();
                if len > 0 {
                    let index = self.rotation_menu_state.selected().unwrap_or(0);
                    self.rotation_menu_state.select(Some(if index == 0 {
                        len - 1
                    } else {
                        index - 1
                    }));
                }
            }
            AppMode::Keybindings => {}
        }
    }

    fn go_to_top(&mut self) {
        match self.mode {
            AppMode::Setup => self.suggestion_state.select(Some(0)),
            AppMode::ThemeSelect => {
                self.theme_state.select(Some(0));
                self.theme = ThemeKind::ALL[0];
            }
            AppMode::PathManage => {
                if !self.config.wallpaper_paths.is_empty() {
                    self.path_state.select(Some(0));
                }
            }
            AppMode::Wallpaper => {
                if !self.section_indices(self.active_section).is_empty() {
                    self.section_state_mut(self.active_section).select(Some(0));
                    self.request_preview_load();
                }
            }
            AppMode::Search => {}
            AppMode::IntervalEdit => {}
            AppMode::RotationMenu => self.rotation_menu_state.select(Some(0)),
            AppMode::Keybindings => {}
        }
    }

    fn go_to_bottom(&mut self) {
        match self.mode {
            AppMode::Setup => {
                if !self.dir_suggestions.is_empty() {
                    self.suggestion_state
                        .select(Some(self.dir_suggestions.len() - 1));
                }
            }
            AppMode::ThemeSelect => {
                let last = ThemeKind::ALL.len() - 1;
                self.theme_state.select(Some(last));
                self.theme = ThemeKind::ALL[last];
            }
            AppMode::PathManage => {
                if !self.config.wallpaper_paths.is_empty() {
                    self.path_state
                        .select(Some(self.config.wallpaper_paths.len() - 1));
                }
            }
            AppMode::Wallpaper => {
                let len = self.section_indices(self.active_section).len();
                if len > 0 {
                    self.section_state_mut(self.active_section)
                        .select(Some(len - 1));
                    self.request_preview_load();
                }
            }
            AppMode::Search => {}
            AppMode::IntervalEdit => {}
            AppMode::RotationMenu => {
                self.rotation_menu_state
                    .select(Some(RotationMenuAction::ALL.len() - 1));
            }
            AppMode::Keybindings => {}
        }
    }

    fn next_section(&mut self) {
        let current = SectionKind::ALL
            .iter()
            .position(|section| *section == self.active_section)
            .unwrap_or(0);
        self.active_section = SectionKind::ALL[(current + 1) % SectionKind::ALL.len()];
        self.ensure_section_selection();
        self.request_preview_load();
    }

    fn previous_section(&mut self) {
        let current = SectionKind::ALL
            .iter()
            .position(|section| *section == self.active_section)
            .unwrap_or(0);
        self.active_section = if current == 0 {
            SectionKind::ALL[SectionKind::ALL.len() - 1]
        } else {
            SectionKind::ALL[current - 1]
        };
        self.ensure_section_selection();
        self.request_preview_load();
    }

    fn toggle_rotation(&mut self) {
        let Some(path) = self
            .current_selected_wallpaper()
            .map(|wallpaper| wallpaper.path.clone())
        else {
            return;
        };

        self.config.toggle_rotation(&path);
        let _ = self.config.save();
        self.rebuild_section_cache();
        self.ensure_section_selection();
        if self.mode == AppMode::RotationMenu {
            self.refresh_rotation_status();
        }
        self.request_preview_load();
    }

    fn toggle_sort_mode(&mut self) {
        let next = self.sort_mode(self.active_section).toggle();
        self.config
            .set_sort_name_for_section(self.active_section.key(), next.as_name());
        let _ = self.config.save();
        self.ensure_section_selection();
        self.request_preview_load();
    }

    fn open_search(&mut self) {
        self.search_buffer = self.active_filter().to_string();
        self.search_before_open = self.search_buffer.clone();
        self.mode = AppMode::Search;
    }

    fn open_interval_editor(&mut self) {
        self.interval_return_mode = self.mode;
        self.interval_buffer = self.config.rotation_interval_secs.to_string();
        self.mode = AppMode::IntervalEdit;
    }

    fn open_rotation_menu(&mut self) {
        self.rotation_menu_state.select(Some(0));
        self.refresh_rotation_status();
        self.mode = AppMode::RotationMenu;
    }

    fn refresh_active_wallpapers(&mut self) {
        match get_active_wallpapers() {
            Ok(paths) => {
                self.active_wallpaper_paths = paths.into_iter().collect();
            }
            Err(error) => {
                eprintln!("Failed to refresh active wallpapers: {error}");
            }
        }
    }

    fn is_active_wallpaper(&self, path: &PathBuf) -> bool {
        self.active_wallpaper_paths.contains(path)
    }

    fn refresh_rotation_status(&mut self) {
        self.rotation_service_state = get_rotation_service_status().ok();
        self.rotation_status_text = rotation_service_status().unwrap_or_else(|error| {
            format!("Rotation Service\nStatus:   error\nError:    {error}")
        });
    }

    fn run_rotation_menu_action(&mut self) {
        let Some(index) = self.rotation_menu_state.selected() else {
            return;
        };

        let action = RotationMenuAction::ALL[index];
        match action {
            RotationMenuAction::InstallOrUninstall => {
                if self
                    .rotation_service_state
                    .as_ref()
                    .map(|status| status.installed)
                    .unwrap_or(false)
                {
                    self.run_rotation_service_action(|| uninstall_rotation_service());
                } else {
                    self.run_rotation_service_action(|| install_rotation_service());
                }
            }
            RotationMenuAction::EnableOrDisable => {
                if self
                    .rotation_service_state
                    .as_ref()
                    .map(|status| status.active == "active")
                    .unwrap_or(false)
                {
                    self.run_rotation_service_action(|| disable_rotation_service());
                } else {
                    self.run_rotation_service_action(|| enable_rotation_service());
                }
            }
            RotationMenuAction::SetInterval => self.open_interval_editor(),
        }
    }

    fn run_rotation_service_action<F>(&mut self, action: F)
    where
        F: FnOnce() -> anyhow::Result<()>,
    {
        match action() {
            Ok(()) => self.refresh_rotation_status(),
            Err(error) => {
                self.rotation_service_state = None;
                self.rotation_status_text =
                    format!("Rotation Service\nStatus:   error\nError:    {error}");
            }
        }
    }

    fn section_state_mut(&mut self, section: SectionKind) -> &mut ListState {
        match section {
            SectionKind::All => &mut self.all_state,
            SectionKind::Rotation => &mut self.rotation_state,
        }
    }

    fn section_state(&self, section: SectionKind) -> &ListState {
        match section {
            SectionKind::All => &self.all_state,
            SectionKind::Rotation => &self.rotation_state,
        }
    }

    fn section_indices(&self, section: SectionKind) -> Vec<usize> {
        let base_indices = match section {
            SectionKind::All => &self.all_indices,
            SectionKind::Rotation => &self.rotation_indices,
        };
        let filter = self.filter_query(section).to_lowercase();
        let mut indices = base_indices
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

    fn sort_mode(&self, section: SectionKind) -> SortMode {
        SortMode::from_name(self.config.sort_name_for_section(section.key()))
    }

    fn filter_query(&self, section: SectionKind) -> &str {
        match section {
            SectionKind::All => &self.all_filter,
            SectionKind::Rotation => &self.rotation_filter,
        }
    }

    fn active_filter(&self) -> &str {
        self.filter_query(self.active_section)
    }

    fn set_active_filter(&mut self, value: String) {
        match self.active_section {
            SectionKind::All => self.all_filter = value,
            SectionKind::Rotation => self.rotation_filter = value,
        }
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

    fn ensure_section_selection(&mut self) {
        for section in SectionKind::ALL {
            let len = self.section_indices(section).len();
            let state = self.section_state_mut(section);
            if len == 0 {
                state.select(None);
            } else {
                let selected = state.selected().unwrap_or(0).min(len - 1);
                state.select(Some(selected));
            }
        }
    }

    fn current_selected_wallpaper(&self) -> Option<&IndexedWallpaper> {
        let indices = self.section_indices(self.active_section);
        let selected = self.section_state(self.active_section).selected()?;
        let wallpaper_index = *indices.get(selected)?;
        self.wallpapers.get(wallpaper_index)
    }

    fn select_active_wallpaper_in_all(&mut self) {
        let all_indices = self.section_indices(SectionKind::All);
        let Some(selected) = first_active_visible_index(
            &all_indices,
            &self.wallpapers,
            &self.active_wallpaper_paths,
        ) else {
            return;
        };

        self.all_state.select(Some(selected));
    }

    fn update_suggestions(&mut self) {
        self.dir_suggestions = if self.input_buffer.is_empty() {
            vec![
                PathBuf::from("/home"),
                dirs::picture_dir().unwrap_or_else(|| PathBuf::from("/home")),
                dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home")),
            ]
        } else if let Some(parent) = PathBuf::from(&self.input_buffer).parent() {
            if parent.is_dir() {
                scan_directory(parent)
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        if !self.dir_suggestions.is_empty() {
            self.suggestion_state.select(Some(0));
        }
    }

    fn refresh_wallpapers(&mut self) {
        if self.config.is_empty() {
            self.wallpapers.clear();
            self.rebuild_section_cache();
            self.mode = AppMode::Setup;
        } else {
            self.wallpapers = self.wallpaper_index.load(&self.config.wallpaper_paths);
            self.rebuild_section_cache();
            self.path_state
                .select(if self.config.wallpaper_paths.is_empty() {
                    None
                } else {
                    Some(0)
                });
            self.ensure_section_selection();
            self.select_active_wallpaper_in_all();
            if self.current_selected_wallpaper().is_some() {
                self.request_preview_load();
            }
            self.request_index_refresh();
        }
    }

    fn select_random_wallpaper(&mut self) -> io::Result<()> {
        let indices = self.section_indices(self.active_section);
        if indices.is_empty() {
            return Ok(());
        }

        let random_index = rand::thread_rng().gen_range(0..indices.len());
        self.section_state_mut(self.active_section)
            .select(Some(random_index));
        self.request_preview_load();

        if let Err(error) = self.apply_wallpaper() {
            eprintln!("Failed to apply random wallpaper: {error}");
        }

        Ok(())
    }

    fn ui(&mut self, frame: &mut Frame) {
        let theme = self.theme.palette();
        frame.render_widget(Block::default().style(theme.surface), frame.area());
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(3)])
            .split(frame.area());

        match self.mode {
            AppMode::Setup => self.render_setup(frame, chunks[0], theme),
            AppMode::PathManage => self.render_path_manage(frame, chunks[0], theme),
            AppMode::ThemeSelect => self.render_theme_picker(frame, chunks[0], theme),
            AppMode::Wallpaper
            | AppMode::Search
            | AppMode::IntervalEdit
            | AppMode::RotationMenu
            | AppMode::Keybindings => {
                let main_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
                    .split(chunks[0]);
                let right_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(12), Constraint::Length(8)])
                    .split(main_chunks[1]);
                let preview_area = self.themed_block(" Preview ", theme).inner(right_chunks[0]);
                if preview_area != self.preview_area {
                    self.preview_area = preview_area;
                    self.request_preview_load();
                }
                self.render_library_sections(frame, main_chunks[0], theme);
                self.render_preview(frame, right_chunks[0], theme);
                self.render_metadata(frame, right_chunks[1], theme);

                match self.mode {
                    AppMode::Search => self.render_search_overlay(frame, chunks[0], theme),
                    AppMode::IntervalEdit => self.render_interval_overlay(frame, chunks[0], theme),
                    AppMode::RotationMenu => {
                        self.render_rotation_menu_overlay(frame, chunks[0], theme)
                    }
                    AppMode::Keybindings => {
                        self.render_keybindings_overlay(frame, chunks[0], theme)
                    }
                    _ => {}
                }
            }
        }

        self.render_help(frame, chunks[1], theme);
    }

    fn themed_block<'a>(&self, title: &'a str, theme: ThemePalette) -> Block<'a> {
        Block::default()
            .style(theme.surface)
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(theme.border)
            .title(title)
            .title_style(theme.title)
    }

    fn render_setup(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let block = self.themed_block(" First Setup - Enter Wallpaper Directory ", theme);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let input_height = 3;
        let input_area = Rect::new(
            inner.x + 1,
            inner.y + 1,
            inner.width.saturating_sub(2),
            input_height,
        );
        let cursor = if self.input_buffer.is_empty() {
            "_"
        } else {
            ""
        };
        let input_style = if self.input_buffer.is_empty() {
            theme.placeholder
        } else {
            theme.accent
        };
        let input = Para::new(Line::from(Span::styled(
            format!("{}{}", self.input_buffer, cursor),
            input_style,
        )))
        .block(self.themed_block(" Path ", theme))
        .alignment(Alignment::Left);
        frame.render_widget(input, input_area);

        if !self.dir_suggestions.is_empty() {
            let list_area = Rect::new(
                inner.x + 1,
                inner.y + input_height + 1,
                inner.width.saturating_sub(2),
                inner.height.saturating_sub(input_height + 2),
            );
            let selected = self.suggestion_state.selected();
            let items: Vec<ListItem> = self
                .dir_suggestions
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    let style = if selected == Some(index) {
                        theme.highlight
                    } else {
                        theme.accent
                    };
                    ListItem::new(path.to_string_lossy().to_string()).style(style)
                })
                .collect();
            let mut state = self.suggestion_state.clone();
            let list = List::new(items)
                .block(self.themed_block(" Directories ", theme))
                .highlight_style(theme.highlight)
                .highlight_symbol("› ");
            frame.render_stateful_widget(list, list_area, &mut state);
        }
    }

    fn render_path_manage(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let block = self.themed_block(" Manage Wallpaper Paths ", theme);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.config.wallpaper_paths.is_empty() {
            frame.render_widget(
                Para::new(Line::from(vec![
                    Span::styled("No paths configured.", theme.muted),
                    Span::raw(" "),
                    Span::styled("Press 'a' to add a path.", theme.key),
                ]))
                .alignment(Alignment::Center),
                inner,
            );
        } else {
            let selected = self.path_state.selected();
            let items: Vec<ListItem> = self
                .config
                .wallpaper_paths
                .iter()
                .enumerate()
                .map(|(index, path)| {
                    let style = if selected == Some(index) {
                        theme.highlight
                    } else {
                        theme.accent
                    };
                    ListItem::new(path.to_string_lossy().to_string()).style(style)
                })
                .collect();
            let mut state = self.path_state.clone();
            let list = List::new(items)
                .block(self.themed_block(" Paths ", theme))
                .highlight_style(theme.highlight)
                .highlight_symbol("› ");
            frame.render_stateful_widget(list, inner, &mut state);
        }
    }

    fn render_theme_picker(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(theme.surface), area);
        let outer = self
            .themed_block(" Theme Picker ", theme)
            .style(theme.surface);
        let inner = outer.inner(area);
        frame.render_widget(outer, area);

        let content = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(inner);

        let picker_help = Para::new(vec![
            Line::from(Span::styled(
                "Select a theme to preview it immediately.",
                theme.muted,
            )),
            Line::from(vec![
                Span::styled("Enter", theme.key),
                Span::raw(" confirm  "),
                Span::styled("Esc", theme.key),
                Span::raw(" cancel"),
            ]),
        ])
        .style(theme.surface);
        frame.render_widget(picker_help, content[0]);

        let selected = self.theme_state.selected();
        let items: Vec<ListItem> = ThemeKind::ALL
            .iter()
            .enumerate()
            .map(|(index, theme_kind)| {
                let style = if selected == Some(index) {
                    theme.highlight
                } else {
                    theme.accent
                };
                ListItem::new(theme_kind.name()).style(style)
            })
            .collect();
        let mut state = self.theme_state.clone();
        let list = List::new(items)
            .block(self.themed_block(" Themes ", theme))
            .style(theme.surface)
            .highlight_style(theme.highlight)
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, content[1], &mut state);
    }

    fn render_library_sections(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
            .split(area);

        self.render_section(
            frame,
            layout[0],
            SectionKind::All,
            theme,
            self.section_indices(SectionKind::All),
            &self.all_state,
        );
        self.render_section(
            frame,
            layout[1],
            SectionKind::Rotation,
            theme,
            self.section_indices(SectionKind::Rotation),
            &self.rotation_state,
        );
    }

    fn render_section(
        &self,
        frame: &mut Frame,
        area: Rect,
        section: SectionKind,
        theme: ThemePalette,
        indices: Vec<usize>,
        state: &ListState,
    ) {
        let mut border_theme = theme;
        if self.active_section == section {
            border_theme.border = theme.highlight;
            border_theme.title = theme.highlight;
        }

        if indices.is_empty() {
            let message = match section {
                SectionKind::All if self.filter_query(section).is_empty() => {
                    "No wallpapers indexed"
                }
                SectionKind::Rotation if self.filter_query(section).is_empty() => {
                    "Rotation list is empty"
                }
                _ => "No matches for current filter",
            };
            frame.render_widget(
                Para::new(Line::from(Span::styled(message, theme.muted)))
                    .block(self.themed_block(section.title(), border_theme))
                    .alignment(Alignment::Center),
                area,
            );
            return;
        }

        let selected = state.selected();
        let items = indices
            .iter()
            .enumerate()
            .filter_map(|(visible_index, index)| {
                self.wallpapers
                    .get(*index)
                    .map(|wallpaper| (visible_index, wallpaper))
            })
            .map(|(visible_index, wallpaper)| {
                let marker = wallpaper_marker_prefix(self.is_active_wallpaper(&wallpaper.path));
                let style = if selected == Some(visible_index) {
                    theme.highlight
                } else {
                    theme.accent
                };
                ListItem::new(format!("{marker}{}", wallpaper.name)).style(style)
            })
            .collect::<Vec<_>>();
        let title = format!(
            "{} [{}{}]",
            section.title().trim(),
            if section == SectionKind::Rotation {
                self.rotation_service_state
                    .as_ref()
                    .map(rotation_service_badge)
                    .unwrap_or("unknown")
                    .to_string()
            } else {
                self.sort_mode(section).label().to_string()
            },
            if section == SectionKind::Rotation {
                format!(" · {}s", self.config.rotation_interval_secs)
            } else {
                String::new()
            }
        );
        let mut list_state = state.clone();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(border_theme.border)
            .title(title)
            .title_style(border_theme.title);
        let list = List::new(items)
            .block(block)
            .highlight_style(theme.highlight)
            .highlight_symbol("› ");
        frame.render_stateful_widget(list, area, &mut list_state);
    }

    fn render_preview(&mut self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let block = self.themed_block(" Preview ", theme);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(Block::default().style(theme.surface), inner);

        if let Some(ref mut protocol) = self.current_image {
            let resize = Resize::Scale(None);
            let render_area = center_rect(inner, protocol.size_for(&resize, inner));
            let image = StatefulImage::default().resize(resize);
            frame.render_stateful_widget(image, render_area, protocol);
        } else {
            let lines = if self.wallpapers.is_empty() {
                vec![
                    Line::from(Span::styled("No wallpapers found", theme.muted)),
                    Line::from(Span::styled("Configure paths with 'p'", theme.key)),
                ]
            } else {
                vec![
                    Line::from(Span::styled("Select a wallpaper", theme.muted)),
                    Line::from(Span::styled("to see preview", theme.muted)),
                ]
            };
            frame.render_widget(Para::new(lines).alignment(Alignment::Center), inner);
        }
    }

    fn render_help(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let controls = match self.mode {
            AppMode::Setup => vec![("Type", "path"), ("Enter", "add"), ("Tab", "suggestion")],
            AppMode::PathManage => vec![("a", "add"), ("d", "remove"), ("p", "back")],
            AppMode::Search => vec![("Type", "filter"), ("Enter", "confirm"), ("Esc", "cancel")],
            AppMode::IntervalEdit => {
                vec![("Type", "seconds"), ("Enter", "save"), ("Esc", "cancel")]
            }
            AppMode::RotationMenu => vec![("↑/↓", "choose"), ("Enter", "run"), ("Esc", "close")],
            AppMode::Keybindings => vec![("?", "close"), ("Esc", "close")],
            AppMode::ThemeSelect => {
                vec![("↑/↓", "preview"), ("Enter", "confirm"), ("Esc", "cancel")]
            }
            AppMode::Wallpaper => vec![
                ("?", "keybindings"),
                ("↑/↓", "move"),
                ("Enter", "apply"),
                ("/", "filter"),
                ("r", "rotate"),
                ("Ctrl+r", "random"),
                ("R", "rotation"),
                ("p", "paths"),
            ],
        };

        let mut spans = vec![Span::raw(" ")];

        for (key, action) in controls {
            spans.push(Span::styled(key, theme.key));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(action, theme.muted));
            spans.push(Span::raw(" | "));
        }

        frame.render_widget(
            Para::new(Line::from(spans))
                .block(self.themed_block(" Help ", theme))
                .alignment(Alignment::Center),
            area,
        );
    }

    fn render_search_overlay(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let popup = centered_rect(60, 5, area);
        let text = if self.search_buffer.is_empty() {
            "/"
        } else {
            ""
        };
        let input = Para::new(Line::from(vec![
            Span::styled("/", theme.key),
            Span::styled(format!("{}{}", self.search_buffer, text), theme.accent),
        ]))
        .block(self.themed_block(" Filter Active Section ", theme))
        .alignment(Alignment::Left);
        self.render_popup(frame, popup, theme, input);
    }

    fn render_interval_overlay(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let popup = centered_rect(60, 5, area);
        let text = if self.interval_buffer.is_empty() {
            "_".to_string()
        } else {
            self.interval_buffer.clone()
        };
        let input = Para::new(Line::from(vec![
            Span::styled("Seconds: ", theme.key),
            Span::styled(text, theme.accent),
        ]))
        .block(self.themed_block(" Edit Rotation Interval ", theme))
        .alignment(Alignment::Left);
        self.render_popup(frame, popup, theme, input);
    }

    fn render_rotation_menu_overlay(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let popup = centered_rect(72, 16, area);
        frame.render_widget(Clear, popup);
        frame.render_widget(Block::default().style(theme.surface), popup);
        let block = self
            .themed_block(" Rotation Actions ", theme)
            .style(theme.surface);
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let status_lines = self
            .rotation_status_text
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), theme.accent)))
            .collect::<Vec<_>>();
        let selected = self.rotation_menu_state.selected();
        let items = RotationMenuAction::ALL
            .iter()
            .enumerate()
            .map(|(index, action)| {
                let style = if selected == Some(index) {
                    theme.highlight
                } else {
                    theme.accent
                };
                ListItem::new(action.label(self.rotation_service_state.as_ref())).style(style)
            })
            .collect::<Vec<_>>();

        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(7), Constraint::Min(0)])
            .split(inner);

        let status = Para::new(status_lines)
            .block(self.themed_block(" Service Status ", theme))
            .alignment(Alignment::Left);
        frame.render_widget(status, sections[0]);

        let mut state = self.rotation_menu_state.clone();
        let actions = List::new(items)
            .block(self.themed_block(" Actions ", theme))
            .highlight_style(theme.highlight)
            .highlight_symbol("› ");
        frame.render_stateful_widget(actions, sections[1], &mut state);
    }

    fn render_keybindings_overlay(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let popup = centered_rect(72, 12, area);
        let pairs = [
            (("Move", "↑/↓ or j/k"), ("Sections", "Tab/l, S-Tab/h")),
            (("Apply", "Enter"), ("Random", "Ctrl+r")),
            (("Rotation", "r"), ("Rotation Menu", "R")),
            (("Interval", "i"), ("Filter", "/")),
            (("Sort", "s"), ("Paths", "p")),
            (("Theme", "t"), ("Keybindings", "?")),
            (("Quit", "q / Esc"), ("Close", "? / Esc")),
        ];
        let left_label_width = pairs
            .iter()
            .map(|(left, _)| left.0.len())
            .max()
            .unwrap_or(0);
        let right_label_width = pairs
            .iter()
            .map(|(_, right)| right.0.len())
            .max()
            .unwrap_or(0);
        let left_value_width = pairs
            .iter()
            .map(|(left, _)| left.1.len())
            .max()
            .unwrap_or(0);

        let lines = pairs
            .iter()
            .map(|(left, right)| {
                Line::from(vec![
                    Span::styled(format!("{:<left_label_width$}", left.0), theme.key),
                    Span::raw("  "),
                    Span::styled(format!("{:<left_value_width$}", left.1), theme.accent),
                    Span::raw("    "),
                    Span::styled(format!("{:<right_label_width$}", right.0), theme.key),
                    Span::raw("  "),
                    Span::styled(right.1, theme.accent),
                ])
            })
            .collect::<Vec<_>>();

        let body = Para::new(lines)
            .block(self.themed_block(" Keybindings ", theme))
            .alignment(Alignment::Left);
        self.render_popup(frame, popup, theme, body);
    }

    fn render_popup<W>(&self, frame: &mut Frame, popup: Rect, theme: ThemePalette, widget: W)
    where
        W: ratatui::widgets::Widget,
    {
        frame.render_widget(Clear, popup);
        frame.render_widget(Block::default().style(theme.surface), popup);
        frame.render_widget(widget, popup);
    }

    fn render_metadata(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let block = self.themed_block(" Metadata ", theme);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(wallpaper) = self.current_selected_wallpaper() else {
            frame.render_widget(
                Para::new(Line::from(Span::styled(
                    "No wallpaper selected",
                    theme.muted,
                )))
                .alignment(Alignment::Center),
                inner,
            );
            return;
        };

        let resolution = match (wallpaper.width, wallpaper.height) {
            (Some(width), Some(height)) => format!("{width}x{height}"),
            _ => "unknown".to_string(),
        };
        let lines = vec![
            Line::from(vec![
                Span::styled("File: ", theme.key),
                Span::styled(wallpaper.name.clone(), theme.accent),
            ]),
            Line::from(vec![
                Span::styled("Dir: ", theme.key),
                Span::styled(
                    wallpaper.directory.to_string_lossy().to_string(),
                    theme.accent,
                ),
            ]),
            Line::from(vec![
                Span::styled("Resolution: ", theme.key),
                Span::styled(resolution, theme.accent),
                Span::raw("  "),
                Span::styled("Size: ", theme.key),
                Span::styled(format_file_size(wallpaper.file_size), theme.accent),
            ]),
            Line::from(vec![
                Span::styled("Modified: ", theme.key),
                Span::styled(format_timestamp(wallpaper.modified_unix_secs), theme.accent),
            ]),
            Line::from(vec![
                Span::styled("Format: ", theme.key),
                Span::styled(wallpaper.extension.to_uppercase(), theme.accent),
            ]),
        ];
        frame.render_widget(Para::new(lines), inner);
    }

    fn request_preview_load(&mut self) {
        if self.preview_area.width == 0 || self.preview_area.height == 0 {
            return;
        }

        let Some(image_path) = self
            .current_selected_wallpaper()
            .map(|wallpaper| wallpaper.path.clone())
        else {
            self.current_image = None;
            self.last_preview_target = None;
            return;
        };

        let target = (image_path.clone(), self.preview_area);
        if self.last_preview_target.as_ref() == Some(&target) {
            return;
        }
        self.last_preview_target = Some(target);

        self.preview_request_id = self.preview_request_id.wrapping_add(1);
        let _ = self.preview_tx.send(PreviewRequest {
            request_id: self.preview_request_id,
            image_path,
            area: self.preview_area,
        });

        let indices = self.section_indices(self.active_section);
        let selected = self
            .section_state(self.active_section)
            .selected()
            .unwrap_or(0);
        let prewarm_paths = indices
            .into_iter()
            .skip(selected.saturating_sub(3))
            .take(7)
            .filter_map(|index| self.wallpapers.get(index))
            .map(|wallpaper| wallpaper.path.clone())
            .collect::<Vec<_>>();
        let _ = self.prewarm_tx.send(prewarm_paths);
    }

    fn drain_preview_updates(&mut self) {
        loop {
            match self.preview_rx.try_recv() {
                Ok(response) => {
                    if response.request_id == self.preview_request_id {
                        match response.protocol {
                            Ok(protocol) => self.current_image = Some(protocol),
                            Err(error) => eprintln!("Failed to load preview: {error}"),
                        }
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

    fn drain_index_updates(&mut self) {
        loop {
            match self.index_rx.try_recv() {
                Ok(response) if response.request_id == self.index_request_id => {
                    if let Ok(wallpapers) = response.wallpapers {
                        self.wallpapers = wallpapers;
                        self.rebuild_section_cache();
                        self.ensure_section_selection();
                        self.select_active_wallpaper_in_all();
                        self.request_preview_load();
                    }
                }
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn apply_wallpaper(&mut self) -> anyhow::Result<()> {
        if let Some(path) = self
            .current_selected_wallpaper()
            .map(|wallpaper| wallpaper.path.clone())
        {
            let path_str = path.to_string_lossy().to_string();
            set_wallpaper(&path_str)?;
            self.refresh_active_wallpapers();
            println!("Wallpaper set to: {path_str}");
        }
        Ok(())
    }
}

fn spawn_index_worker(index: WallpaperIndex) -> (Sender<IndexRequest>, Receiver<IndexResponse>) {
    let (request_tx, request_rx) = mpsc::channel::<IndexRequest>();
    let (response_tx, response_rx) = mpsc::channel::<IndexResponse>();

    thread::spawn(move || {
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

fn spawn_preview_worker(
    picker: Picker,
    thumbnail_cache: Option<ThumbnailCache>,
) -> (Sender<PreviewRequest>, Receiver<PreviewResponse>) {
    let (request_tx, request_rx) = mpsc::channel::<PreviewRequest>();
    let (response_tx, response_rx) = mpsc::channel::<PreviewResponse>();

    thread::spawn(move || {
        while let Ok(mut request) = request_rx.recv() {
            while let Ok(next_request) = request_rx.try_recv() {
                request = next_request;
            }

            let protocol = build_preview_protocol(picker, thumbnail_cache.as_ref(), &request);
            let _ = response_tx.send(PreviewResponse {
                request_id: request.request_id,
                protocol,
            });
        }
    });

    (request_tx, response_rx)
}

fn spawn_prewarm_worker(thumbnail_cache: Option<ThumbnailCache>) -> Sender<Vec<PathBuf>> {
    let (request_tx, request_rx) = mpsc::channel::<Vec<PathBuf>>();

    thread::spawn(move || {
        while let Ok(mut paths) = request_rx.recv() {
            while let Ok(next_paths) = request_rx.try_recv() {
                paths = next_paths;
            }

            let Some(cache) = thumbnail_cache.as_ref() else {
                continue;
            };

            for path in paths {
                let _ = cache.generate_thumbnail(path);
            }
        }
    });

    request_tx
}

fn build_preview_protocol(
    picker: Picker,
    thumbnail_cache: Option<&ThumbnailCache>,
    request: &PreviewRequest,
) -> anyhow::Result<StatefulProtocol> {
    let preview_path = thumbnail_cache
        .and_then(|cache| cache.generate_thumbnail(&request.image_path).ok())
        .unwrap_or_else(|| request.image_path.clone());
    let dyn_img = image::open(preview_path)?;
    let mut protocol = picker.new_resize_protocol(dyn_img);
    protocol.resize_encode(&Resize::Scale(None), request.area);
    Ok(protocol)
}

fn center_rect(area: Rect, size: Rect) -> Rect {
    let width = size.width.min(area.width);
    let height = size.height.min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width, height)
}

fn centered_rect(width_percent: u16, height: u16, area: Rect) -> Rect {
    let width = area.width.saturating_mul(width_percent).saturating_div(100);
    let popup_width = width.max(24).min(area.width);
    let popup_height = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    Rect::new(x, y, popup_width, popup_height)
}

fn first_active_visible_index(
    indices: &[usize],
    wallpapers: &[IndexedWallpaper],
    active_wallpaper_paths: &HashSet<PathBuf>,
) -> Option<usize> {
    indices.iter().position(|index| {
        wallpapers
            .get(*index)
            .map(|wallpaper| active_wallpaper_paths.contains(&wallpaper.path))
            .unwrap_or(false)
    })
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

fn wallpaper_marker_prefix(is_active: bool) -> String {
    if is_active {
        "● ".to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod marker_tests {
    use super::{first_active_visible_index, wallpaper_marker_prefix};
    use crate::cache::IndexedWallpaper;
    use std::{collections::HashSet, path::PathBuf};

    fn wallpaper(name: &str) -> IndexedWallpaper {
        IndexedWallpaper {
            path: PathBuf::from(format!("/wallpapers/{name}.jpg")),
            name: name.to_string(),
            directory: PathBuf::from("/wallpapers"),
            extension: "jpg".to_string(),
            modified_unix_secs: 0,
            file_size: 0,
            width: None,
            height: None,
        }
    }

    #[test]
    fn marker_prefix_for_plain_wallpaper() {
        assert_eq!(wallpaper_marker_prefix(false), "");
    }

    #[test]
    fn marker_prefix_for_active_wallpaper() {
        assert_eq!(wallpaper_marker_prefix(true), "● ");
    }

    #[test]
    fn selects_first_active_wallpaper_in_all() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")];
        let indices = vec![0, 1, 2];
        let active_paths = HashSet::from([wallpapers[1].path.clone()]);

        assert_eq!(
            first_active_visible_index(&indices, &wallpapers, &active_paths),
            Some(1)
        );
    }

    #[test]
    fn selects_first_matching_active_wallpaper_when_multiple_are_active() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")];
        let indices = vec![2, 1, 0];
        let active_paths = HashSet::from([wallpapers[0].path.clone(), wallpapers[2].path.clone()]);

        assert_eq!(
            first_active_visible_index(&indices, &wallpapers, &active_paths),
            Some(0)
        );
    }

    #[test]
    fn returns_none_when_active_wallpaper_is_not_indexed() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta")];
        let indices = vec![0, 1];
        let active_paths = HashSet::from([PathBuf::from("/wallpapers/missing.jpg")]);

        assert_eq!(
            first_active_visible_index(&indices, &wallpapers, &active_paths),
            None
        );
    }
}
