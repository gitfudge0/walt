use std::{
    collections::HashSet,
    path::PathBuf,
    process::{Command, Output},
    sync::{Mutex, OnceLock},
    thread,
    time::Duration,
};

use log::{debug, error, info, warn};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HyprpaperCommandFailure {
    Transient(BackendUnavailableReason),
    UnsupportedPreload,
    UnsupportedActiveQuery,
    HardFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreloadSupport {
    Preloaded,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilitySupport {
    Unknown,
    Supported,
    Unsupported,
}

static PRELOAD_SUPPORT_CACHE: OnceLock<Mutex<CapabilitySupport>> = OnceLock::new();
static ACTIVE_QUERY_SUPPORT_CACHE: OnceLock<Mutex<CapabilitySupport>> = OnceLock::new();

pub fn get_monitors() -> Vec<Monitor> {
    let output = match Command::new("hyprctl").args(["monitors", "-j"]).output() {
        Ok(output) => output,
        Err(_) => return vec![],
    };

    if !output.status.success() {
        return vec![];
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let monitors = parse_monitors(&json_str);
    debug!(
        "parsed monitors count={} monitors={:?}",
        monitors.len(),
        monitors
    );
    monitors
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
    info!("applying wallpaper to all monitors path={wallpaper_path}");
    preload_wallpaper_if_supported(wallpaper_path)?;
    let monitors = get_monitors();
    if monitors.is_empty() {
        return Err(anyhow::anyhow!("No monitors found"));
    }

    let mut failures = Vec::new();
    for monitor in monitors {
        if let Err(error) = apply_wallpaper_to_monitor(&monitor.name, wallpaper_path) {
            failures.push((monitor.name, error.to_string()));
        }
    }

    summarize_multi_monitor_apply_failures(&failures)
}

pub fn set_wallpaper_for_monitor(monitor_name: &str, wallpaper_path: &str) -> anyhow::Result<()> {
    info!("applying wallpaper to single monitor monitor={monitor_name} path={wallpaper_path}");
    preload_wallpaper_if_supported(wallpaper_path)?;
    apply_wallpaper_to_monitor(monitor_name, wallpaper_path)
}

pub fn set_wallpapers_for_monitors(assignments: &[(String, PathBuf)]) -> anyhow::Result<()> {
    info!("applying wallpaper batch assignments={}", assignments.len());
    preload_unique_wallpapers(assignments, |wallpaper_path| {
        preload_wallpaper_if_supported(&wallpaper_path.to_string_lossy())
    })?;

    for (monitor_name, wallpaper_path) in assignments {
        apply_wallpaper_to_monitor(monitor_name, &wallpaper_path.to_string_lossy())?;
    }

    Ok(())
}

#[allow(dead_code)]
pub fn get_active_wallpaper_assignments() -> anyhow::Result<Vec<ActiveWallpaperAssignment>> {
    Ok(active_wallpaper_assignments_or_empty(
        get_active_wallpaper_assignments_if_supported()?,
    ))
}

#[allow(dead_code)]
pub fn get_active_wallpapers() -> anyhow::Result<Vec<PathBuf>> {
    Ok(active_wallpapers_from_assignments(
        get_active_wallpaper_assignments_if_supported()?,
    ))
}

pub(crate) fn get_active_wallpapers_if_supported() -> anyhow::Result<Option<Vec<PathBuf>>> {
    Ok(get_active_wallpaper_assignments_if_supported()?
        .map(|assignments| active_wallpapers_from_assignments(Some(assignments))))
}

pub(crate) fn get_active_wallpaper_assignments_if_supported(
) -> anyhow::Result<Option<Vec<ActiveWallpaperAssignment>>> {
    info!("querying active wallpapers");
    if cached_capability_support(&ACTIVE_QUERY_SUPPORT_CACHE) == CapabilitySupport::Unsupported {
        debug!("skipping active wallpaper query because capability cache is unsupported");
        return Ok(None);
    }

    compatibility_wrap_active_wallpaper_assignments(
        run_hyprpaper_query_with_retry(&["listactive"], "Active wallpaper query failed").map(
            |output| parse_active_wallpaper_assignments(&String::from_utf8_lossy(&output.stdout)),
        ),
    )
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

fn active_wallpaper_assignments_or_empty(
    assignments: Option<Vec<ActiveWallpaperAssignment>>,
) -> Vec<ActiveWallpaperAssignment> {
    assignments.unwrap_or_default()
}

fn active_wallpapers_from_assignments(
    assignments: Option<Vec<ActiveWallpaperAssignment>>,
) -> Vec<PathBuf> {
    deduplicate_active_wallpapers(&active_wallpaper_assignments_or_empty(assignments))
}

fn compatibility_wrap_active_wallpaper_assignments(
    assignments: anyhow::Result<Vec<ActiveWallpaperAssignment>>,
) -> anyhow::Result<Option<Vec<ActiveWallpaperAssignment>>> {
    match assignments {
        Ok(assignments) => {
            debug!(
                "parsed active wallpaper assignments count={}",
                assignments.len()
            );
            update_cached_capability_support(&ACTIVE_QUERY_SUPPORT_CACHE, "active-query", Ok(()));
            Ok(Some(assignments))
        }
        Err(error) => {
            let failure = classify_hyprpaper_command_failure_message(&error.to_string());
            log_hyprpaper_failure("active wallpaper query", failure, &error.to_string());
            update_cached_capability_support(
                &ACTIVE_QUERY_SUPPORT_CACHE,
                "active-query",
                Err(failure),
            );
            match failure {
                HyprpaperCommandFailure::UnsupportedActiveQuery => {
                    info!("active wallpaper query unsupported; using best-effort local state");
                    Ok(None)
                }
                _ => Err(error),
            }
        }
    }
}

fn summarize_multi_monitor_apply_failures(failures: &[(String, String)]) -> anyhow::Result<()> {
    if failures.is_empty() {
        return Ok(());
    }

    let details = failures
        .iter()
        .map(|(monitor, error)| format!("{monitor}: {error}"))
        .collect::<Vec<_>>()
        .join("; ");

    let message = format!("Failed to set wallpaper on one or more monitors: {details}");
    error!("{message}");
    Err(anyhow::anyhow!("{message}"))
}

fn command_failure(context: &str, output: &std::process::Output) -> anyhow::Error {
    anyhow::anyhow!("{context}: {}", command_output_details(output))
}

fn command_output_details(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let mut details = format!("status {}", output.status);

    if !stderr.is_empty() {
        details.push_str(&format!(", stderr: {stderr}"));
    }

    if !stdout.is_empty() {
        details.push_str(&format!(", stdout: {stdout}"));
    }

    details
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

fn classify_hyprpaper_command_failure_message(message: &str) -> HyprpaperCommandFailure {
    let normalized = message.to_lowercase();

    if normalized.contains("preload failed") && normalized.contains("invalid hyprpaper request") {
        return HyprpaperCommandFailure::UnsupportedPreload;
    }

    if normalized.contains("preload failed")
        && (normalized.contains("unknown hyprpaper request")
            || normalized.contains("unkonwn hyprpaper request")
            || normalized.contains("invalid request"))
    {
        return HyprpaperCommandFailure::UnsupportedPreload;
    }

    if normalized.contains("active wallpaper query failed")
        && (normalized.contains("protocol version too low")
            || normalized.contains("hyprpaper too old"))
    {
        return HyprpaperCommandFailure::UnsupportedActiveQuery;
    }

    if normalized.contains("active wallpaper query failed")
        && (normalized.contains("unknown hyprpaper request")
            || normalized.contains("invalid request"))
    {
        return HyprpaperCommandFailure::UnsupportedActiveQuery;
    }

    if let Some(reason) = classify_backend_unavailable_message(message) {
        return HyprpaperCommandFailure::Transient(reason);
    }

    HyprpaperCommandFailure::HardFailure
}

fn capability_cache_state_after_result(
    current: CapabilitySupport,
    result: Result<(), HyprpaperCommandFailure>,
) -> CapabilitySupport {
    match result {
        Ok(()) => CapabilitySupport::Supported,
        Err(HyprpaperCommandFailure::UnsupportedPreload)
        | Err(HyprpaperCommandFailure::UnsupportedActiveQuery) => CapabilitySupport::Unsupported,
        Err(HyprpaperCommandFailure::Transient(_)) | Err(HyprpaperCommandFailure::HardFailure) => {
            current
        }
    }
}

fn cached_capability_support(
    cache: &'static OnceLock<Mutex<CapabilitySupport>>,
) -> CapabilitySupport {
    *cache
        .get_or_init(|| Mutex::new(CapabilitySupport::Unknown))
        .lock()
        .expect("capability cache lock poisoned")
}

fn update_cached_capability_support(
    cache: &'static OnceLock<Mutex<CapabilitySupport>>,
    capability_name: &str,
    result: Result<(), HyprpaperCommandFailure>,
) {
    let mut guard = cache
        .get_or_init(|| Mutex::new(CapabilitySupport::Unknown))
        .lock()
        .expect("capability cache lock poisoned");
    let previous = *guard;
    *guard = capability_cache_state_after_result(*guard, result);
    if previous != *guard {
        debug!(
            "{} capability cache transitioned from {:?} to {:?}",
            capability_name, previous, *guard
        );
    }
}

fn should_retry_hyprpaper_command_failure(failure: HyprpaperCommandFailure) -> bool {
    matches!(failure, HyprpaperCommandFailure::Transient(_))
}

fn preload_wallpaper_if_supported(wallpaper_path: &str) -> anyhow::Result<PreloadSupport> {
    if cached_capability_support(&PRELOAD_SUPPORT_CACHE) == CapabilitySupport::Unsupported {
        debug!("skipping preload because capability cache is unsupported");
        return Ok(PreloadSupport::Unsupported);
    }

    match run_hyprpaper_command_with_retry(&["preload", wallpaper_path], "Preload failed") {
        Ok(()) => {
            update_cached_capability_support(&PRELOAD_SUPPORT_CACHE, "preload", Ok(()));
            Ok(PreloadSupport::Preloaded)
        }
        Err(error) => {
            let failure = classify_hyprpaper_command_failure_message(&error.to_string());
            update_cached_capability_support(&PRELOAD_SUPPORT_CACHE, "preload", Err(failure));
            match failure {
                HyprpaperCommandFailure::UnsupportedPreload => {
                    info!(
                        "hyprpaper preload unsupported for path={wallpaper_path}; applying wallpaper without preload"
                    );
                    Ok(PreloadSupport::Unsupported)
                }
                _ => Err(error),
            }
        }
    }
}

fn preload_unique_wallpapers<F>(
    assignments: &[(String, PathBuf)],
    mut preload: F,
) -> anyhow::Result<()>
where
    F: FnMut(&PathBuf) -> anyhow::Result<PreloadSupport>,
{
    for wallpaper_path in unique_wallpaper_paths(assignments) {
        match preload(&wallpaper_path)? {
            PreloadSupport::Preloaded => {}
            PreloadSupport::Unsupported => break,
        }
    }

    Ok(())
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

fn unsupported_active_query_verification_message(
    active_query_support: CapabilitySupport,
) -> Option<&'static str> {
    (active_query_support == CapabilitySupport::Unsupported).then_some(
        "wallpaper IPC accepted by hyprctl, but Walt cannot verify the visual result because active status query is unsupported",
    )
}

fn apply_wallpaper_to_monitor(monitor_name: &str, wallpaper_path: &str) -> anyhow::Result<()> {
    let arg = format!("{monitor_name},{wallpaper_path}");
    debug!("applying hyprpaper wallpaper arg={arg}");
    run_hyprpaper_command_with_retry(&["wallpaper", &arg], "wallpaper command failed")?;
    if let Some(message) = unsupported_active_query_verification_message(cached_capability_support(
        &ACTIVE_QUERY_SUPPORT_CACHE,
    )) {
        debug!("{message}");
    }
    Ok(())
}

fn ensure_hyprpaper_ready() -> anyhow::Result<()> {
    start_hyprpaper_service_if_possible();
    let _ = wait_for_hyprpaper_process();
    Ok(())
}

fn start_hyprpaper_service_if_possible() {
    debug!("attempting to start hyprpaper service via systemd user unit");
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
        debug!(
            "hyprpaper command success payload args={:?} {}",
            args,
            command_output_details(&output)
        );
        Ok(())
    } else {
        Err(with_hyprpaper_hint(command_failure(context, &output)))
    }
}

fn run_hyprpaper_query_with_retry(args: &[&str], context: &str) -> anyhow::Result<Output> {
    ensure_hyprpaper_ready()?;

    let mut last_error = None;
    for attempt in 0..HYPERPAPER_WAIT_ATTEMPTS {
        debug!(
            "running hyprpaper command attempt={}/{} args={:?} context={}",
            attempt + 1,
            HYPERPAPER_WAIT_ATTEMPTS,
            args,
            context
        );
        let output = run_hyprpaper_query(args)
            .map_err(|error| anyhow::anyhow!("Failed to run hyprpaper command: {error}"))?;

        if output.status.success() {
            debug!(
                "hyprpaper command succeeded attempt={}/{} args={:?}",
                attempt + 1,
                HYPERPAPER_WAIT_ATTEMPTS,
                args
            );
            return Ok(output);
        }

        let error = command_failure(context, &output);
        let failure = classify_hyprpaper_command_failure_message(&error.to_string());
        debug!(
            "hyprpaper command failed attempt={}/{} args={:?} failure={:?} error={}",
            attempt + 1,
            HYPERPAPER_WAIT_ATTEMPTS,
            args,
            failure,
            error
        );
        if !should_retry_hyprpaper_command_failure(failure) {
            log_hyprpaper_failure(context, failure, &error.to_string());
            return Err(with_hyprpaper_hint(error));
        }

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
    debug!("executing hyprctl hyprpaper args={:?}", args);
    Command::new("hyprctl").arg("hyprpaper").args(args).output()
}

fn log_hyprpaper_failure(context: &str, failure: HyprpaperCommandFailure, details: &str) {
    match failure {
        HyprpaperCommandFailure::Transient(reason) => warn!(
            "hyprpaper {} transient failure reason={:?} details={}",
            context, reason, details
        ),
        HyprpaperCommandFailure::UnsupportedPreload => {
            warn!(
                "hyprpaper {} unsupported-preload details={}",
                context, details
            )
        }
        HyprpaperCommandFailure::UnsupportedActiveQuery => warn!(
            "hyprpaper {} unsupported-active-query details={}",
            context, details
        ),
        HyprpaperCommandFailure::HardFailure => {
            error!("hyprpaper {} hard failure details={}", context, details)
        }
    }
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
        active_wallpaper_assignments_or_empty, active_wallpapers_from_assignments,
        capability_cache_state_after_result, classify_backend_unavailable_message,
        classify_hyprpaper_command_failure_message, command_output_details,
        compatibility_wrap_active_wallpaper_assignments, deduplicate_active_wallpapers,
        parse_active_wallpaper_assignments, parse_monitors, preload_unique_wallpapers,
        should_retry_hyprpaper_command_failure, should_wait_for_another_hyprpaper_attempt,
        summarize_multi_monitor_apply_failures, unique_wallpaper_paths,
        unsupported_active_query_verification_message, with_hyprpaper_hint,
        ActiveWallpaperAssignment, BackendUnavailableReason, CapabilitySupport,
        HyprpaperCommandFailure, Monitor, PreloadSupport,
    };
    use std::{os::unix::process::ExitStatusExt, path::PathBuf, process::Output};

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
    fn classifies_invalid_hyprpaper_request_as_unsupported_preload() {
        assert_eq!(
            classify_hyprpaper_command_failure_message(
                "Preload failed: status exit status: 1, stdout: error: invalid hyprpaper request"
            ),
            HyprpaperCommandFailure::UnsupportedPreload
        );
    }

    #[test]
    fn classifies_unknown_hyprpaper_request_as_unsupported_preload() {
        assert_eq!(
            classify_hyprpaper_command_failure_message(
                "Preload failed: status exit status: 1, stdout: error: Unknown hyprpaper request"
            ),
            HyprpaperCommandFailure::UnsupportedPreload
        );
    }

    #[test]
    fn classifies_invalid_request_as_unsupported_preload() {
        assert_eq!(
            classify_hyprpaper_command_failure_message(
                "Preload failed: status exit status: 1, stdout: error: Invalid request"
            ),
            HyprpaperCommandFailure::UnsupportedPreload
        );
    }

    #[test]
    fn classifies_old_hyprpaper_protocol_mismatch_as_unsupported_active_query() {
        assert_eq!(
            classify_hyprpaper_command_failure_message(
                "Active wallpaper query failed: status exit status: 1, stdout: error: can't send: hyprpaper protocol version too low (hyprpaper too old)"
            ),
            HyprpaperCommandFailure::UnsupportedActiveQuery
        );
    }

    #[test]
    fn classifies_unknown_hyprpaper_request_as_unsupported_active_query() {
        assert_eq!(
            classify_hyprpaper_command_failure_message(
                "Active wallpaper query failed: status exit status: 1, stderr: error: Unknown hyprpaper request"
            ),
            HyprpaperCommandFailure::UnsupportedActiveQuery
        );
    }

    #[test]
    fn classifies_invalid_request_as_unsupported_active_query() {
        assert_eq!(
            classify_hyprpaper_command_failure_message(
                "Active wallpaper query failed: status exit status: 1, stderr: error: Invalid request"
            ),
            HyprpaperCommandFailure::UnsupportedActiveQuery
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
    fn classifies_permission_denied_as_hard_failure() {
        assert_eq!(
            classify_hyprpaper_command_failure_message("Preload failed: permission denied"),
            HyprpaperCommandFailure::HardFailure
        );
    }

    #[test]
    fn retries_transient_failures() {
        assert!(should_retry_hyprpaper_command_failure(
            HyprpaperCommandFailure::Transient(BackendUnavailableReason::HyprpaperUnavailable)
        ));
    }

    #[test]
    fn does_not_retry_unsupported_preload() {
        assert!(!should_retry_hyprpaper_command_failure(
            HyprpaperCommandFailure::UnsupportedPreload
        ));
    }

    #[test]
    fn does_not_retry_unsupported_active_query() {
        assert!(!should_retry_hyprpaper_command_failure(
            HyprpaperCommandFailure::UnsupportedActiveQuery
        ));
    }

    #[test]
    fn does_not_retry_hard_failures() {
        assert!(!should_retry_hyprpaper_command_failure(
            HyprpaperCommandFailure::HardFailure
        ));
    }

    #[test]
    fn stops_batch_preloading_after_first_unsupported_result() {
        let assignments = vec![
            (
                "HDMI-A-1".to_string(),
                PathBuf::from("/wallpapers/alpha.jpg"),
            ),
            ("DP-1".to_string(), PathBuf::from("/wallpapers/beta.jpg")),
        ];
        let mut attempts = Vec::new();

        preload_unique_wallpapers(&assignments, |wallpaper_path| {
            attempts.push(wallpaper_path.clone());
            if attempts.len() == 1 {
                Ok(PreloadSupport::Unsupported)
            } else {
                Ok(PreloadSupport::Preloaded)
            }
        })
        .expect("preload compatibility flow should succeed");

        assert_eq!(attempts, vec![PathBuf::from("/wallpapers/alpha.jpg")]);
    }

    #[test]
    fn unsupported_active_query_becomes_no_assignments() {
        let assignments = compatibility_wrap_active_wallpaper_assignments(Err(anyhow::anyhow!(
            "Active wallpaper query failed: status exit status: 1, stdout: error: can't send: hyprpaper protocol version too low (hyprpaper too old)"
        )))
        .expect("old hyprpaper active query should degrade");

        assert_eq!(
            active_wallpaper_assignments_or_empty(assignments),
            Vec::new()
        );
    }

    #[test]
    fn unsupported_active_query_becomes_no_active_wallpapers() {
        let wallpapers = active_wallpapers_from_assignments(None);
        assert!(wallpapers.is_empty());
    }

    #[test]
    fn formats_successful_command_output_details_with_payloads() {
        let output = Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: b"ok".to_vec(),
            stderr: b"warning".to_vec(),
        };

        assert_eq!(
            command_output_details(&output),
            "status exit status: 0, stderr: warning, stdout: ok"
        );
    }

    #[test]
    fn reports_unverified_wallpaper_success_when_active_query_is_unsupported() {
        assert_eq!(
            unsupported_active_query_verification_message(CapabilitySupport::Unsupported),
            Some(
                "wallpaper IPC accepted by hyprctl, but Walt cannot verify the visual result because active status query is unsupported"
            )
        );
        assert_eq!(
            unsupported_active_query_verification_message(CapabilitySupport::Supported),
            None
        );
    }

    #[test]
    fn capability_cache_marks_success_as_supported() {
        assert_eq!(
            capability_cache_state_after_result(CapabilitySupport::Unknown, Ok(())),
            CapabilitySupport::Supported
        );
    }

    #[test]
    fn capability_cache_marks_unsupported_results_as_unsupported() {
        assert_eq!(
            capability_cache_state_after_result(
                CapabilitySupport::Unknown,
                Err(HyprpaperCommandFailure::UnsupportedPreload)
            ),
            CapabilitySupport::Unsupported
        );
        assert_eq!(
            capability_cache_state_after_result(
                CapabilitySupport::Unknown,
                Err(HyprpaperCommandFailure::UnsupportedActiveQuery)
            ),
            CapabilitySupport::Unsupported
        );
    }

    #[test]
    fn capability_cache_keeps_current_state_for_transient_failures() {
        assert_eq!(
            capability_cache_state_after_result(
                CapabilitySupport::Unknown,
                Err(HyprpaperCommandFailure::Transient(
                    BackendUnavailableReason::HyprpaperUnavailable
                ))
            ),
            CapabilitySupport::Unknown
        );
        assert_eq!(
            capability_cache_state_after_result(
                CapabilitySupport::Supported,
                Err(HyprpaperCommandFailure::HardFailure)
            ),
            CapabilitySupport::Supported
        );
    }

    #[test]
    fn unrelated_active_query_failures_remain_errors() {
        let error = compatibility_wrap_active_wallpaper_assignments(Err(anyhow::anyhow!(
            "Active wallpaper query failed: permission denied"
        )))
        .expect_err("unexpected active query failures should remain hard errors");

        assert_eq!(
            error.to_string(),
            "Active wallpaper query failed: permission denied"
        );
    }

    #[test]
    fn multi_monitor_apply_returns_error_when_any_monitor_fails() {
        let error = summarize_multi_monitor_apply_failures(&[
            (
                "HDMI-A-1".to_string(),
                "wallpaper command failed: invalid format".to_string(),
            ),
            (
                "DP-1".to_string(),
                "wallpaper command failed: unknown output".to_string(),
            ),
        ])
        .expect_err("monitor failures should not be reported as success");

        let text = error.to_string();
        assert!(text.contains("Failed to set wallpaper on one or more monitors"));
        assert!(text.contains("HDMI-A-1"));
        assert!(text.contains("DP-1"));
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
