use std::{collections::HashSet, path::PathBuf, process::Command};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub name: String,
}

pub fn get_monitors() -> Vec<Monitor> {
    let output = match Command::new("hyprctl").args(["monitors", "-j"]).output() {
        Ok(output) => output,
        Err(_) => return vec![],
    };

    if !output.status.success() {
        return vec![];
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    parse_monitors(&json_str)
}

fn parse_monitors(json_str: &str) -> Vec<Monitor> {
    let mut monitors = Vec::new();

    if let Ok(data) = serde_json::from_str::<Vec<serde_json::Value>>(json_str) {
        for monitor in data {
            if let (Some(name), Some(_width), Some(_height)) = (
                monitor.get("name").and_then(|v| v.as_str()),
                monitor.get("width").and_then(|v| v.as_i64()),
                monitor.get("height").and_then(|v| v.as_i64()),
            ) {
                monitors.push(Monitor {
                    name: name.to_string(),
                });
            }
        }
    }

    monitors
}

pub fn set_wallpaper(wallpaper_path: &str) -> anyhow::Result<()> {
    preload_wallpaper(wallpaper_path)?;
    let monitors = get_monitors();
    if monitors.is_empty() {
        return Err(anyhow::anyhow!("No monitors found"));
    }

    for monitor in monitors {
        if let Err(error) = apply_wallpaper_to_monitor(&monitor.name, wallpaper_path) {
            eprintln!(
                "Warning: Failed to set wallpaper for {}: {}",
                monitor.name, error
            );
        }
    }

    Ok(())
}

pub fn set_wallpaper_for_monitor(monitor_name: &str, wallpaper_path: &str) -> anyhow::Result<()> {
    preload_wallpaper(wallpaper_path)?;
    apply_wallpaper_to_monitor(monitor_name, wallpaper_path)
}

pub fn get_active_wallpapers() -> anyhow::Result<Vec<PathBuf>> {
    let output = Command::new("hyprctl")
        .args(["hyprpaper", "listactive"])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to read active wallpapers: {}", e))?;

    if !output.status.success() {
        return Err(command_failure("Active wallpaper query failed", &output));
    }

    Ok(parse_active_wallpapers(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_active_wallpapers(output: &str) -> Vec<PathBuf> {
    let mut wallpapers = Vec::new();
    let mut seen = HashSet::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let path_text = trimmed
            .split_once(" = ")
            .map(|(_, right)| right.trim())
            .or_else(|| trimmed.split_once(',').map(|(_, right)| right.trim()));

        let Some(path_text) = path_text else {
            continue;
        };

        if path_text.is_empty() {
            continue;
        }

        let path = PathBuf::from(path_text);
        if seen.insert(path.clone()) {
            wallpapers.push(path);
        }
    }

    wallpapers
}

fn command_failure(context: &str, output: &std::process::Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut details = format!("status {}", output.status);

    if !stderr.is_empty() {
        details.push_str(&format!(", stderr: {stderr}"));
    }

    if !stdout.is_empty() {
        details.push_str(&format!(", stdout: {stdout}"));
    }

    anyhow::anyhow!("{context}: {details}")
}

fn preload_wallpaper(wallpaper_path: &str) -> anyhow::Result<()> {
    let preload_output = Command::new("hyprctl")
        .args(["hyprpaper", "preload", wallpaper_path])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to preload wallpaper: {}", e))?;

    if !preload_output.status.success() {
        return Err(command_failure("Preload failed", &preload_output));
    }

    Ok(())
}

fn apply_wallpaper_to_monitor(monitor_name: &str, wallpaper_path: &str) -> anyhow::Result<()> {
    let arg = format!("{monitor_name},{wallpaper_path}");
    let output = Command::new("hyprctl")
        .args(["hyprpaper", "wallpaper", &arg])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to set wallpaper: {}", e))?;

    if !output.status.success() {
        return Err(command_failure("wallpaper command failed", &output));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{parse_active_wallpapers, parse_monitors, Monitor};
    use std::path::PathBuf;

    #[test]
    fn parses_one_monitor() {
        let monitors = parse_monitors(r#"[{"name":"HDMI-A-1","width":1920,"height":1080}]"#);

        assert_eq!(
            monitors,
            vec![Monitor {
                name: "HDMI-A-1".to_string()
            }]
        );
    }

    #[test]
    fn parses_multiple_monitors() {
        let monitors = parse_monitors(
            r#"[
                {"name":"HDMI-A-1","width":1920,"height":1080},
                {"name":"DP-1","width":2560,"height":1440}
            ]"#,
        );

        assert_eq!(
            monitors,
            vec![
                Monitor {
                    name: "HDMI-A-1".to_string()
                },
                Monitor {
                    name: "DP-1".to_string()
                }
            ]
        );
    }

    #[test]
    fn ignores_invalid_monitor_entries() {
        let monitors = parse_monitors(
            r#"[
                {"name":"HDMI-A-1","width":1920,"height":1080},
                {"name":"DP-1","width":2560},
                {"name":"DP-2","height":1440},
                {"width":1280,"height":720}
            ]"#,
        );

        assert_eq!(
            monitors,
            vec![Monitor {
                name: "HDMI-A-1".to_string()
            }]
        );
    }

    #[test]
    fn parses_one_active_wallpaper() {
        let wallpapers = parse_active_wallpapers("HDMI-A-1 = /wallpapers/alpha.jpg\n");
        assert_eq!(wallpapers, vec![PathBuf::from("/wallpapers/alpha.jpg")]);
    }

    #[test]
    fn parses_multiple_active_wallpapers() {
        let wallpapers = parse_active_wallpapers(
            "HDMI-A-1 = /wallpapers/alpha.jpg\nDP-1 = /wallpapers/beta.png\n",
        );

        assert_eq!(
            wallpapers,
            vec![
                PathBuf::from("/wallpapers/alpha.jpg"),
                PathBuf::from("/wallpapers/beta.png")
            ]
        );
    }

    #[test]
    fn deduplicates_active_wallpapers() {
        let wallpapers = parse_active_wallpapers(
            "HDMI-A-1 = /wallpapers/alpha.jpg\nDP-1 = /wallpapers/alpha.jpg\n",
        );

        assert_eq!(wallpapers, vec![PathBuf::from("/wallpapers/alpha.jpg")]);
    }

    #[test]
    fn ignores_unparseable_lines() {
        let wallpapers = parse_active_wallpapers("not valid output\nstill not valid\n");
        assert!(wallpapers.is_empty());
    }
}
