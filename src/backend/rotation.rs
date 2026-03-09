use std::{fs, path::PathBuf, process::Command, thread, time::Duration};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    cache::{IndexedWallpaper, WallpaperIndex},
    config::Config,
};

use super::set_wallpaper;

const CONFIG_DIR: &str = "walt";
const ROTATION_STATE_FILE: &str = "rotation-state.json";
const SERVICE_FILE: &str = "walt-rotation.service";
const BENIGN_DISABLE_ERRORS: &[&str] = &[
    "unit file walt-rotation.service does not exist",
    "unit walt-rotation.service does not exist",
    "unit walt-rotation.service not loaded",
    "no files found for walt-rotation.service",
    "unit walt-rotation.service could not be found",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SortMode {
    Name,
    Modified,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RotationState {
    last_wallpaper: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationServiceStatus {
    pub installed: bool,
    pub enabled: String,
    pub active: String,
    pub rotates_all_wallpapers: bool,
    pub interval_secs: u64,
    pub rotation_entries: usize,
    pub service_file: PathBuf,
}

pub fn install_rotation_service() -> Result<()> {
    let service_dir = service_dir()?;
    fs::create_dir_all(&service_dir)?;
    fs::write(service_file_path()?, service_file_contents()?)?;
    enable_rotation_service()
}

pub fn enable_rotation_service() -> Result<()> {
    if !service_file_path()?.exists() {
        bail!("Rotation service is not installed. Run 'walt rotation install' first.");
    }

    run_systemctl(&["daemon-reload"])?;
    run_systemctl(&["enable", "--now", SERVICE_FILE])
}

pub fn disable_rotation_service() -> Result<()> {
    run_systemctl_checked(&["disable", "--now", SERVICE_FILE], BENIGN_DISABLE_ERRORS)
}

pub fn restart_rotation_service_if_active() -> Result<()> {
    let service_path = service_file_path()?;
    if !service_path.exists() {
        return Ok(());
    }

    let active = read_systemctl_state(&["is-active", SERVICE_FILE], "inactive", "activity")?;
    if active != "active" {
        return Ok(());
    }

    run_systemctl(&["restart", SERVICE_FILE])
}

pub fn uninstall_rotation_service() -> Result<()> {
    let service_path = service_file_path()?;
    disable_rotation_service()?;
    if service_path.exists() {
        fs::remove_file(service_path)?;
    }
    run_systemctl(&["daemon-reload"])?;
    Ok(())
}

pub fn rotation_service_file_path() -> Result<PathBuf> {
    service_file_path()
}

pub fn get_rotation_service_status() -> Result<RotationServiceStatus> {
    let config = Config::new();
    let service_file = service_file_path()?;
    let installed = service_file.exists();
    let enabled = if installed {
        read_systemctl_state(&["is-enabled", SERVICE_FILE], "disabled", "enablement")?
    } else {
        "not installed".to_string()
    };
    let active = if installed {
        read_systemctl_state(&["is-active", SERVICE_FILE], "inactive", "activity")?
    } else {
        "inactive".to_string()
    };

    Ok(RotationServiceStatus {
        installed,
        enabled,
        active,
        rotates_all_wallpapers: config.uses_all_wallpapers_for_rotation(),
        interval_secs: config.rotation_interval_secs,
        rotation_entries: config.rotation.len(),
        service_file,
    })
}

pub fn rotation_service_status() -> Result<String> {
    Ok(format_rotation_service_status(
        &get_rotation_service_status()?,
    ))
}

pub fn run_rotation_daemon() -> Result<()> {
    let index = WallpaperIndex::new()?;

    loop {
        let config = Config::new();
        let candidates = rotation_candidates(&index, &config);
        let interval = Duration::from_secs(config.rotation_interval_secs.max(1));

        if candidates.is_empty() {
            thread::sleep(interval.min(Duration::from_secs(30)));
            continue;
        }

        let mut state = load_rotation_state()?;
        let next = next_wallpaper(&candidates, state.last_wallpaper.as_ref())
            .cloned()
            .context("Could not determine next rotation wallpaper")?;
        set_wallpaper(&next.path.to_string_lossy())?;
        state.last_wallpaper = Some(next.path.clone());
        save_rotation_state(&state)?;
        thread::sleep(interval);
    }
}

fn rotation_candidates(index: &WallpaperIndex, config: &Config) -> Vec<IndexedWallpaper> {
    let wallpapers = index
        .refresh(&config.wallpaper_paths)
        .unwrap_or_else(|_| index.load(&config.wallpaper_paths));
    let mut wallpapers = filter_wallpapers_for_rotation(wallpapers, config);

    let sort_mode = match config.rotation_sort.as_str() {
        "modified" => SortMode::Modified,
        _ => SortMode::Name,
    };

    wallpapers.sort_by(|left, right| match sort_mode {
        SortMode::Name => left
            .name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.path.cmp(&right.path)),
        SortMode::Modified => right
            .modified_unix_secs
            .cmp(&left.modified_unix_secs)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase())),
    });

    wallpapers
}

fn filter_wallpapers_for_rotation(
    mut wallpapers: Vec<IndexedWallpaper>,
    config: &Config,
) -> Vec<IndexedWallpaper> {
    if config.uses_all_wallpapers_for_rotation() {
        return wallpapers;
    }

    wallpapers.retain(|wallpaper| config.is_in_rotation(&wallpaper.path));
    wallpapers
}

fn next_wallpaper<'a>(
    wallpapers: &'a [IndexedWallpaper],
    last_wallpaper: Option<&PathBuf>,
) -> Option<&'a IndexedWallpaper> {
    if wallpapers.is_empty() {
        return None;
    }

    let next_index = last_wallpaper
        .and_then(|last| {
            wallpapers
                .iter()
                .position(|wallpaper| &wallpaper.path == last)
        })
        .map(|index| (index + 1) % wallpapers.len())
        .unwrap_or(0);

    wallpapers.get(next_index)
}

fn run_systemctl(args: &[&str]) -> Result<()> {
    run_systemctl_checked(args, &[])
}

fn read_systemctl_state(args: &[&str], fallback: &str, label: &str) -> Result<String> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .with_context(|| format!("Failed to query systemd user service {label}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !stdout.is_empty() {
        return Ok(stdout);
    }

    if output.status.success() {
        return Ok(fallback.to_string());
    }

    if stderr.is_empty() {
        return Ok(fallback.to_string());
    }

    Err(anyhow::anyhow!(
        "Failed to query systemd user service {label}: {stderr}"
    ))
}

fn run_systemctl_checked(args: &[&str], tolerated_failures: &[&str]) -> Result<()> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .with_context(|| format!("Failed to run systemctl --user {}", args.join(" ")))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if !stderr.is_empty() { stderr } else { stdout };

    if is_tolerated_systemctl_failure(&details, tolerated_failures) {
        return Ok(());
    }

    Err(anyhow::anyhow!(
        "systemctl --user {} failed: {}",
        args.join(" "),
        details
    ))
}

fn is_tolerated_systemctl_failure(details: &str, tolerated_failures: &[&str]) -> bool {
    let details = details.to_lowercase();
    tolerated_failures
        .iter()
        .any(|fragment| details.contains(fragment))
}

fn format_rotation_service_status(status: &RotationServiceStatus) -> String {
    let loaded = if status.installed {
        format!("loaded ({})", status.service_file.display())
    } else {
        format!("not found ({})", status.service_file.display())
    };
    let summary = summarize_rotation_service(status);

    format!(
        "Rotation Service\n\
Status:   {summary}\n\
Loaded:   {loaded}\n\
Enabled:  {}\n\
Active:   {}\n\
Mode:     {}\n\
Interval: {}\n\
Entries:  {}",
        status.enabled,
        status.active,
        rotation_mode_label(status.rotates_all_wallpapers),
        format_interval(status.interval_secs),
        rotation_entries_label(status)
    )
}

fn summarize_rotation_service(status: &RotationServiceStatus) -> &'static str {
    if !status.installed {
        return "not installed";
    }

    match (status.enabled.as_str(), status.active.as_str()) {
        ("enabled", "active") => "running",
        ("enabled", _) => "installed, enabled, not running",
        (_, "active") => "running without boot-time enablement",
        _ => "installed, not running",
    }
}

pub fn rotation_service_badge(status: &RotationServiceStatus) -> &'static str {
    if !status.installed {
        return "not installed";
    }

    if status.active == "active" {
        "active"
    } else {
        "disabled"
    }
}

fn rotation_mode_label(rotates_all_wallpapers: bool) -> &'static str {
    if rotates_all_wallpapers {
        "all wallpapers"
    } else {
        "selected wallpapers"
    }
}

fn rotation_entries_label(status: &RotationServiceStatus) -> String {
    if status.rotates_all_wallpapers {
        "all wallpapers".to_string()
    } else {
        pluralize_wallpapers(status.rotation_entries)
    }
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

fn pluralize_wallpapers(count: usize) -> String {
    if count == 1 {
        "1 wallpaper".to_string()
    } else {
        format!("{count} wallpapers")
    }
}

fn service_file_contents() -> Result<String> {
    let executable = std::env::current_exe()?.display().to_string();
    Ok(format!(
        "[Unit]\nDescription=Walt wallpaper rotation service\nAfter=graphical-session.target\n\n[Service]\nType=simple\nExecStart={executable} --rotate-daemon\nRestart=always\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n"
    ))
}

fn load_rotation_state() -> Result<RotationState> {
    let path = rotation_state_path()?;
    if !path.exists() {
        return Ok(RotationState::default());
    }

    let content = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&content).unwrap_or_default())
}

fn save_rotation_state(state: &RotationState) -> Result<()> {
    let path = rotation_state_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

fn rotation_state_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(ROTATION_STATE_FILE))
}

fn service_file_path() -> Result<PathBuf> {
    Ok(service_dir()?.join(SERVICE_FILE))
}

fn service_dir() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("Could not find config directory")?
        .join("systemd")
        .join("user"))
}

fn config_dir() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("Could not find config directory")?
        .join(CONFIG_DIR))
}

#[cfg(test)]
mod tests {
    use super::{
        filter_wallpapers_for_rotation, format_rotation_service_status,
        is_tolerated_systemctl_failure, next_wallpaper, service_file_contents,
        RotationServiceStatus,
    };
    use crate::cache::IndexedWallpaper;
    use crate::config::Config;
    use std::path::PathBuf;

    fn wallpaper(name: &str) -> IndexedWallpaper {
        IndexedWallpaper {
            path: PathBuf::from(format!("/tmp/{name}.png")),
            name: name.to_string(),
            directory: PathBuf::from("/tmp"),
            extension: "png".to_string(),
            modified_unix_secs: 0,
            file_size: 0,
            width: None,
            height: None,
        }
    }

    fn config(rotation: Vec<&str>, rotate_all_wallpapers: bool) -> Config {
        Config {
            wallpaper_paths: vec![],
            theme_name: "System".to_string(),
            rotation: rotation
                .into_iter()
                .map(|name| PathBuf::from(format!("/tmp/{name}.png")))
                .collect(),
            rotate_all_wallpapers,
            rotation_interval_secs: 300,
            all_sort: "name".to_string(),
            rotation_sort: "name".to_string(),
        }
    }

    #[test]
    fn wraps_to_first_wallpaper_when_last_is_end_of_list() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta")];
        let next = next_wallpaper(&wallpapers, Some(&wallpapers[1].path)).expect("next wallpaper");
        assert_eq!(next.name, "alpha");
    }

    #[test]
    fn keeps_only_selected_wallpapers_when_rotate_all_is_off() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")];
        let filtered = filter_wallpapers_for_rotation(wallpapers, &config(vec!["beta"], false));

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "beta");
    }

    #[test]
    fn keeps_all_wallpapers_when_rotate_all_is_on() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")];
        let filtered = filter_wallpapers_for_rotation(wallpapers, &config(vec!["beta"], true));

        assert_eq!(filtered.len(), 3);
        assert_eq!(filtered[0].name, "alpha");
        assert_eq!(filtered[1].name, "beta");
        assert_eq!(filtered[2].name, "gamma");
    }

    #[test]
    fn service_unit_runs_rotate_daemon() {
        let unit = service_file_contents().expect("service file");
        assert!(unit.contains("--rotate-daemon"));
        assert!(unit.contains("Restart=always"));
    }

    #[test]
    fn tolerates_disable_errors_for_missing_units_only() {
        assert!(is_tolerated_systemctl_failure(
            "Failed to disable unit: Unit file walt-rotation.service does not exist.",
            &[
                "unit file walt-rotation.service does not exist",
                "unit walt-rotation.service does not exist",
            ]
        ));
        assert!(is_tolerated_systemctl_failure(
            "Failed to disable unit: Unit walt-rotation.service does not exist",
            &[
                "unit file walt-rotation.service does not exist",
                "unit walt-rotation.service does not exist",
            ]
        ));
        assert!(!is_tolerated_systemctl_failure(
            "Failed to connect to bus: No medium found",
            &[
                "unit file walt-rotation.service does not exist",
                "unit walt-rotation.service does not exist",
            ]
        ));
    }

    #[test]
    fn formats_rotation_status_for_installed_service() {
        let text = format_rotation_service_status(&RotationServiceStatus {
            installed: true,
            enabled: "enabled".to_string(),
            active: "active".to_string(),
            rotates_all_wallpapers: false,
            interval_secs: 300,
            rotation_entries: 4,
            service_file: PathBuf::from("/tmp/walt-rotation.service"),
        });

        assert!(text.contains("Rotation Service"));
        assert!(text.contains("Status:   running"));
        assert!(text.contains("Loaded:   loaded (/tmp/walt-rotation.service)"));
        assert!(text.contains("Enabled:  enabled"));
        assert!(text.contains("Active:   active"));
        assert!(text.contains("Mode:     selected wallpapers"));
        assert!(text.contains("Interval: 300s (5m)"));
        assert!(text.contains("Entries:  4 wallpapers"));
    }

    #[test]
    fn formats_rotation_status_for_missing_service() {
        let text = format_rotation_service_status(&RotationServiceStatus {
            installed: false,
            enabled: "not installed".to_string(),
            active: "inactive".to_string(),
            rotates_all_wallpapers: false,
            interval_secs: 45,
            rotation_entries: 1,
            service_file: PathBuf::from("/tmp/walt-rotation.service"),
        });

        assert!(text.contains("Status:   not installed"));
        assert!(text.contains("Loaded:   not found (/tmp/walt-rotation.service)"));
        assert!(text.contains("Enabled:  not installed"));
        assert!(text.contains("Active:   inactive"));
        assert!(text.contains("Mode:     selected wallpapers"));
        assert!(text.contains("Interval: 45s"));
        assert!(text.contains("Entries:  1 wallpaper"));
    }

    #[test]
    fn formats_rotation_status_for_rotate_all_mode() {
        let text = format_rotation_service_status(&RotationServiceStatus {
            installed: true,
            enabled: "enabled".to_string(),
            active: "active".to_string(),
            rotates_all_wallpapers: true,
            interval_secs: 300,
            rotation_entries: 4,
            service_file: PathBuf::from("/tmp/walt-rotation.service"),
        });

        assert!(text.contains("Mode:     all wallpapers"));
        assert!(text.contains("Entries:  all wallpapers"));
    }
}
