pub mod hyprpaper;
pub mod rotation;
pub mod scanner;
pub mod uninstall;

pub use hyprpaper::{
    get_active_wallpapers, get_monitors, set_wallpaper, set_wallpaper_for_monitor, Monitor,
};
pub use rotation::{
    disable_rotation_service, enable_rotation_service, get_rotation_service_status,
    install_rotation_service, restart_rotation_service_if_active, rotation_service_badge,
    rotation_service_status, run_rotation_daemon, uninstall_rotation_service,
    RotationServiceStatus,
};
pub use scanner::{scan_directory, scan_wallpapers_from_paths};
pub use uninstall::{uninstall_paths, uninstall_walt, UninstallPaths};
