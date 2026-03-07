use std::process::Command;

#[derive(Debug)]
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
    // Preload first to avoid flicker
    let preload_output = Command::new("hyprctl")
        .args(["hyprpaper", "preload", wallpaper_path])
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to preload wallpaper: {}", e))?;

    if !preload_output.status.success() {
        return Err(command_failure("Preload failed", &preload_output));
    }

    // Get monitors and set wallpaper for each
    let monitors = get_monitors();
    if monitors.is_empty() {
        return Err(anyhow::anyhow!("No monitors found"));
    }

    for monitor in monitors {
        let arg = format!("{},{}", monitor.name, wallpaper_path);
        let output = Command::new("hyprctl")
            .args(["hyprpaper", "wallpaper", &arg])
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to set wallpaper: {}", e))?;

        if !output.status.success() {
            eprintln!(
                "Warning: Failed to set wallpaper for {}: {}",
                monitor.name,
                command_failure("wallpaper command failed", &output)
            );
        }
    }

    Ok(())
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
