use std::fs;
use std::path::PathBuf;

const CONFIG_DIR: &str = "walt";
const LEGACY_CONFIG_DIR: &str = "wallpaper-switcher";
const PATHS_FILE: &str = "paths.conf";
const THEME_FILE: &str = "theme.conf";

pub struct Config {
    pub wallpaper_paths: Vec<PathBuf>,
    pub theme_name: String,
}

impl Config {
    pub fn new() -> Self {
        let base_config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_dir = base_config_dir.join(CONFIG_DIR);
        let legacy_config_dir = base_config_dir.join(LEGACY_CONFIG_DIR);

        let paths_file = config_dir.join(PATHS_FILE);
        let theme_file = config_dir.join(THEME_FILE);
        let legacy_paths_file = legacy_config_dir.join(PATHS_FILE);
        let legacy_theme_file = legacy_config_dir.join(THEME_FILE);

        let wallpaper_paths = if paths_file.exists() {
            Self::load_paths(&paths_file)
        } else if legacy_paths_file.exists() {
            Self::load_paths(&legacy_paths_file)
        } else {
            vec![]
        };

        let theme_name = if theme_file.exists() {
            Self::load_theme(&theme_file)
        } else if legacy_theme_file.exists() {
            Self::load_theme(&legacy_theme_file)
        } else {
            "System".to_string()
        };

        Self {
            wallpaper_paths,
            theme_name,
        }
    }

    fn load_paths(paths_file: &PathBuf) -> Vec<PathBuf> {
        let content = fs::read_to_string(paths_file).unwrap_or_default();
        content
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
            .map(|line| PathBuf::from(line.trim()))
            .filter(|p| p.is_dir())
            .collect()
    }

    fn load_theme(theme_file: &PathBuf) -> String {
        let theme_name = fs::read_to_string(theme_file).unwrap_or_default();
        let theme_name = theme_name.trim();

        if theme_name.is_empty() {
            "System".to_string()
        } else {
            theme_name.to_string()
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find config directory"))?
            .join(CONFIG_DIR);

        fs::create_dir_all(&config_dir)?;

        let paths_file = config_dir.join(PATHS_FILE);
        let content = self
            .wallpaper_paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        fs::write(paths_file, content)?;
        fs::write(config_dir.join(THEME_FILE), format!("{}\n", self.theme_name))?;

        Ok(())
    }

    pub fn add_path(&mut self, path: PathBuf) {
        if path.is_dir() && !self.wallpaper_paths.contains(&path) {
            self.wallpaper_paths.push(path);
        }
    }

    pub fn remove_path(&mut self, path: &PathBuf) {
        self.wallpaper_paths.retain(|p| p != path);
    }

    pub fn set_theme<S: Into<String>>(&mut self, theme_name: S) {
        self.theme_name = theme_name.into();
    }

    pub fn is_empty(&self) -> bool {
        self.wallpaper_paths.is_empty()
    }
}
