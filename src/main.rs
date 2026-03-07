mod backend;
mod cache;
mod config;
mod ui;

use std::env;

use anyhow::{bail, Context, Result};
use rand::seq::SliceRandom;

const ROTATION_OPTIONS: &str = "install, enable, disable, uninstall, status, interval";
const ROTATION_USAGE: &str = "walt rotation <install|enable|disable|uninstall|status|interval>";

#[derive(Debug, Eq, PartialEq)]
enum CliCommand {
    LaunchUi,
    Help,
    Random,
    RotateDaemon,
    Rotation(RotationCommand),
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
        CliCommand::Help => {
            print_usage();
            return Ok(());
        }
        CliCommand::Random => return print_random_wallpaper(),
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
        ["random"] => Ok(CliCommand::Random),
        ["random", ..] => bail!("Usage: walt random"),
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

fn print_random_wallpaper() -> Result<()> {
    let config = config::Config::new();

    if config.is_empty() {
        bail!("No wallpaper directories configured. Launch walt once to add paths.")
    }

    let wallpapers = backend::scan_wallpapers_from_paths(&config.wallpaper_paths);
    let wallpaper = wallpapers
        .choose(&mut rand::thread_rng())
        .ok_or_else(|| anyhow::anyhow!("No wallpapers found in configured directories."))?;

    let wallpaper_path = wallpaper.path.to_string_lossy();
    backend::set_wallpaper(&wallpaper_path)?;
    Ok(())
}

fn print_usage() {
    println!("{}", usage_text());
}

fn usage_text() -> String {
    [
        "Walt",
        "",
        "Usage:",
        "  walt random",
        "  walt rotation <command>",
        "",
        "Commands:",
        "  random                    Apply a random wallpaper from the configured list",
        "  rotation install          Install and start the persistent rotation service",
        "  rotation enable           Enable and start the installed rotation service",
        "  rotation disable          Disable and stop the installed rotation service",
        "  rotation uninstall        Stop and remove the persistent rotation service",
        "  rotation status           Show formatted rotation service status, including activity",
        "  rotation interval <secs>  Set the rotation interval in seconds",
        "",
        "Examples:",
        "  walt random",
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
    use super::{parse_command, usage_text, CliCommand, RotationCommand};

    #[test]
    fn parses_random_command() {
        assert_eq!(
            parse_command(&["random"]).expect("command"),
            CliCommand::Random
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
        assert_eq!(error.to_string(), "Usage: walt random");
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
        assert!(usage.contains("rotation interval <secs>"));
        assert!(!usage.contains("--random"));
        assert!(!usage.contains("--install-service"));
        assert!(!usage.contains("--service-status"));
        assert!(!usage.contains("--rotate-daemon"));
    }
}
