use std::{
    collections::HashSet,
    path::PathBuf,
    process::{Command, Output},
    thread,
    time::Duration,
};

const HYPERPAPER_SERVICE: &str = "hyprpaper.service";
const HYPERPAPER_PROCESS_NAME: &str = "hyprpaper";
const HYPERPAPER_WAIT_ATTEMPTS: usize = 20;
const HYPERPAPER_WAIT_DELAY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveWallpaperAssignment {
    pub monitor_name: String,
    pub wallpaper_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendUnavailableReason {
    HyprpaperUnavailable,
    NoMonitors,
    MonitorNotReady,
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

pub fn classify_backend_unavailable(error: &anyhow::Error) -> Option<BackendUnavailableReason> {
    classify_backend_unavailable_message(&error.to_string())
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

pub fn set_wallpapers_for_monitors(assignments: &[(String, PathBuf)]) -> anyhow::Result<()> {
    for wallpaper_path in unique_wallpaper_paths(assignments) {
        preload_wallpaper(&wallpaper_path.to_string_lossy())?;
    }

    for (monitor_name, wallpaper_path) in assignments {
        apply_wallpaper_to_monitor(monitor_name, &wallpaper_path.to_string_lossy())?;
    }

    Ok(())
}

pub fn get_active_wallpaper_assignments() -> anyhow::Result<Vec<ActiveWallpaperAssignment>> {
    let output = run_hyprpaper_query_with_retry(&["listactive"], "Active wallpaper query failed")?;

    Ok(parse_active_wallpaper_assignments(
        &String::from_utf8_lossy(&output.stdout),
    ))
}

pub fn get_active_wallpapers() -> anyhow::Result<Vec<PathBuf>> {
    Ok(deduplicate_active_wallpapers(
        &get_active_wallpaper_assignments()?,
    ))
}

fn parse_active_wallpaper_assignments(output: &str) -> Vec<ActiveWallpaperAssignment> {
    output
        .lines()
        .filter_map(parse_active_wallpaper_assignment_line)
        .collect()
}

fn parse_active_wallpaper_assignment_line(line: &str) -> Option<ActiveWallpaperAssignment> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (monitor_name, wallpaper_path) = trimmed
        .split_once(" = ")
        .map(|(left, right)| (left.trim(), right.trim()))
        .or_else(|| {
            trimmed
                .split_once(',')
                .map(|(left, right)| (left.trim(), right.trim()))
        })?;

    if monitor_name.is_empty() || wallpaper_path.is_empty() {
        return None;
    }

    Some(ActiveWallpaperAssignment {
        monitor_name: monitor_name.to_string(),
        wallpaper_path: PathBuf::from(wallpaper_path),
    })
}

fn deduplicate_active_wallpapers(assignments: &[ActiveWallpaperAssignment]) -> Vec<PathBuf> {
    let mut wallpapers = Vec::new();
    let mut seen = HashSet::new();

    for assignment in assignments {
        let path = assignment.wallpaper_path.clone();
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

fn classify_backend_unavailable_message(message: &str) -> Option<BackendUnavailableReason> {
    let normalized = message.to_lowercase();

    if normalized.contains("couldn't connect to")
        && (normalized.contains(".hyprpaper.sock") || normalized.contains("hyprpaper"))
    {
        return Some(BackendUnavailableReason::HyprpaperUnavailable);
    }

    if normalized.contains("no monitors found") {
        return Some(BackendUnavailableReason::NoMonitors);
    }

    if normalized.contains("monitor")
        && (normalized.contains("not found")
            || normalized.contains("not ready")
            || normalized.contains("unknown output")
            || normalized.contains("invalid format"))
    {
        return Some(BackendUnavailableReason::MonitorNotReady);
    }

    None
}

fn preload_wallpaper(wallpaper_path: &str) -> anyhow::Result<()> {
    run_hyprpaper_command_with_retry(&["preload", wallpaper_path], "Preload failed")
}

fn unique_wallpaper_paths(assignments: &[(String, PathBuf)]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut unique_paths = Vec::new();

    for (_, wallpaper_path) in assignments {
        if seen.insert(wallpaper_path.clone()) {
            unique_paths.push(wallpaper_path.clone());
        }
    }

    unique_paths
}

fn apply_wallpaper_to_monitor(monitor_name: &str, wallpaper_path: &str) -> anyhow::Result<()> {
    let arg = format!("{monitor_name},{wallpaper_path}");
    run_hyprpaper_command_with_retry(&["wallpaper", &arg], "wallpaper command failed")
}

fn ensure_hyprpaper_ready() -> anyhow::Result<()> {
    start_hyprpaper_service_if_possible();
    let _ = wait_for_hyprpaper_process();
    Ok(())
}

fn start_hyprpaper_service_if_possible() {
    let _ = Command::new("systemctl")
        .arg("--user")
        .args(["start", "--no-block", HYPERPAPER_SERVICE])
        .output();
}

fn wait_for_hyprpaper_process() -> bool {
    for _ in 0..HYPERPAPER_WAIT_ATTEMPTS {
        if hyprpaper_process_running() {
            return true;
        }
        thread::sleep(HYPERPAPER_WAIT_DELAY);
    }

    hyprpaper_process_running()
}

fn hyprpaper_process_running() -> bool {
    Command::new("pgrep")
        .args(["-x", HYPERPAPER_PROCESS_NAME])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_hyprpaper_command_with_retry(args: &[&str], context: &str) -> anyhow::Result<()> {
    let output = run_hyprpaper_query_with_retry(args, context)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(with_hyprpaper_hint(command_failure(context, &output)))
    }
}

fn run_hyprpaper_query_with_retry(args: &[&str], context: &str) -> anyhow::Result<Output> {
    ensure_hyprpaper_ready()?;

    let mut last_error = None;
    for attempt in 0..HYPERPAPER_WAIT_ATTEMPTS {
        let output = run_hyprpaper_query(args)
            .map_err(|error| anyhow::anyhow!("Failed to run hyprpaper command: {error}"))?;

        if output.status.success() {
            return Ok(output);
        }

        let error = command_failure(context, &output);
        last_error = Some(error);

        if should_wait_for_another_hyprpaper_attempt(attempt, HYPERPAPER_WAIT_ATTEMPTS) {
            thread::sleep(HYPERPAPER_WAIT_DELAY);
        }
    }

    Err(with_hyprpaper_hint(last_error.unwrap_or_else(|| {
        anyhow::anyhow!("{context}: hyprpaper command failed")
    })))
}

fn run_hyprpaper_query(args: &[&str]) -> std::io::Result<Output> {
    Command::new("hyprctl").arg("hyprpaper").args(args).output()
}

fn with_hyprpaper_hint(error: anyhow::Error) -> anyhow::Error {
    match classify_backend_unavailable(&error) {
        Some(BackendUnavailableReason::HyprpaperUnavailable) => anyhow::anyhow!(
            "{} Hint: Walt tried to start {} and retry hyprpaper IPC, but hyprpaper still appears unavailable. Check whether {} is crashing or failing to start in your session.",
            error,
            HYPERPAPER_SERVICE,
            HYPERPAPER_SERVICE
        ),
        _ => error,
    }
}

fn should_wait_for_another_hyprpaper_attempt(attempt: usize, max_attempts: usize) -> bool {
    attempt + 1 < max_attempts
}

#[cfg(test)]
mod tests {
    use super::{
        classify_backend_unavailable_message, deduplicate_active_wallpapers,
        parse_active_wallpaper_assignments, parse_monitors,
        should_wait_for_another_hyprpaper_attempt, unique_wallpaper_paths, with_hyprpaper_hint,
        ActiveWallpaperAssignment, BackendUnavailableReason, Monitor,
    };
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
    fn keeps_unique_preload_paths_in_assignment_order() {
        let unique = unique_wallpaper_paths(&[
            (
                "HDMI-A-1".to_string(),
                PathBuf::from("/wallpapers/alpha.jpg"),
            ),
            ("DP-1".to_string(), PathBuf::from("/wallpapers/beta.jpg")),
            ("DP-2".to_string(), PathBuf::from("/wallpapers/alpha.jpg")),
        ]);

        assert_eq!(
            unique,
            vec![
                PathBuf::from("/wallpapers/alpha.jpg"),
                PathBuf::from("/wallpapers/beta.jpg")
            ]
        );
    }

    #[test]
    fn supports_single_monitor_assignment_batches() {
        let unique = unique_wallpaper_paths(&[(
            "HDMI-A-1".to_string(),
            PathBuf::from("/wallpapers/alpha.jpg"),
        )]);

        assert_eq!(unique, vec![PathBuf::from("/wallpapers/alpha.jpg")]);
    }

    #[test]
    fn parses_one_active_wallpaper_assignment() {
        let assignments = parse_active_wallpaper_assignments("HDMI-A-1 = /wallpapers/alpha.jpg\n");

        assert_eq!(
            assignments,
            vec![ActiveWallpaperAssignment {
                monitor_name: "HDMI-A-1".to_string(),
                wallpaper_path: PathBuf::from("/wallpapers/alpha.jpg"),
            }]
        );
    }

    #[test]
    fn parses_multiple_active_wallpaper_assignment_formats() {
        let assignments = parse_active_wallpaper_assignments(
            "HDMI-A-1 = /wallpapers/alpha.jpg\nDP-1,/wallpapers/beta.png\n",
        );

        assert_eq!(
            assignments,
            vec![
                ActiveWallpaperAssignment {
                    monitor_name: "HDMI-A-1".to_string(),
                    wallpaper_path: PathBuf::from("/wallpapers/alpha.jpg"),
                },
                ActiveWallpaperAssignment {
                    monitor_name: "DP-1".to_string(),
                    wallpaper_path: PathBuf::from("/wallpapers/beta.png"),
                }
            ]
        );
    }

    #[test]
    fn deduplicates_active_wallpapers_for_compatibility_wrapper() {
        let wallpapers = deduplicate_active_wallpapers(&[
            ActiveWallpaperAssignment {
                monitor_name: "HDMI-A-1".to_string(),
                wallpaper_path: PathBuf::from("/wallpapers/alpha.jpg"),
            },
            ActiveWallpaperAssignment {
                monitor_name: "DP-1".to_string(),
                wallpaper_path: PathBuf::from("/wallpapers/alpha.jpg"),
            },
        ]);

        assert_eq!(wallpapers, vec![PathBuf::from("/wallpapers/alpha.jpg")]);
    }

    #[test]
    fn ignores_unparseable_active_wallpaper_lines() {
        let assignments = parse_active_wallpaper_assignments(
            "not valid output\nstill not valid\nDP-1 = \n = /wallpapers/alpha.jpg\n",
        );

        assert!(assignments.is_empty());
    }

    #[test]
    fn classifies_hyprpaper_socket_errors_as_transient() {
        assert_eq!(
            classify_backend_unavailable_message(
                "Preload failed: status exit status: 3, stdout: Couldn't connect to /run/user/1000/hypr/instance/.hyprpaper.sock. (3)"
            ),
            Some(BackendUnavailableReason::HyprpaperUnavailable)
        );
    }

    #[test]
    fn classifies_no_monitors_as_transient() {
        assert_eq!(
            classify_backend_unavailable_message("No monitors found"),
            Some(BackendUnavailableReason::NoMonitors)
        );
    }

    #[test]
    fn leaves_unrelated_failures_as_hard_errors() {
        assert_eq!(
            classify_backend_unavailable_message("Preload failed: permission denied"),
            None
        );
    }

    #[test]
    fn retries_until_last_attempt_only() {
        assert!(should_wait_for_another_hyprpaper_attempt(0, 20));
        assert!(should_wait_for_another_hyprpaper_attempt(18, 20));
        assert!(!should_wait_for_another_hyprpaper_attempt(19, 20));
    }

    #[test]
    fn appends_hint_for_persistent_hyprpaper_unavailability() {
        let error = anyhow::anyhow!(
            "Preload failed: status exit status: 3, stdout: Couldn't connect to /run/user/1000/hypr/instance/.hyprpaper.sock. (3)"
        );

        let hinted = with_hyprpaper_hint(error);
        let text = hinted.to_string();
        assert!(text.contains("Hint: Walt tried to start hyprpaper.service"));
        assert!(text.contains("hyprpaper.service is crashing or failing to start"));
    }

    #[test]
    fn leaves_non_hyprpaper_errors_without_hint() {
        let error = anyhow::anyhow!("Preload failed: permission denied");
        let hinted = with_hyprpaper_hint(error);
        assert_eq!(hinted.to_string(), "Preload failed: permission denied");
    }
}
