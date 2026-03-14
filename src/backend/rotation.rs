use std::{
    collections::HashSet,
    fs,
    io::{BufRead, BufReader},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use log::{debug, error, info, warn};
use rand::{seq::SliceRandom, Rng};
use serde::{Deserialize, Serialize};

use crate::{
    cache::{IndexedWallpaper, WallpaperIndex},
    config::Config,
};

use super::{
    get_monitors,
    hyprpaper::{
        classify_backend_unavailable, get_active_wallpaper_assignments_if_supported,
        ActiveWallpaperAssignment, BackendUnavailableReason,
    },
    set_wallpaper, set_wallpaper_for_monitor, set_wallpapers_for_monitors, Monitor,
};

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
const BACKEND_RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SortMode {
    Name,
    Modified,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RotationState {
    last_wallpaper: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HyprlandEvent {
    MonitorAdded(String),
    MonitorRemoved(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MonitorHotplugAction {
    SetOnMonitor {
        monitor_name: String,
        wallpaper_path: PathBuf,
    },
    SetOnAllDisplays {
        wallpaper_path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RotationTickOutcome {
    Scheduled(Duration),
    RetrySoon(Duration, BackendUnavailableReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HotplugApplyOutcome {
    Applied,
    RetrySoon(BackendUnavailableReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MonitorAddedPlan {
    Apply(MonitorHotplugAction),
    RetrySoon(BackendUnavailableReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationServiceStatus {
    pub installed: bool,
    pub enabled: String,
    pub active: String,
    pub rotates_all_wallpapers: bool,
    pub same_wallpaper_on_all_displays: bool,
    pub interval_secs: u64,
    pub rotation_entries: usize,
    pub service_file: PathBuf,
}

pub fn install_rotation_service() -> Result<()> {
    info!("installing Walt rotation service");
    let service_dir = service_dir()?;
    fs::create_dir_all(&service_dir)?;
    fs::write(service_file_path()?, service_file_contents()?)?;
    enable_rotation_service()
}

pub fn enable_rotation_service() -> Result<()> {
    info!("enabling Walt rotation service");
    if !service_file_path()?.exists() {
        bail!("Rotation service is not installed. Run 'walt rotation install' first.");
    }

    run_systemctl(&["daemon-reload"])?;
    run_systemctl(&["enable", "--now", SERVICE_FILE])
}

pub fn disable_rotation_service() -> Result<()> {
    info!("disabling Walt rotation service");
    run_systemctl_checked(&["disable", "--now", SERVICE_FILE], BENIGN_DISABLE_ERRORS)
}

pub fn restart_rotation_service_if_active() -> Result<()> {
    debug!("restarting Walt rotation service if active");
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
    info!("uninstalling Walt rotation service");
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
        same_wallpaper_on_all_displays: config.uses_same_wallpaper_on_all_displays_for_rotation(),
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
    info!("starting Walt rotation daemon");
    let index = WallpaperIndex::new()?;
    let mut known_monitors = current_monitor_names();
    let mut pending_monitor_additions = HashSet::new();
    let mut last_transient_backend_message = None;
    let (event_tx, event_rx) = mpsc::channel();
    spawn_hyprland_event_listener(event_tx);
    let mut next_rotation_at = Instant::now()
        + handle_rotation_tick_outcome(
            run_rotation_iteration(&index)?,
            &mut last_transient_backend_message,
        );

    loop {
        pending_monitor_additions.retain(|name| known_monitors.contains(name));
        if let Some(delay) = try_apply_pending_monitor_additions(
            &index,
            &known_monitors,
            &mut pending_monitor_additions,
            &mut last_transient_backend_message,
        )? {
            next_rotation_at = Instant::now()
                + delay.min(next_rotation_at.saturating_duration_since(Instant::now()));
        }

        let timeout = next_rotation_at.saturating_duration_since(Instant::now());
        match event_rx.recv_timeout(timeout) {
            Ok(HyprlandEvent::MonitorAdded(name)) => {
                queue_monitor_addition(&known_monitors, &mut pending_monitor_additions, &name);
                known_monitors = current_monitor_names();
            }
            Ok(HyprlandEvent::MonitorRemoved(name)) => {
                known_monitors = current_monitor_names();
                known_monitors.remove(&name);
                pending_monitor_additions.remove(&name);
            }
            Err(RecvTimeoutError::Timeout) => {
                next_rotation_at = Instant::now()
                    + handle_rotation_tick_outcome(
                        run_rotation_iteration(&index)?,
                        &mut last_transient_backend_message,
                    );
                known_monitors = current_monitor_names();
                pending_monitor_additions.retain(|name| known_monitors.contains(name));
            }
            Err(RecvTimeoutError::Disconnected) => {
                bail!("Hyprland event listener channel disconnected");
            }
        }
    }
}

fn queue_monitor_addition(
    previous_known_monitors: &HashSet<String>,
    pending_monitor_additions: &mut HashSet<String>,
    monitor_name: &str,
) -> bool {
    if should_queue_monitor_addition(previous_known_monitors, monitor_name) {
        pending_monitor_additions.insert(monitor_name.to_string());
        true
    } else {
        false
    }
}

fn should_queue_monitor_addition(
    previous_known_monitors: &HashSet<String>,
    monitor_name: &str,
) -> bool {
    !previous_known_monitors.contains(monitor_name)
}

fn handle_rotation_tick_outcome(
    outcome: RotationTickOutcome,
    last_transient_backend_message: &mut Option<String>,
) -> Duration {
    match outcome {
        RotationTickOutcome::Scheduled(delay) => {
            *last_transient_backend_message = None;
            delay
        }
        RotationTickOutcome::RetrySoon(delay, reason) => {
            log_transient_backend_state(last_transient_backend_message, reason);
            delay
        }
    }
}

fn run_rotation_iteration(index: &WallpaperIndex) -> Result<RotationTickOutcome> {
    let config = Config::new();
    let candidates = rotation_candidates(index, &config);
    let interval = Duration::from_secs(config.rotation_interval_secs.max(1));

    if candidates.is_empty() {
        return Ok(RotationTickOutcome::Scheduled(
            interval.min(Duration::from_secs(30)),
        ));
    }

    let mut state = load_rotation_state()?;
    if config.uses_same_wallpaper_on_all_displays_for_rotation() {
        let next = next_wallpaper(&candidates, state.last_wallpaper.as_ref())
            .cloned()
            .context("Could not determine next rotation wallpaper")?;
        if let Err(error) = set_wallpaper(&next.path.to_string_lossy()) {
            if let Some(reason) = classify_backend_unavailable(&error) {
                return Ok(RotationTickOutcome::RetrySoon(BACKEND_RETRY_DELAY, reason));
            }
            return Err(error);
        }
        state.last_wallpaper = Some(next.path.clone());
    } else {
        let monitors = get_monitors();
        let selection =
            next_wallpaper_assignments(&monitors, &candidates, state.last_wallpaper.as_ref())
                .context("Could not determine next rotation wallpapers")?;
        if let Err(error) = set_wallpapers_for_monitors(&selection.assignments) {
            if let Some(reason) = classify_backend_unavailable(&error) {
                return Ok(RotationTickOutcome::RetrySoon(BACKEND_RETRY_DELAY, reason));
            }
            return Err(error);
        }
        state.last_wallpaper = Some(selection.last_wallpaper);
    }
    save_rotation_state(&state)?;

    Ok(RotationTickOutcome::Scheduled(interval))
}

fn current_monitor_names() -> HashSet<String> {
    get_monitors()
        .into_iter()
        .map(|monitor| monitor.name)
        .collect()
}

fn try_apply_pending_monitor_additions(
    index: &WallpaperIndex,
    known_monitors: &HashSet<String>,
    pending_monitor_additions: &mut HashSet<String>,
    last_transient_backend_message: &mut Option<String>,
) -> Result<Option<Duration>> {
    if pending_monitor_additions.is_empty() {
        return Ok(None);
    }

    let pending = pending_monitor_additions
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let mut should_retry = false;

    for monitor_name in pending {
        if !known_monitors.contains(&monitor_name) {
            pending_monitor_additions.remove(&monitor_name);
            continue;
        }

        match handle_monitor_added(index, &monitor_name)? {
            HotplugApplyOutcome::Applied => {
                pending_monitor_additions.remove(&monitor_name);
                *last_transient_backend_message = None;
            }
            HotplugApplyOutcome::RetrySoon(reason) => {
                should_retry = true;
                log_transient_backend_state(last_transient_backend_message, reason);
            }
        }
    }

    if should_retry {
        Ok(Some(BACKEND_RETRY_DELAY))
    } else {
        Ok(None)
    }
}

fn log_transient_backend_state(
    last_transient_backend_message: &mut Option<String>,
    reason: BackendUnavailableReason,
) {
    let message = transient_backend_message(reason);
    if last_transient_backend_message.as_deref() != Some(message) {
        warn!("{message}");
        *last_transient_backend_message = Some(message.to_string());
    }
}

fn transient_backend_message(reason: BackendUnavailableReason) -> &'static str {
    match reason {
        BackendUnavailableReason::HyprpaperUnavailable => {
            "Hyprpaper is unavailable; Walt rotation will retry shortly."
        }
        BackendUnavailableReason::NoMonitors => {
            "No monitors are available; Walt rotation will retry shortly."
        }
        BackendUnavailableReason::MonitorNotReady => {
            "A monitor is not ready for wallpaper assignment yet; Walt rotation will retry shortly."
        }
    }
}

fn handle_monitor_added(index: &WallpaperIndex, monitor_name: &str) -> Result<HotplugApplyOutcome> {
    info!("handling monitor-added event monitor={monitor_name}");
    let config = Config::new();
    let candidates = rotation_candidates(index, &config);
    if candidates.is_empty() {
        debug!("no rotation candidates available for monitor-added handling");
        return Ok(HotplugApplyOutcome::Applied);
    }

    let mut rng = rand::thread_rng();
    let plan = plan_monitor_added_from_query_result(
        monitor_name,
        &config,
        &candidates,
        get_active_wallpaper_assignments_if_supported(),
        load_rotation_state()?.last_wallpaper.as_ref(),
        &mut rng,
    )?;
    let action = match plan {
        MonitorAddedPlan::Apply(action) => action,
        MonitorAddedPlan::RetrySoon(reason) => {
            warn!("monitor-added planning requested retry reason={:?}", reason);
            return Ok(HotplugApplyOutcome::RetrySoon(reason));
        }
    };

    match action {
        MonitorHotplugAction::SetOnMonitor {
            monitor_name,
            wallpaper_path,
        } => match set_wallpaper_for_monitor(&monitor_name, &wallpaper_path.to_string_lossy()) {
            Ok(()) => {
                info!(
                    "applied hotplug wallpaper to monitor={} path={}",
                    monitor_name,
                    wallpaper_path.display()
                );
                Ok(HotplugApplyOutcome::Applied)
            }
            Err(error) => {
                if let Some(reason) = classify_backend_unavailable(&error) {
                    warn!(
                        "hotplug single-monitor apply transient failure monitor={} reason={:?}",
                        monitor_name, reason
                    );
                    return Ok(HotplugApplyOutcome::RetrySoon(reason));
                }
                error!("hotplug single-monitor apply failed: {error}");
                Err(error)
            }
        },
        MonitorHotplugAction::SetOnAllDisplays { wallpaper_path } => {
            if let Err(error) = set_wallpaper(&wallpaper_path.to_string_lossy()) {
                if let Some(reason) = classify_backend_unavailable(&error) {
                    warn!(
                        "hotplug all-displays apply transient failure path={} reason={:?}",
                        wallpaper_path.display(),
                        reason
                    );
                    return Ok(HotplugApplyOutcome::RetrySoon(reason));
                }
                error!("hotplug all-displays apply failed: {error}");
                return Err(error);
            }
            let mut state = load_rotation_state()?;
            state.last_wallpaper = Some(wallpaper_path);
            save_rotation_state(&state)?;
            info!("applied hotplug wallpaper to all displays");
            Ok(HotplugApplyOutcome::Applied)
        }
    }
}

fn plan_monitor_added_from_query_result<R: Rng + ?Sized>(
    new_monitor_name: &str,
    config: &Config,
    candidates: &[IndexedWallpaper],
    active_assignments_result: Result<Option<Vec<ActiveWallpaperAssignment>>>,
    last_wallpaper: Option<&PathBuf>,
    rng: &mut R,
) -> Result<MonitorAddedPlan> {
    let active_assignments = match active_assignments_result {
        Ok(Some(assignments)) => assignments,
        Ok(None) => {
            debug!("monitor-added planning is using fallback because active-query support is unavailable");
            Vec::new()
        }
        Err(error) => {
            if let Some(reason) = classify_backend_unavailable(&error) {
                return Ok(MonitorAddedPlan::RetrySoon(reason));
            }
            error!("monitor-added planning failed to query active assignments: {error}");
            return Err(error);
        }
    };

    Ok(MonitorAddedPlan::Apply(plan_monitor_added_action(
        new_monitor_name,
        config,
        candidates,
        &active_assignments,
        last_wallpaper,
        rng,
    )?))
}

fn plan_monitor_added_action<R: Rng + ?Sized>(
    new_monitor_name: &str,
    config: &Config,
    candidates: &[IndexedWallpaper],
    active_assignments: &[ActiveWallpaperAssignment],
    last_wallpaper: Option<&PathBuf>,
    rng: &mut R,
) -> Result<MonitorHotplugAction> {
    if config.uses_same_wallpaper_on_all_displays_for_rotation() {
        if let Some(existing) = active_assignments.iter().find(|assignment| {
            assignment.monitor_name != new_monitor_name
                && !assignment.wallpaper_path.as_os_str().is_empty()
        }) {
            return Ok(MonitorHotplugAction::SetOnMonitor {
                monitor_name: new_monitor_name.to_string(),
                wallpaper_path: existing.wallpaper_path.clone(),
            });
        }

        let next = next_wallpaper(candidates, last_wallpaper)
            .cloned()
            .context("Could not determine next rotation wallpaper")?;
        return Ok(MonitorHotplugAction::SetOnAllDisplays {
            wallpaper_path: next.path,
        });
    }

    let candidate_paths = candidates
        .iter()
        .map(|wallpaper| wallpaper.path.clone())
        .collect::<Vec<_>>();
    let active_paths = active_assignments
        .iter()
        .filter(|assignment| assignment.monitor_name != new_monitor_name)
        .map(|assignment| assignment.wallpaper_path.clone())
        .collect::<HashSet<_>>();
    let wallpaper_path = choose_hotplug_wallpaper_with_rng(&candidate_paths, &active_paths, rng)?;

    Ok(MonitorHotplugAction::SetOnMonitor {
        monitor_name: new_monitor_name.to_string(),
        wallpaper_path,
    })
}

fn choose_hotplug_wallpaper_with_rng<R: Rng + ?Sized>(
    candidates: &[PathBuf],
    active_paths: &HashSet<PathBuf>,
    rng: &mut R,
) -> Result<PathBuf> {
    let preferred = candidates
        .iter()
        .filter(|candidate| !active_paths.contains(*candidate))
        .cloned()
        .collect::<Vec<_>>();
    let pool = if preferred.is_empty() {
        candidates
    } else {
        preferred.as_slice()
    };

    pool.choose(rng)
        .cloned()
        .context("No rotation wallpapers available for monitor hotplug")
}

fn parse_hyprland_event(line: &str) -> Option<HyprlandEvent> {
    let (event_name, data) = line.trim().split_once(">>")?;
    match event_name {
        "monitoradded" => parse_direct_monitor_event(data).map(HyprlandEvent::MonitorAdded),
        "monitorremoved" => parse_direct_monitor_event(data).map(HyprlandEvent::MonitorRemoved),
        "monitoraddedv2" => parse_v2_monitor_event(data).map(HyprlandEvent::MonitorAdded),
        "monitorremovedv2" => parse_v2_monitor_event(data).map(HyprlandEvent::MonitorRemoved),
        _ => None,
    }
}

fn parse_direct_monitor_event(data: &str) -> Option<String> {
    let name = data.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn parse_v2_monitor_event(data: &str) -> Option<String> {
    let mut fields = data.split(',');
    fields.next()?;
    let name = fields.next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn socket2_path() -> Result<PathBuf> {
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
        .context("XDG_RUNTIME_DIR is not set for Hyprland event listener")?;
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .context("HYPRLAND_INSTANCE_SIGNATURE is not set for Hyprland event listener")?;

    Ok(PathBuf::from(runtime_dir)
        .join("hypr")
        .join(signature)
        .join(".socket2.sock"))
}

fn spawn_hyprland_event_listener(tx: Sender<HyprlandEvent>) {
    thread::spawn(move || loop {
        let socket_path = match socket2_path() {
            Ok(path) => path,
            Err(error) => {
                error!("failed to resolve Hyprland event socket path: {error}");
                thread::sleep(Duration::from_secs(5));
                continue;
            }
        };

        let stream = match UnixStream::connect(&socket_path) {
            Ok(stream) => stream,
            Err(error) => {
                error!(
                    "Failed to connect to Hyprland event socket {}: {}",
                    socket_path.display(),
                    error
                );
                thread::sleep(Duration::from_secs(5));
                continue;
            }
        };

        let reader = BufReader::new(stream);
        let mut disconnected = false;
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if let Some(event) = parse_hyprland_event(&line) {
                        info!("received Hyprland event {:?}", event);
                        if tx.send(event).is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    error!("failed to read Hyprland event socket: {error}");
                    disconnected = true;
                    break;
                }
            }
        }

        if !disconnected {
            warn!("Hyprland event socket disconnected; retrying");
        }
        thread::sleep(Duration::from_secs(5));
    });
}

fn rotation_candidates(index: &WallpaperIndex, config: &Config) -> Vec<IndexedWallpaper> {
    let wallpapers = index
        .refresh(&config.wallpaper_paths)
        .unwrap_or_else(|_| index.load(&config.wallpaper_paths));
    let mut wallpapers = filter_wallpapers_for_rotation(wallpapers, config);
    debug!("rotation candidate count={}", wallpapers.len());

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

#[derive(Debug, PartialEq, Eq)]
struct RotationAssignmentSelection {
    assignments: Vec<(String, PathBuf)>,
    last_wallpaper: PathBuf,
}

fn next_wallpaper_assignments(
    monitors: &[Monitor],
    wallpapers: &[IndexedWallpaper],
    last_wallpaper: Option<&PathBuf>,
) -> Option<RotationAssignmentSelection> {
    if monitors.is_empty() || wallpapers.is_empty() {
        return None;
    }

    let start_index = wallpapers
        .iter()
        .position(|wallpaper| Some(&wallpaper.path) == last_wallpaper)
        .map(|index| (index + 1) % wallpapers.len())
        .unwrap_or(0);

    let mut assignments = Vec::with_capacity(monitors.len());
    for (offset, monitor) in monitors.iter().enumerate() {
        let wallpaper = &wallpapers[(start_index + offset) % wallpapers.len()];
        assignments.push((monitor.name.clone(), wallpaper.path.clone()));
    }

    let last_wallpaper = assignments.last()?.1.clone();

    Some(RotationAssignmentSelection {
        assignments,
        last_wallpaper,
    })
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

pub fn format_rotation_service_status(status: &RotationServiceStatus) -> String {
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
Displays: {}\n\
Interval: {}\n\
Entries:  {}",
        status.enabled,
        status.active,
        rotation_mode_label(status.rotates_all_wallpapers),
        rotation_display_mode_label(status.same_wallpaper_on_all_displays),
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

fn rotation_display_mode_label(same_wallpaper_on_all_displays: bool) -> &'static str {
    if same_wallpaper_on_all_displays {
        "same on all displays"
    } else {
        "different per display"
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
        choose_hotplug_wallpaper_with_rng, filter_wallpapers_for_rotation,
        format_rotation_service_status, handle_rotation_tick_outcome,
        is_tolerated_systemctl_failure, next_wallpaper, next_wallpaper_assignments,
        parse_hyprland_event, plan_monitor_added_action, plan_monitor_added_from_query_result,
        queue_monitor_addition, service_file_contents, should_queue_monitor_addition,
        transient_backend_message, BackendUnavailableReason, HyprlandEvent, Monitor,
        MonitorAddedPlan, MonitorHotplugAction, RotationServiceStatus, RotationTickOutcome,
        BACKEND_RETRY_DELAY,
    };
    use crate::backend::hyprpaper::{classify_backend_unavailable, ActiveWallpaperAssignment};
    use crate::cache::IndexedWallpaper;
    use crate::config::Config;
    use rand::{rngs::StdRng, SeedableRng};
    use std::{collections::HashSet, path::PathBuf, time::Duration};

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

    fn config(
        rotation: Vec<&str>,
        rotate_all_wallpapers: bool,
        same_on_all_displays: bool,
    ) -> Config {
        Config {
            wallpaper_paths: vec![],
            theme_name: "System".to_string(),
            rotation: rotation
                .into_iter()
                .map(|name| PathBuf::from(format!("/tmp/{name}.png")))
                .collect(),
            rotate_all_wallpapers,
            rotation_same_wallpaper_on_all_displays: same_on_all_displays,
            rotation_interval_secs: 300,
            all_sort: "name".to_string(),
            rotation_sort: "name".to_string(),
        }
    }

    fn active_assignment(monitor_name: &str, wallpaper_name: &str) -> ActiveWallpaperAssignment {
        ActiveWallpaperAssignment {
            monitor_name: monitor_name.to_string(),
            wallpaper_path: PathBuf::from(format!("/tmp/{wallpaper_name}.png")),
        }
    }

    fn monitors(names: &[&str]) -> Vec<Monitor> {
        names
            .iter()
            .map(|name| Monitor {
                name: (*name).to_string(),
            })
            .collect()
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
        let filtered =
            filter_wallpapers_for_rotation(wallpapers, &config(vec!["beta"], false, true));

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "beta");
    }

    #[test]
    fn keeps_all_wallpapers_when_rotate_all_is_on() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")];
        let filtered =
            filter_wallpapers_for_rotation(wallpapers, &config(vec!["beta"], true, true));

        assert_eq!(filtered.len(), 3);
        assert_eq!(filtered[0].name, "alpha");
        assert_eq!(filtered[1].name, "beta");
        assert_eq!(filtered[2].name, "gamma");
    }

    #[test]
    fn selects_consecutive_wallpapers_for_multiple_displays() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")];
        let selection = next_wallpaper_assignments(
            &monitors(&["eDP-1", "DP-1"]),
            &wallpapers,
            Some(&wallpapers[0].path),
        )
        .expect("selection");

        assert_eq!(
            selection.assignments,
            vec![
                ("eDP-1".to_string(), PathBuf::from("/tmp/beta.png")),
                ("DP-1".to_string(), PathBuf::from("/tmp/gamma.png")),
            ]
        );
        assert_eq!(selection.last_wallpaper, PathBuf::from("/tmp/gamma.png"));
    }

    #[test]
    fn repeats_wallpapers_only_after_sequence_wraps_for_multiple_displays() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta")];
        let selection = next_wallpaper_assignments(
            &monitors(&["eDP-1", "DP-1", "HDMI-A-1"]),
            &wallpapers,
            None,
        )
        .expect("selection");

        assert_eq!(
            selection.assignments,
            vec![
                ("eDP-1".to_string(), PathBuf::from("/tmp/alpha.png")),
                ("DP-1".to_string(), PathBuf::from("/tmp/beta.png")),
                ("HDMI-A-1".to_string(), PathBuf::from("/tmp/alpha.png")),
            ]
        );
        assert_eq!(selection.last_wallpaper, PathBuf::from("/tmp/alpha.png"));
    }

    #[test]
    fn returns_none_for_multi_display_assignments_without_monitors() {
        let wallpapers = vec![wallpaper("alpha")];
        assert!(next_wallpaper_assignments(&[], &wallpapers, None).is_none());
    }

    #[test]
    fn parses_monitor_added_event() {
        assert_eq!(
            parse_hyprland_event("monitoradded>>DP-1"),
            Some(HyprlandEvent::MonitorAdded("DP-1".to_string()))
        );
    }

    #[test]
    fn parses_monitor_added_v2_event() {
        assert_eq!(
            parse_hyprland_event("monitoraddedv2>>3,DP-1,Dell Inc."),
            Some(HyprlandEvent::MonitorAdded("DP-1".to_string()))
        );
    }

    #[test]
    fn parses_monitor_removed_event() {
        assert_eq!(
            parse_hyprland_event("monitorremoved>>HDMI-A-1"),
            Some(HyprlandEvent::MonitorRemoved("HDMI-A-1".to_string()))
        );
    }

    #[test]
    fn parses_monitor_removed_v2_event() {
        assert_eq!(
            parse_hyprland_event("monitorremovedv2>>7,eDP-1,Laptop Panel"),
            Some(HyprlandEvent::MonitorRemoved("eDP-1".to_string()))
        );
    }

    #[test]
    fn ignores_unrelated_hyprland_events() {
        assert_eq!(parse_hyprland_event("workspace>>1"), None);
        assert_eq!(parse_hyprland_event("monitoraddedv2>>"), None);
    }

    #[test]
    fn same_on_all_hotplug_mirrors_existing_wallpaper() {
        let mut rng = StdRng::seed_from_u64(7);
        let action = plan_monitor_added_action(
            "DP-1",
            &config(vec!["alpha", "beta"], false, true),
            &vec![wallpaper("alpha"), wallpaper("beta")],
            &[active_assignment("eDP-1", "beta")],
            Some(&PathBuf::from("/tmp/alpha.png")),
            &mut rng,
        )
        .expect("action");

        assert_eq!(
            action,
            MonitorHotplugAction::SetOnMonitor {
                monitor_name: "DP-1".to_string(),
                wallpaper_path: PathBuf::from("/tmp/beta.png"),
            }
        );
    }

    #[test]
    fn same_on_all_hotplug_falls_back_to_next_rotation_wallpaper() {
        let mut rng = StdRng::seed_from_u64(7);
        let action = plan_monitor_added_action(
            "DP-1",
            &config(vec!["alpha", "beta"], false, true),
            &vec![wallpaper("alpha"), wallpaper("beta")],
            &[],
            Some(&PathBuf::from("/tmp/alpha.png")),
            &mut rng,
        )
        .expect("action");

        assert_eq!(
            action,
            MonitorHotplugAction::SetOnAllDisplays {
                wallpaper_path: PathBuf::from("/tmp/beta.png"),
            }
        );
    }

    #[test]
    fn different_per_display_hotplug_prefers_non_active_wallpaper() {
        let mut rng = StdRng::seed_from_u64(7);
        let action = plan_monitor_added_action(
            "DP-1",
            &config(vec!["alpha", "beta", "gamma"], true, false),
            &vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")],
            &[
                active_assignment("eDP-1", "alpha"),
                active_assignment("HDMI-A-1", "beta"),
            ],
            None,
            &mut rng,
        )
        .expect("action");

        assert_eq!(
            action,
            MonitorHotplugAction::SetOnMonitor {
                monitor_name: "DP-1".to_string(),
                wallpaper_path: PathBuf::from("/tmp/gamma.png"),
            }
        );
    }

    #[test]
    fn different_per_display_hotplug_falls_back_to_full_pool() {
        let mut rng = StdRng::seed_from_u64(7);
        let picked = choose_hotplug_wallpaper_with_rng(
            &[
                PathBuf::from("/tmp/alpha.png"),
                PathBuf::from("/tmp/beta.png"),
            ],
            &HashSet::from([
                PathBuf::from("/tmp/alpha.png"),
                PathBuf::from("/tmp/beta.png"),
            ]),
            &mut rng,
        )
        .expect("wallpaper");

        assert!(
            picked == PathBuf::from("/tmp/alpha.png") || picked == PathBuf::from("/tmp/beta.png")
        );
    }

    #[test]
    fn hotplug_random_selection_errors_without_candidates() {
        let mut rng = StdRng::seed_from_u64(7);
        let error = choose_hotplug_wallpaper_with_rng(&[], &HashSet::new(), &mut rng)
            .expect_err("empty candidates should fail");

        assert!(error
            .to_string()
            .contains("No rotation wallpapers available for monitor hotplug"));
    }

    #[test]
    fn monitor_add_event_queues_monitor_when_missing_from_previous_known_set() {
        let previous_known = HashSet::from(["eDP-1".to_string()]);
        let refreshed_known = HashSet::from(["eDP-1".to_string(), "DP-1".to_string()]);
        let mut pending = HashSet::new();

        assert!(queue_monitor_addition(
            &previous_known,
            &mut pending,
            "DP-1"
        ));
        assert!(pending.contains("DP-1"));
        assert!(refreshed_known.contains("DP-1"));
    }

    #[test]
    fn monitor_add_event_skips_duplicate_when_already_in_previous_known_set() {
        let previous_known = HashSet::from(["eDP-1".to_string(), "DP-1".to_string()]);
        let refreshed_known = HashSet::from(["eDP-1".to_string(), "DP-1".to_string()]);
        let mut pending = HashSet::new();

        assert!(!queue_monitor_addition(
            &previous_known,
            &mut pending,
            "DP-1"
        ));
        assert!(!pending.contains("DP-1"));
        assert!(refreshed_known.contains("DP-1"));
    }

    #[test]
    fn monitor_removed_then_monitor_added_queues_again() {
        let previous_known = HashSet::from(["eDP-1".to_string()]);
        let mut pending = HashSet::new();

        assert!(should_queue_monitor_addition(&previous_known, "DP-1"));
        assert!(queue_monitor_addition(
            &previous_known,
            &mut pending,
            "DP-1"
        ));
        assert!(pending.contains("DP-1"));
    }

    #[test]
    fn daemon_event_sequence_populates_pending_queue_for_new_attach() {
        let previous_known = HashSet::from(["eDP-1".to_string()]);
        let refreshed_known = HashSet::from(["eDP-1".to_string(), "DP-1".to_string()]);
        let mut pending = HashSet::new();

        let queued = queue_monitor_addition(&previous_known, &mut pending, "DP-1");

        assert!(queued);
        assert!(pending.contains("DP-1"));
        assert!(refreshed_known.contains("DP-1"));
    }

    #[test]
    fn retry_tick_keeps_transient_message_until_success() {
        let mut last_message = None;
        let retry_delay = handle_rotation_tick_outcome(
            RotationTickOutcome::RetrySoon(
                BACKEND_RETRY_DELAY,
                BackendUnavailableReason::HyprpaperUnavailable,
            ),
            &mut last_message,
        );

        assert_eq!(retry_delay, BACKEND_RETRY_DELAY);
        assert_eq!(
            last_message,
            Some(
                transient_backend_message(BackendUnavailableReason::HyprpaperUnavailable)
                    .to_string()
            )
        );

        let scheduled = handle_rotation_tick_outcome(
            RotationTickOutcome::Scheduled(Duration::from_secs(300)),
            &mut last_message,
        );
        assert_eq!(scheduled, Duration::from_secs(300));
        assert_eq!(last_message, None);
    }

    #[test]
    fn same_on_all_hotplug_returns_retry_when_backend_is_unavailable() {
        let error = anyhow::anyhow!(
            "Active wallpaper query failed: status exit status: 3, stdout: Couldn't connect to /run/user/1000/hypr/instance/.hyprpaper.sock. (3)"
        );

        assert_eq!(
            classify_backend_unavailable(&error),
            Some(BackendUnavailableReason::HyprpaperUnavailable)
        );
    }

    #[test]
    fn same_on_all_hotplug_uses_fallback_when_active_query_is_unsupported() {
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_monitor_added_from_query_result(
            "DP-1",
            &config(vec!["alpha", "beta"], false, true),
            &vec![wallpaper("alpha"), wallpaper("beta")],
            Ok(None),
            Some(&PathBuf::from("/tmp/alpha.png")),
            &mut rng,
        )
        .expect("plan");

        assert_eq!(
            plan,
            MonitorAddedPlan::Apply(MonitorHotplugAction::SetOnAllDisplays {
                wallpaper_path: PathBuf::from("/tmp/beta.png"),
            })
        );
    }

    #[test]
    fn different_per_display_hotplug_uses_full_pool_when_active_query_is_unsupported() {
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_monitor_added_from_query_result(
            "DP-1",
            &config(vec!["alpha", "beta", "gamma"], true, false),
            &vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")],
            Ok(None),
            None,
            &mut rng,
        )
        .expect("plan");

        assert_eq!(
            plan,
            MonitorAddedPlan::Apply(MonitorHotplugAction::SetOnMonitor {
                monitor_name: "DP-1".to_string(),
                wallpaper_path: PathBuf::from("/tmp/beta.png"),
            })
        );
    }

    #[test]
    fn transient_active_query_errors_still_request_retry() {
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_monitor_added_from_query_result(
            "DP-1",
            &config(vec!["alpha", "beta"], false, true),
            &vec![wallpaper("alpha"), wallpaper("beta")],
            Err(anyhow::anyhow!(
                "Active wallpaper query failed: status exit status: 3, stdout: Couldn't connect to /run/user/1000/hypr/instance/.hyprpaper.sock. (3)"
            )),
            Some(&PathBuf::from("/tmp/alpha.png")),
            &mut rng,
        )
        .expect("transient backend errors should not fail");

        assert_eq!(
            plan,
            MonitorAddedPlan::RetrySoon(BackendUnavailableReason::HyprpaperUnavailable)
        );
    }

    #[test]
    fn pending_monitor_queue_retains_monitor_until_success() {
        let known_monitors = HashSet::from(["DP-1".to_string()]);
        let mut pending = HashSet::from(["DP-1".to_string()]);

        pending.retain(|name| known_monitors.contains(name));
        assert!(pending.contains("DP-1"));

        pending.remove("DP-1");
        assert!(pending.is_empty());
    }

    #[test]
    fn pending_monitor_queue_drops_removed_monitor() {
        let known_monitors = HashSet::from(["eDP-1".to_string()]);
        let mut pending = HashSet::from(["DP-1".to_string()]);

        pending.retain(|name| known_monitors.contains(name));
        assert!(pending.is_empty());
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
            same_wallpaper_on_all_displays: true,
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
        assert!(text.contains("Displays: same on all displays"));
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
            same_wallpaper_on_all_displays: true,
            interval_secs: 45,
            rotation_entries: 1,
            service_file: PathBuf::from("/tmp/walt-rotation.service"),
        });

        assert!(text.contains("Status:   not installed"));
        assert!(text.contains("Loaded:   not found (/tmp/walt-rotation.service)"));
        assert!(text.contains("Enabled:  not installed"));
        assert!(text.contains("Active:   inactive"));
        assert!(text.contains("Mode:     selected wallpapers"));
        assert!(text.contains("Displays: same on all displays"));
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
            same_wallpaper_on_all_displays: false,
            interval_secs: 300,
            rotation_entries: 4,
            service_file: PathBuf::from("/tmp/walt-rotation.service"),
        });

        assert!(text.contains("Mode:     all wallpapers"));
        assert!(text.contains("Displays: different per display"));
        assert!(text.contains("Entries:  all wallpapers"));
    }
}
