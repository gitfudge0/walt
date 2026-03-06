mod theme;

use rand::Rng;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    crossterm::{
        event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    symbols::border,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph as Para},
    Frame, Terminal,
};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol, Resize, StatefulImage};
use std::{
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
};

use crate::backend::{scan_directory, scan_wallpapers_from_paths, set_wallpaper, Wallpaper};
use crate::cache::ThumbnailCache;
use crate::config::Config;
use theme::{ThemeKind, ThemePalette};

#[derive(Clone, Copy, PartialEq)]
enum AppMode {
    Setup,
    PathManage,
    Wallpaper,
    ThemeSelect,
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

pub struct App {
    config: Config,
    theme: ThemeKind,
    theme_before_picker: ThemeKind,
    theme_return_mode: AppMode,
    wallpapers: Vec<Wallpaper>,
    list_state: ListState,
    theme_state: ListState,
    selected_index: usize,
    mode: AppMode,
    preview_area: Rect,
    preview_request_id: u64,
    preview_tx: Sender<PreviewRequest>,
    preview_rx: Receiver<PreviewResponse>,
    current_image: Option<StatefulProtocol>,
    input_buffer: String,
    dir_suggestions: Vec<PathBuf>,
    suggestion_state: ListState,
}

impl App {
    pub fn new() -> anyhow::Result<Self> {
        let config = Config::new();
        let theme = ThemeKind::from_name(&config.theme_name);
        let picker = Picker::from_query_stdio()?;

        let wallpapers = if config.is_empty() {
            vec![]
        } else {
            scan_wallpapers_from_paths(&config.wallpaper_paths)
        };

        let mut list_state = ListState::default();
        if !wallpapers.is_empty() {
            list_state.select(Some(0));
        }

        let mut suggestion_state = ListState::default();
        suggestion_state.select(Some(0));
        let mut theme_state = ListState::default();
        theme_state.select(Some(theme.index()));

        let mode = if config.is_empty() {
            AppMode::Setup
        } else {
            AppMode::Wallpaper
        };
        let (preview_tx, preview_rx) = spawn_preview_worker(picker, ThumbnailCache::new().ok());

        let mut app = Self {
            config,
            theme,
            theme_before_picker: theme,
            theme_return_mode: mode,
            wallpapers,
            list_state,
            theme_state,
            selected_index: 0,
            mode,
            preview_area: Rect::default(),
            preview_request_id: 0,
            preview_tx,
            preview_rx,
            current_image: None,
            input_buffer: String::new(),
            dir_suggestions: vec![],
            suggestion_state,
        };

        if !app.wallpapers.is_empty() && app.mode == AppMode::Wallpaper {
            app.request_preview_load(0);
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
            terminal.draw(|f| self.ui(f))?;

            if event::poll(std::time::Duration::from_millis(16))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match self.mode {
                            AppMode::Setup => self.handle_setup_key(key.code)?,
                            AppMode::PathManage => self.handle_path_manage_key(key.code),
                            AppMode::ThemeSelect => self.handle_theme_select_key(key.code),
                            AppMode::Wallpaper => {
                                if self.handle_wallpaper_key(key.code)? {
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
                self.input_buffer = String::new();
                self.update_suggestions();
                self.mode = AppMode::Setup;
            }
            KeyCode::Char('d') => {
                if let Some(idx) = self.list_state.selected() {
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

    fn handle_wallpaper_key(&mut self, key: KeyCode) -> io::Result<bool> {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
            KeyCode::Char('p') => self.mode = AppMode::PathManage,
            KeyCode::Char('r') => self.select_random_wallpaper()?,
            KeyCode::Char('t') => self.open_theme_picker(),
            KeyCode::Char('j') | KeyCode::Down => self.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.move_up(),
            KeyCode::Char('g') | KeyCode::Home => self.go_to_top(),
            KeyCode::Char('G') | KeyCode::End => self.go_to_bottom(),
            KeyCode::Enter => self.handle_enter()?,
            _ => {}
        }

        Ok(false)
    }

    fn select_random_wallpaper(&mut self) -> io::Result<()> {
        if self.wallpapers.is_empty() {
            return Ok(());
        }

        let random_index = rand::thread_rng().gen_range(0..self.wallpapers.len());
        self.list_state.select(Some(random_index));
        self.selected_index = random_index;
        self.request_preview_load(random_index);

        if let Err(error) = self.apply_wallpaper() {
            eprintln!("Failed to apply random wallpaper: {}", error);
        } else {
            return Ok(());
        }

        Ok(())
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
                    self.input_buffer = String::new();
                    self.dir_suggestions = vec![];
                    self.mode = AppMode::Wallpaper;
                    self.refresh_wallpapers();
                }
            }
            AppMode::Wallpaper => {
                if let Err(e) = self.apply_wallpaper() {
                    eprintln!("Failed to apply: {}", e);
                } else {
                    return Ok(());
                }
            }
            AppMode::ThemeSelect => self.confirm_theme_picker(),
            AppMode::PathManage => {}
        }
        Ok(())
    }

    fn move_down(&mut self) {
        match self.mode {
            AppMode::Setup => {
                if !self.dir_suggestions.is_empty() {
                    let i = self.suggestion_state.selected().unwrap_or(0);
                    self.suggestion_state
                        .select(Some((i + 1) % self.dir_suggestions.len()));
                }
            }
            AppMode::ThemeSelect => {
                let i = self.theme_state.selected().unwrap_or(self.theme.index());
                let new_i = (i + 1) % ThemeKind::ALL.len();
                self.theme_state.select(Some(new_i));
                self.theme = ThemeKind::ALL[new_i];
            }
            AppMode::PathManage | AppMode::Wallpaper => {
                let len = if self.mode == AppMode::PathManage {
                    self.config.wallpaper_paths.len()
                } else {
                    self.wallpapers.len()
                };
                if len > 0 {
                    let i = self.list_state.selected().unwrap_or(0);
                    let new_i = (i + 1) % len;
                    self.list_state.select(Some(new_i));
                    self.selected_index = new_i;
                    if self.mode == AppMode::Wallpaper {
                        self.request_preview_load(new_i);
                    }
                }
            }
        }
    }

    fn move_up(&mut self) {
        match self.mode {
            AppMode::Setup => {
                if !self.dir_suggestions.is_empty() {
                    let i = self.suggestion_state.selected().unwrap_or(0);
                    self.suggestion_state.select(Some(if i == 0 {
                        self.dir_suggestions.len() - 1
                    } else {
                        i - 1
                    }));
                }
            }
            AppMode::ThemeSelect => {
                let i = self.theme_state.selected().unwrap_or(self.theme.index());
                let new_i = if i == 0 { ThemeKind::ALL.len() - 1 } else { i - 1 };
                self.theme_state.select(Some(new_i));
                self.theme = ThemeKind::ALL[new_i];
            }
            AppMode::PathManage | AppMode::Wallpaper => {
                let len = if self.mode == AppMode::PathManage {
                    self.config.wallpaper_paths.len()
                } else {
                    self.wallpapers.len()
                };
                if len > 0 {
                    let i = self.list_state.selected().unwrap_or(0);
                    let new_i = if i == 0 { len - 1 } else { i - 1 };
                    self.list_state.select(Some(new_i));
                    self.selected_index = new_i;
                    if self.mode == AppMode::Wallpaper {
                        self.request_preview_load(new_i);
                    }
                }
            }
        }
    }

    fn go_to_top(&mut self) {
        match self.mode {
            AppMode::Setup => self.suggestion_state.select(Some(0)),
            AppMode::ThemeSelect => {
                self.theme_state.select(Some(0));
                self.theme = ThemeKind::ALL[0];
            }
            AppMode::PathManage | AppMode::Wallpaper => {
                let len = if self.mode == AppMode::PathManage {
                    self.config.wallpaper_paths.len()
                } else {
                    self.wallpapers.len()
                };
                if len > 0 {
                    self.list_state.select(Some(0));
                    self.selected_index = 0;
                    if self.mode == AppMode::Wallpaper {
                        self.request_preview_load(0);
                    }
                }
            }
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
            AppMode::PathManage | AppMode::Wallpaper => {
                let len = if self.mode == AppMode::PathManage {
                    self.config.wallpaper_paths.len()
                } else {
                    self.wallpapers.len()
                };
                if len > 0 {
                    let last = len - 1;
                    self.list_state.select(Some(last));
                    self.selected_index = last;
                    if self.mode == AppMode::Wallpaper {
                        self.request_preview_load(last);
                    }
                }
            }
        }
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
            self.wallpapers = vec![];
            self.mode = AppMode::Setup;
        } else {
            self.wallpapers = scan_wallpapers_from_paths(&self.config.wallpaper_paths);
            self.list_state.select(if self.wallpapers.is_empty() {
                None
            } else {
                Some(0)
            });
            self.selected_index = 0;
            if !self.wallpapers.is_empty() {
                self.request_preview_load(0);
            }
        }
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
            AppMode::Wallpaper => {
                let main_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                    .split(chunks[0]);
                let preview_area = self.themed_block(" Preview ", theme).inner(main_chunks[1]);
                if preview_area != self.preview_area {
                    self.preview_area = preview_area;
                    if !self.wallpapers.is_empty() {
                        self.request_preview_load(self.selected_index);
                    }
                }
                self.render_wallpaper_list(frame, main_chunks[0], theme);
                self.render_preview(frame, main_chunks[1], theme);
            }
        }

        self.render_help(frame, chunks[1], theme);
    }

    fn themed_block<'a>(&self, title: &'a str, theme: ThemePalette) -> Block<'a> {
        Block::default()
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
        let cursor = if self.input_buffer.is_empty() { "_" } else { "" };
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
            let items: Vec<ListItem> = self
                .dir_suggestions
                .iter()
                .map(|p| ListItem::new(p.to_string_lossy().to_string()).style(theme.accent))
                .collect();
            let mut ss = self.suggestion_state.clone();
            let list = List::new(items)
                .block(self.themed_block(" Directories ", theme))
                .highlight_style(theme.highlight)
                .highlight_symbol("▶ ");
            frame.render_stateful_widget(list, list_area, &mut ss);
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
            let items: Vec<ListItem> = self
                .config
                .wallpaper_paths
                .iter()
                .map(|p| ListItem::new(p.to_string_lossy().to_string()).style(theme.accent))
                .collect();
            let mut ls = self.list_state.clone();
            let list = List::new(items)
                .block(self.themed_block(" Paths ", theme))
                .highlight_style(theme.highlight)
                .highlight_symbol("▶ ");
            frame.render_stateful_widget(list, inner, &mut ls);
        }
    }

    fn render_theme_picker(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(theme.surface), area);
        let outer = self.themed_block(" Theme Picker ", theme).style(theme.surface);
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

        let items: Vec<ListItem> = ThemeKind::ALL
            .iter()
            .map(|theme_kind| ListItem::new(theme_kind.name()).style(theme.accent))
            .collect();
        let mut state = self.theme_state.clone();
        let list = List::new(items)
            .block(self.themed_block(" Themes ", theme))
            .style(theme.surface)
            .highlight_style(theme.highlight)
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, content[1], &mut state);
    }

    fn render_wallpaper_list(&mut self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let items: Vec<ListItem> = self
            .wallpapers
            .iter()
            .map(|w| ListItem::new(w.name.clone()).style(theme.accent))
            .collect();
        let list = List::new(items)
            .block(self.themed_block(" Wallpapers ", theme))
            .highlight_style(theme.highlight)
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_preview(&mut self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let block = self.themed_block(" Preview ", theme);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(Block::default().style(theme.surface), inner);

        if let Some(ref mut protocol) = self.current_image {
            let resize = Resize::Scale(None);
            let render_area = center_rect(inner, protocol.size_for(&resize, inner));
            let img = StatefulImage::default().resize(resize);
            frame.render_stateful_widget(img, render_area, protocol);
        } else {
            let content = if self.wallpapers.is_empty() {
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
            frame.render_widget(Para::new(content).alignment(Alignment::Center), inner);
        }
    }

    fn render_help(&self, frame: &mut Frame, area: Rect, theme: ThemePalette) {
        let (mode_text, controls) = match self.mode {
            AppMode::Setup => (
                " Setup ",
                vec![
                    ("type", "enter path"),
                    ("↑/↓", "navigate"),
                    ("Tab", "select"),
                    ("Enter", "add path"),
                ],
            ),
            AppMode::PathManage => (
                " Path Manager ",
                vec![
                    ("p", "wallpapers"),
                    ("a", "add path"),
                    ("d", "remove"),
                    ("↑/↓", "navigate"),
                    ("t", "theme"),
                ],
            ),
            AppMode::ThemeSelect => (
                " Theme Picker ",
                vec![
                    ("↑/↓/j/k", "preview"),
                    ("g/G", "top/bottom"),
                    ("Enter", "confirm"),
                    ("Esc/q", "cancel"),
                ],
            ),
            AppMode::Wallpaper => (
                " Wallpapers ",
                vec![
                    ("p", "paths"),
                    ("r", "random"),
                    ("↑/↓/j/k", "navigate"),
                    ("g/G", "top/bottom"),
                    ("Enter", "apply"),
                    ("t", "theme"),
                    ("q", "quit"),
                ],
            ),
        };

        let mut spans = vec![
            Span::raw(" "),
            Span::styled(mode_text, theme.title),
            Span::raw(" | "),
            Span::styled("Theme", theme.key),
            Span::raw(": "),
            Span::styled(self.theme.name(), theme.accent),
            Span::raw(" | "),
        ];

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

    fn request_preview_load(&mut self, index: usize) {
        if self.preview_area.width == 0 || self.preview_area.height == 0 {
            return;
        }

        let Some(wallpaper) = self.wallpapers.get(index) else {
            return;
        };

        self.preview_request_id = self.preview_request_id.wrapping_add(1);
        let _ = self.preview_tx.send(PreviewRequest {
            request_id: self.preview_request_id,
            image_path: wallpaper.path.clone(),
            area: self.preview_area,
        });
    }

    fn drain_preview_updates(&mut self) {
        loop {
            match self.preview_rx.try_recv() {
                Ok(response) => {
                    if response.request_id == self.preview_request_id {
                        match response.protocol {
                            Ok(protocol) => self.current_image = Some(protocol),
                            Err(error) => eprintln!("Failed to load preview: {}", error),
                        }
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn apply_wallpaper(&self) -> anyhow::Result<()> {
        if let Some(wallpaper) = self.wallpapers.get(self.selected_index) {
            let path_str = wallpaper.path.to_string_lossy().to_string();
            set_wallpaper(&path_str)?;
            println!("Wallpaper set to: {}", path_str);
        }
        Ok(())
    }
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
