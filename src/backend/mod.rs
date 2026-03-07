pub mod hyprpaper;
pub mod rotation;
pub mod scanner;

pub use hyprpaper::set_wallpaper;
pub use rotation::{
    disable_rotation_service, enable_rotation_service, get_rotation_service_status,
    install_rotation_service, rotation_service_badge, rotation_service_status, run_rotation_daemon,
    uninstall_rotation_service, RotationServiceStatus,
};
pub use scanner::{scan_directory, scan_wallpapers_from_paths};
