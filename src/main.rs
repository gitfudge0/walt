mod backend;
mod cache;
mod config;
mod gui;
mod shared;
mod theme;
mod ui;

use std::env;
use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{bail, Context, Result};

const ROTATION_OPTIONS: &str = "install, enable, disable, uninstall, status, interval";
const ROTATION_USAGE: &str = "walt rotation <install|enable|disable|uninstall|status|interval>";
const RANDOM_USAGE: &str = "walt random [--same|DISPLAY_INDEX]";
const UNINSTALL_USAGE: &str = "walt uninstall [--yes]";

#[derive(Debug, Eq, PartialEq)]
enum CliCommand {
    LaunchUi,
    Gui,
    Help,
    Random(RandomCommand),
    Uninstall { yes: bool },
    RotateDaemon,
    Rotation(RotationCommand),
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum RandomCommand {
    DifferentAll,
    SameAll,
    DisplayIndex(usize),
}

#[derive(Debug, Eq, PartialEq)]
enum RotationCommand {
    Install,
    Enable,
    Disable,
    Uninstall,
    Status,
    Interval(u64),
}

fn main() -> Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let argv = args.iter().map(String::as_str).collect::<Vec<_>>();

    match parse_command(&argv)? {
        CliCommand::LaunchUi => {}
        CliCommand::Gui => return gui::run(),
        CliCommand::Help => {
            print_usage();
            return Ok(());
        }
        CliCommand::Random(command) => return run_random_wallpaper(command),
        CliCommand::Uninstall { yes } => return run_uninstall(yes),
        CliCommand::RotateDaemon => return backend::run_rotation_daemon(),
        CliCommand::Rotation(command) => {
            match command {
                RotationCommand::Install => {
                    backend::install_rotation_service()?;
                    println!("Installed and started Walt rotation service.");
                }
                RotationCommand::Enable => {
                    backend::enable_rotation_service()?;
                    println!("Enabled and started Walt rotation service.");
                }
                RotationCommand::Disable => {
                    backend::disable_rotation_service()?;
                    println!("Disabled and stopped Walt rotation service.");
                }
                RotationCommand::Uninstall => {
                    backend::uninstall_rotation_service()?;
                    println!("Removed Walt rotation service.");
                }
                RotationCommand::Status => {
                    println!("{}", backend::rotation_service_status()?);
                }
                RotationCommand::Interval(seconds) => {
                    set_rotation_interval(seconds)?;
                    println!("Rotation interval set to {}.", format_interval(seconds));
                }
            }
            return Ok(());
        }
    }

    let mut app = ui::App::new()?;
    app.run()?;
    Ok(())
}

fn parse_command(args: &[&str]) -> Result<CliCommand> {
    match args {
        [] => Ok(CliCommand::LaunchUi),
        ["gui"] => Ok(CliCommand::Gui),
        ["random"] => Ok(CliCommand::Random(RandomCommand::DifferentAll)),
        ["random", "--same"] => Ok(CliCommand::Random(RandomCommand::SameAll)),
        ["random", display_index] => {
            let Ok(display_index) = display_index.parse::<usize>() else {
                bail!("Usage: {RANDOM_USAGE}");
            };
            Ok(CliCommand::Random(RandomCommand::DisplayIndex(
                display_index,
            )))
        }
        ["random", ..] => bail!("Usage: {RANDOM_USAGE}"),
        ["uninstall"] => Ok(CliCommand::Uninstall { yes: false }),
        ["uninstall", "--yes"] | ["uninstall", "-y"] => Ok(CliCommand::Uninstall { yes: true }),
        ["uninstall", ..] => bail!("Usage: {UNINSTALL_USAGE}"),
        ["rotation"] => bail!("Missing rotation option. Valid options: {ROTATION_OPTIONS}"),
        ["rotation", "interval"] => bail!("Usage: walt rotation interval <seconds>"),
        ["rotation", "interval", seconds] => Ok(CliCommand::Rotation(RotationCommand::Interval(
            parse_rotation_interval(seconds)?,
        ))),
        ["rotation", action] => Ok(CliCommand::Rotation(parse_rotation_command(action)?)),
        ["rotation", _, ..] => bail!("Usage: {ROTATION_USAGE}"),
        ["--rotate-daemon"] => Ok(CliCommand::RotateDaemon),
        ["--help"] | ["-h"] => Ok(CliCommand::Help),
        [arg, ..] => bail!("Unknown argument: {arg}"),
    }
}

fn parse_rotation_command(action: &str) -> Result<RotationCommand> {
    match action {
        "install" => Ok(RotationCommand::Install),
        "enable" => Ok(RotationCommand::Enable),
        "disable" => Ok(RotationCommand::Disable),
        "uninstall" => Ok(RotationCommand::Uninstall),
        "status" => Ok(RotationCommand::Status),
        "interval" => bail!("Usage: walt rotation interval <seconds>"),
        _ => bail!("Unknown rotation option: {action}"),
    }
}

fn parse_rotation_interval(value: &str) -> Result<u64> {
    let seconds = value.parse::<u64>().with_context(|| {
        format!("Invalid rotation interval: {value}. Expected a positive number of seconds.")
    })?;

    if seconds == 0 {
        bail!("Invalid rotation interval: {value}. Expected a positive number of seconds.");
    }

    Ok(seconds)
}

fn run_random_wallpaper(command: RandomCommand) -> Result<()> {
    let config = config::Config::new();

    if config.is_empty() {
        bail!("No wallpaper directories configured. Launch walt once to add paths.")
    }

    let wallpapers = backend::scan_wallpapers_from_paths(&config.wallpaper_paths);
    let wallpaper_paths = wallpapers
        .into_iter()
        .map(|wallpaper| wallpaper.path)
        .collect::<Vec<_>>();
    let monitors = backend::get_monitors();
    let random_mode = match command {
        RandomCommand::DifferentAll => backend::RandomMode::DifferentAll,
        RandomCommand::SameAll => backend::RandomMode::SameAll,
        RandomCommand::DisplayIndex(index) => backend::RandomMode::DisplayIndex(index),
    };
    let plan = backend::plan_random_assignments(&monitors, &wallpaper_paths, random_mode)?;

    backend::apply_random_plan(&plan)?;
    print_random_summary(&plan);
    Ok(())
}

fn print_random_summary(plan: &backend::RandomPlan) {
    match &plan.mode {
        backend::RandomMode::DifferentAll => {
            println!("Applied random wallpapers to all displays:");
            for assignment in &plan.assignments {
                println!(
                    "  {} -> {}",
                    assignment.monitor_name,
                    assignment.wallpaper_path.display()
                );
            }
        }
        backend::RandomMode::SameAll => {
            if let Some(assignment) = plan.assignments.first() {
                println!(
                    "Applied the same random wallpaper to all displays: {}",
                    assignment.wallpaper_path.display()
                );
            }
        }
        backend::RandomMode::DisplayIndex(requested) => {
            if let Some(assignment) = plan.assignments.first() {
                let resolved = plan.resolved_display_index.unwrap_or(*requested);
                if resolved == *requested {
                    println!(
                        "Applied a random wallpaper to display {} ({}): {}",
                        resolved,
                        assignment.monitor_name,
                        assignment.wallpaper_path.display()
                    );
                } else {
                    println!(
                        "Applied a random wallpaper to display {} (clamped from {}, {}): {}",
                        resolved,
                        requested,
                        assignment.monitor_name,
                        assignment.wallpaper_path.display()
                    );
                }
            }
        }
    }
}

fn run_uninstall(yes: bool) -> Result<()> {
    let paths = backend::uninstall_paths()?;
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stdin_is_terminal = stdin.is_terminal();
    let stdout_is_terminal = stdout.is_terminal();
    let mut stdin_lock = stdin.lock();
    let mut stdout_lock = stdout.lock();
    let confirmed = confirm_uninstall_with_io(
        yes,
        &paths,
        stdin_is_terminal,
        stdout_is_terminal,
        &mut stdin_lock,
        &mut stdout_lock,
    )?;

    if !confirmed {
        println!("Walt uninstall cancelled.");
        return Ok(());
    }

    let report = backend::uninstall_walt()?;
    println!("{}", report.summary());
    Ok(())
}

fn confirm_uninstall_with_io<R, W>(
    yes: bool,
    paths: &backend::UninstallPaths,
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
    input: &mut R,
    output: &mut W,
) -> Result<bool>
where
    R: BufRead,
    W: Write,
{
    if yes {
        return Ok(true);
    }

    if !(stdin_is_terminal && stdout_is_terminal) {
        bail!("walt uninstall requires confirmation in an interactive terminal. Re-run with `walt uninstall --yes`.");
    }

    writeln!(output, "This will remove Walt from this system:")?;
    writeln!(
        output,
        "  - rotation service: {}",
        paths.service_file.display()
    )?;
    writeln!(output, "  - config: {}", paths.config_dir.display())?;
    writeln!(output, "  - cache: {}", paths.cache_dir.display())?;
    writeln!(output, "  - binary: {}", paths.binary_path.display())?;
    write!(output, "Continue? [y/N]: ")?;
    output.flush()?;

    let mut answer = String::new();
    input.read_line(&mut answer)?;
    let answer = answer.trim().to_ascii_lowercase();
    Ok(matches!(answer.as_str(), "y" | "yes"))
}

fn print_usage() {
    println!("{}", usage_text());
}

fn usage_text() -> String {
    [
        "Walt",
        "",
        "Usage:",
        "  walt gui",
        "  walt random [--same|DISPLAY_INDEX]",
        "  walt uninstall [--yes]",
        "  walt rotation <command>",
        "",
        "Commands:",
        "  gui                       Launch the desktop GUI",
        "  random [--same|index]     Apply random wallpaper(s), zero-based display index clamps to last",
        "  uninstall [--yes]         Remove Walt service, config, cache, and ~/.local/bin/walt",
        "  rotation install          Install and start the persistent rotation service",
        "  rotation enable           Enable and start the installed rotation service",
        "  rotation disable          Disable and stop the installed rotation service",
        "  rotation uninstall        Stop and remove the persistent rotation service",
        "  rotation status           Show formatted rotation service status, including activity",
        "  rotation interval <secs>  Set the rotation interval in seconds",
        "",
        "Examples:",
        "  walt gui",
        "  walt random",
        "  walt random --same",
        "  walt random 0",
        "  walt uninstall --yes",
        "  walt rotation install",
        "  walt rotation interval 900",
        "  walt rotation status",
    ]
    .join("\n")
}

fn set_rotation_interval(seconds: u64) -> Result<()> {
    let mut config = config::Config::new();
    config.set_rotation_interval_secs(seconds)
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

#[cfg(test)]
mod tests {
    use super::{
        confirm_uninstall_with_io, parse_command, usage_text, CliCommand, RandomCommand,
        RotationCommand,
    };
    use crate::backend::UninstallPaths;
    use std::io::Cursor;
    use std::path::PathBuf;

    fn uninstall_paths() -> UninstallPaths {
        UninstallPaths {
            service_file: PathBuf::from("/tmp/walt-rotation.service"),
            config_dir: PathBuf::from("/tmp/config/walt"),
            cache_dir: PathBuf::from("/tmp/cache/walt"),
            binary_path: PathBuf::from("/tmp/.local/bin/walt"),
        }
    }

    #[test]
    fn parses_random_command() {
        assert_eq!(
            parse_command(&["random"]).expect("command"),
            CliCommand::Random(RandomCommand::DifferentAll)
        );
    }

    #[test]
    fn parses_gui_command() {
        assert_eq!(parse_command(&["gui"]).expect("command"), CliCommand::Gui);
    }

    #[test]
    fn parses_random_same_command() {
        assert_eq!(
            parse_command(&["random", "--same"]).expect("command"),
            CliCommand::Random(RandomCommand::SameAll)
        );
    }

    #[test]
    fn parses_random_display_index_command() {
        assert_eq!(
            parse_command(&["random", "0"]).expect("command"),
            CliCommand::Random(RandomCommand::DisplayIndex(0))
        );
    }

    #[test]
    fn parses_uninstall_command() {
        assert_eq!(
            parse_command(&["uninstall"]).expect("command"),
            CliCommand::Uninstall { yes: false }
        );
    }

    #[test]
    fn parses_uninstall_yes_long_flag() {
        assert_eq!(
            parse_command(&["uninstall", "--yes"]).expect("command"),
            CliCommand::Uninstall { yes: true }
        );
    }

    #[test]
    fn parses_uninstall_yes_short_flag() {
        assert_eq!(
            parse_command(&["uninstall", "-y"]).expect("command"),
            CliCommand::Uninstall { yes: true }
        );
    }

    #[test]
    fn parses_rotation_install_command() {
        assert_eq!(
            parse_command(&["rotation", "install"]).expect("command"),
            CliCommand::Rotation(RotationCommand::Install)
        );
    }

    #[test]
    fn parses_rotation_enable_command() {
        assert_eq!(
            parse_command(&["rotation", "enable"]).expect("command"),
            CliCommand::Rotation(RotationCommand::Enable)
        );
    }

    #[test]
    fn parses_rotation_disable_command() {
        assert_eq!(
            parse_command(&["rotation", "disable"]).expect("command"),
            CliCommand::Rotation(RotationCommand::Disable)
        );
    }

    #[test]
    fn parses_rotation_uninstall_command() {
        assert_eq!(
            parse_command(&["rotation", "uninstall"]).expect("command"),
            CliCommand::Rotation(RotationCommand::Uninstall)
        );
    }

    #[test]
    fn parses_rotation_status_command() {
        assert_eq!(
            parse_command(&["rotation", "status"]).expect("command"),
            CliCommand::Rotation(RotationCommand::Status)
        );
    }

    #[test]
    fn parses_rotation_interval_command() {
        assert_eq!(
            parse_command(&["rotation", "interval", "900"]).expect("command"),
            CliCommand::Rotation(RotationCommand::Interval(900))
        );
    }

    #[test]
    fn rejects_missing_rotation_option() {
        let error = parse_command(&["rotation"]).expect_err("missing option should fail");
        assert_eq!(
            error.to_string(),
            "Missing rotation option. Valid options: install, enable, disable, uninstall, status, interval"
        );
    }

    #[test]
    fn rejects_unknown_rotation_option() {
        let error = parse_command(&["rotation", "bogus"]).expect_err("unknown option should fail");
        assert_eq!(error.to_string(), "Unknown rotation option: bogus");
    }

    #[test]
    fn rejects_random_with_extra_arguments() {
        let error = parse_command(&["random", "extra"]).expect_err("extra args should fail");
        assert_eq!(
            error.to_string(),
            "Usage: walt random [--same|DISPLAY_INDEX]"
        );
    }

    #[test]
    fn rejects_random_same_with_extra_arguments() {
        let error = parse_command(&["random", "--same", "1"]).expect_err("extra args should fail");
        assert_eq!(
            error.to_string(),
            "Usage: walt random [--same|DISPLAY_INDEX]"
        );
    }

    #[test]
    fn rejects_uninstall_with_extra_arguments() {
        let error = parse_command(&["uninstall", "extra"]).expect_err("extra args should fail");
        assert_eq!(error.to_string(), "Usage: walt uninstall [--yes]");
    }

    #[test]
    fn rejects_rotation_option_with_extra_arguments() {
        let error =
            parse_command(&["rotation", "install", "extra"]).expect_err("extra args should fail");
        assert_eq!(
            error.to_string(),
            "Usage: walt rotation <install|enable|disable|uninstall|status|interval>"
        );
    }

    #[test]
    fn rejects_missing_rotation_interval_value() {
        let error =
            parse_command(&["rotation", "interval"]).expect_err("missing interval should fail");
        assert_eq!(error.to_string(), "Usage: walt rotation interval <seconds>");
    }

    #[test]
    fn rejects_invalid_rotation_interval_value() {
        let error =
            parse_command(&["rotation", "interval", "0"]).expect_err("zero interval should fail");
        assert_eq!(
            error.to_string(),
            "Invalid rotation interval: 0. Expected a positive number of seconds."
        );
    }

    #[test]
    fn help_text_only_mentions_public_commands() {
        let usage = usage_text();
        assert!(usage.contains("Usage:"));
        assert!(usage.contains("Commands:"));
        assert!(usage.contains("Examples:"));
        assert!(usage.contains("walt gui"));
        assert!(usage.contains("walt random [--same|DISPLAY_INDEX]"));
        assert!(usage.contains("walt random --same"));
        assert!(usage.contains("walt random 0"));
        assert!(usage.contains("walt uninstall [--yes]"));
        assert!(usage.contains("rotation interval <secs>"));
        assert!(!usage.contains("--random"));
        assert!(!usage.contains("--install-service"));
        assert!(!usage.contains("--service-status"));
        assert!(!usage.contains("--rotate-daemon"));
    }

    #[test]
    fn accepts_uninstall_confirmation() {
        let mut input = Cursor::new("yes\n");
        let mut output = Vec::new();

        let confirmed = confirm_uninstall_with_io(
            false,
            &uninstall_paths(),
            true,
            true,
            &mut input,
            &mut output,
        )
        .expect("confirmation");

        assert!(confirmed);
        let text = String::from_utf8(output).expect("utf8");
        assert!(text.contains("This will remove Walt from this system:"));
        assert!(text.contains("Continue? [y/N]: "));
    }

    #[test]
    fn declines_uninstall_confirmation() {
        let mut input = Cursor::new("n\n");
        let mut output = Vec::new();

        let confirmed = confirm_uninstall_with_io(
            false,
            &uninstall_paths(),
            true,
            true,
            &mut input,
            &mut output,
        )
        .expect("confirmation");

        assert!(!confirmed);
    }

    #[test]
    fn rejects_non_interactive_uninstall_without_yes() {
        let mut input = Cursor::new(Vec::<u8>::new());
        let mut output = Vec::new();

        let error = confirm_uninstall_with_io(
            false,
            &uninstall_paths(),
            false,
            false,
            &mut input,
            &mut output,
        )
        .expect_err("non-interactive uninstall should fail");

        assert_eq!(
            error.to_string(),
            "walt uninstall requires confirmation in an interactive terminal. Re-run with `walt uninstall --yes`."
        );
    }
}
