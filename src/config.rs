use std::fs;
use std::path::PathBuf;

use anyhow::bail;
use serde::{Deserialize, Serialize};

const CONFIG_DIR: &str = "walt";
const LEGACY_CONFIG_DIR: &str = "wallpaper-switcher";
const PATHS_FILE: &str = "paths.conf";
const THEME_FILE: &str = "theme.conf";
const STATE_FILE: &str = "state.json";

#[derive(Clone)]
pub struct Config {
    pub wallpaper_paths: Vec<PathBuf>,
    pub theme_name: String,
    pub favorites: Vec<PathBuf>,
    pub rotation: Vec<PathBuf>,
    pub rotation_interval_secs: u64,
    pub all_sort: String,
    pub favorites_sort: String,
    pub rotation_sort: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    wallpaper_paths: Vec<PathBuf>,
    theme_name: String,
    favorites: Vec<PathBuf>,
    rotation: Vec<PathBuf>,
    rotation_interval_secs: u64,
    all_sort: String,
    favorites_sort: String,
    rotation_sort: String,
}

impl Config {
    pub fn new() -> Self {
        let base_config_dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_dir = base_config_dir.join(CONFIG_DIR);
        let state_file = config_dir.join(STATE_FILE);

        if state_file.exists() {
            return Self::from_state_file(&state_file);
        }

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
            favorites: vec![],
            rotation: vec![],
            rotation_interval_secs: 300,
            all_sort: "name".to_string(),
            favorites_sort: "name".to_string(),
            rotation_sort: "name".to_string(),
        }
    }

    fn from_state_file(path: &PathBuf) -> Self {
        let state = fs::read_to_string(path)
            .ok()
            .and_then(|content| serde_json::from_str::<ConfigFile>(&content).ok())
            .unwrap_or_default();

        Self {
            wallpaper_paths: state
                .wallpaper_paths
                .into_iter()
                .filter(|path| path.is_dir())
                .collect(),
            theme_name: if state.theme_name.trim().is_empty() {
                "System".to_string()
            } else {
                state.theme_name
            },
            favorites: state.favorites,
            rotation: state.rotation,
            rotation_interval_secs: if state.rotation_interval_secs == 0 {
                300
            } else {
                state.rotation_interval_secs
            },
            all_sort: default_sort_name(state.all_sort),
            favorites_sort: default_sort_name(state.favorites_sort),
            rotation_sort: default_sort_name(state.rotation_sort),
        }
    }

    fn load_paths(paths_file: &PathBuf) -> Vec<PathBuf> {
        let content = fs::read_to_string(paths_file).unwrap_or_default();
        content
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
            .map(|line| PathBuf::from(line.trim()))
            .filter(|path| path.is_dir())
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

        let state = ConfigFile {
            wallpaper_paths: self.wallpaper_paths.clone(),
            theme_name: self.theme_name.clone(),
            favorites: self.favorites.clone(),
            rotation: self.rotation.clone(),
            rotation_interval_secs: self.rotation_interval_secs,
            all_sort: default_sort_name(self.all_sort.clone()),
            favorites_sort: default_sort_name(self.favorites_sort.clone()),
            rotation_sort: default_sort_name(self.rotation_sort.clone()),
        };
        fs::write(
            config_dir.join(STATE_FILE),
            serde_json::to_vec_pretty(&state)?,
        )?;

        let paths_content = self
            .wallpaper_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(config_dir.join(PATHS_FILE), paths_content)?;
        fs::write(
            config_dir.join(THEME_FILE),
            format!("{}\n", self.theme_name),
        )?;

        Ok(())
    }

    pub fn add_path(&mut self, path: PathBuf) {
        if path.is_dir() && !self.wallpaper_paths.contains(&path) {
            self.wallpaper_paths.push(path);
        }
    }

    pub fn remove_path(&mut self, path: &PathBuf) {
        self.wallpaper_paths.retain(|entry| entry != path);
        self.favorites.retain(|entry| entry != path);
        self.rotation.retain(|entry| entry != path);
    }

    pub fn set_theme<S: Into<String>>(&mut self, theme_name: S) {
        self.theme_name = theme_name.into();
    }

    pub fn toggle_favorite(&mut self, path: &PathBuf) -> bool {
        if let Some(index) = self.favorites.iter().position(|entry| entry == path) {
            self.favorites.remove(index);
            false
        } else {
            self.favorites.push(path.clone());
            true
        }
    }

    pub fn toggle_rotation(&mut self, path: &PathBuf) -> bool {
        if let Some(index) = self.rotation.iter().position(|entry| entry == path) {
            self.rotation.remove(index);
            false
        } else {
            self.rotation.push(path.clone());
            true
        }
    }

    pub fn is_in_rotation(&self, path: &PathBuf) -> bool {
        self.rotation.iter().any(|entry| entry == path)
    }

    pub fn sort_name_for_section(&self, section: &str) -> &str {
        match section {
            "favorites" => &self.favorites_sort,
            "rotation" => &self.rotation_sort,
            _ => &self.all_sort,
        }
    }

    pub fn set_sort_name_for_section(&mut self, section: &str, value: &str) {
        let value = default_sort_name(value.to_string());
        match section {
            "favorites" => self.favorites_sort = value,
            "rotation" => self.rotation_sort = value,
            _ => self.all_sort = value,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.wallpaper_paths.is_empty()
    }

    pub fn set_rotation_interval_secs(&mut self, seconds: u64) -> anyhow::Result<()> {
        if seconds == 0 {
            bail!("Rotation interval must be greater than 0 seconds.");
        }

        self.rotation_interval_secs = seconds;
        self.save()
    }
}

fn default_sort_name(name: String) -> String {
    match name.as_str() {
        "modified" => "modified".to_string(),
        _ => "name".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use std::path::PathBuf;

    fn test_config() -> Config {
        Config {
            wallpaper_paths: vec![],
            theme_name: "System".to_string(),
            favorites: vec![],
            rotation: vec![],
            rotation_interval_secs: 300,
            all_sort: "name".to_string(),
            favorites_sort: "name".to_string(),
            rotation_sort: "name".to_string(),
        }
    }

    #[test]
    fn toggles_rotation_membership() {
        let path = PathBuf::from("/tmp/wallpaper.jpg");
        let mut config = test_config();

        assert!(config.toggle_rotation(&path));
        assert_eq!(config.rotation, vec![path.clone()]);

        assert!(!config.toggle_rotation(&path));
        assert!(config.rotation.is_empty());
    }

    #[test]
    fn rejects_zero_rotation_interval() {
        let mut config = test_config();

        let error = config
            .set_rotation_interval_secs(0)
            .expect_err("zero seconds should fail");
        assert_eq!(
            error.to_string(),
            "Rotation interval must be greater than 0 seconds."
        );
    }
}
