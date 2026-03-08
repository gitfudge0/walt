use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::rotation::{rotation_service_file_path, uninstall_rotation_service};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UninstallPaths {
    pub service_file: PathBuf,
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub binary_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CleanupStatus {
    Removed,
    SkippedMissing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UninstallReport {
    pub paths: UninstallPaths,
    pub service: CleanupStatus,
    pub config: CleanupStatus,
    pub cache: CleanupStatus,
    pub binary: CleanupStatus,
}

impl UninstallReport {
    pub fn summary(&self) -> String {
        [
            "Walt uninstall complete.".to_string(),
            format!(
                "Rotation service: {} ({})",
                self.service.label(),
                self.paths.service_file.display()
            ),
            format!(
                "Config: {} ({})",
                self.config.label(),
                self.paths.config_dir.display()
            ),
            format!(
                "Cache: {} ({})",
                self.cache.label(),
                self.paths.cache_dir.display()
            ),
            format!(
                "Binary: {} ({})",
                self.binary.label(),
                self.paths.binary_path.display()
            ),
        ]
        .join("\n")
    }
}

impl CleanupStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::SkippedMissing => "not found",
        }
    }
}

pub fn uninstall_paths() -> Result<UninstallPaths> {
    Ok(UninstallPaths {
        service_file: rotation_service_file_path()?,
        config_dir: dirs::config_dir()
            .context("Could not find config directory")?
            .join("walt"),
        cache_dir: dirs::cache_dir()
            .context("Could not find cache directory")?
            .join("walt"),
        binary_path: dirs::home_dir()
            .context("Could not find home directory")?
            .join(".local")
            .join("bin")
            .join("walt"),
    })
}

pub fn uninstall_walt() -> Result<UninstallReport> {
    let paths = uninstall_paths()?;
    uninstall_walt_with_paths(paths, remove_rotation_service)
}

fn remove_rotation_service(path: &Path) -> Result<CleanupStatus> {
    if !path.exists() {
        return Ok(CleanupStatus::SkippedMissing);
    }

    uninstall_rotation_service()?;
    Ok(CleanupStatus::Removed)
}

fn remove_dir_if_exists(path: &Path) -> Result<CleanupStatus> {
    if !path.exists() {
        return Ok(CleanupStatus::SkippedMissing);
    }

    fs::remove_dir_all(path)?;
    Ok(CleanupStatus::Removed)
}

fn remove_file_if_exists(path: &Path) -> Result<CleanupStatus> {
    if !path.exists() {
        return Ok(CleanupStatus::SkippedMissing);
    }

    fs::remove_file(path)?;
    Ok(CleanupStatus::Removed)
}

fn uninstall_walt_with_paths<F>(paths: UninstallPaths, remove_service: F) -> Result<UninstallReport>
where
    F: FnOnce(&Path) -> Result<CleanupStatus>,
{
    let service = remove_service(&paths.service_file)?;
    let config = remove_dir_if_exists(&paths.config_dir)?;
    let cache = remove_dir_if_exists(&paths.cache_dir)?;
    let binary = remove_file_if_exists(&paths.binary_path)?;

    Ok(UninstallReport {
        paths,
        service,
        config,
        cache,
        binary,
    })
}

#[cfg(test)]
mod tests {
    use super::{uninstall_walt_with_paths, CleanupStatus, UninstallPaths};
    use std::fs;
    use std::path::Path;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_temp_dir(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("walt-uninstall-test-{name}-{unique}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn sample_paths(root: &Path) -> UninstallPaths {
        UninstallPaths {
            service_file: root
                .join("systemd")
                .join("user")
                .join("walt-rotation.service"),
            config_dir: root.join("config").join("walt"),
            cache_dir: root.join("cache").join("walt"),
            binary_path: root.join(".local").join("bin").join("walt"),
        }
    }

    #[test]
    fn removes_existing_targets() {
        let root = make_temp_dir("existing");
        let paths = sample_paths(&root);
        let unrelated_binary = root.join("target").join("debug").join("walt");

        fs::create_dir_all(paths.config_dir.join("nested")).expect("config dir");
        fs::create_dir_all(paths.cache_dir.join("thumbs")).expect("cache dir");
        fs::create_dir_all(paths.binary_path.parent().expect("binary parent")).expect("bin dir");
        fs::create_dir_all(unrelated_binary.parent().expect("unrelated parent"))
            .expect("unrelated dir");
        fs::write(&paths.config_dir.join("state.json"), b"{}").expect("config file");
        fs::write(&paths.cache_dir.join("wallpapers.json"), b"[]").expect("cache file");
        fs::write(&paths.binary_path, b"binary").expect("binary");
        fs::write(&unrelated_binary, b"other-binary").expect("other binary");
        fs::create_dir_all(paths.service_file.parent().expect("service parent"))
            .expect("service dir");
        fs::write(&paths.service_file, b"[Unit]").expect("service file");

        let service_called = Arc::new(AtomicBool::new(false));
        let service_called_for_closure = Arc::clone(&service_called);
        let report = uninstall_walt_with_paths(paths.clone(), move |service_path| {
            service_called_for_closure.store(true, Ordering::SeqCst);
            fs::remove_file(service_path).expect("remove service file");
            Ok(CleanupStatus::Removed)
        })
        .expect("uninstall");

        assert!(service_called.load(Ordering::SeqCst));
        assert_eq!(report.service, CleanupStatus::Removed);
        assert_eq!(report.config, CleanupStatus::Removed);
        assert_eq!(report.cache, CleanupStatus::Removed);
        assert_eq!(report.binary, CleanupStatus::Removed);
        assert!(!paths.service_file.exists());
        assert!(!paths.config_dir.exists());
        assert!(!paths.cache_dir.exists());
        assert!(!paths.binary_path.exists());
        assert!(unrelated_binary.exists());

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn succeeds_when_targets_are_missing() {
        let root = make_temp_dir("missing");
        let paths = sample_paths(&root);

        let report = uninstall_walt_with_paths(paths, |_| Ok(CleanupStatus::SkippedMissing))
            .expect("uninstall");

        assert_eq!(report.service, CleanupStatus::SkippedMissing);
        assert_eq!(report.config, CleanupStatus::SkippedMissing);
        assert_eq!(report.cache, CleanupStatus::SkippedMissing);
        assert_eq!(report.binary, CleanupStatus::SkippedMissing);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn invokes_service_cleanup_when_service_exists() {
        let root = make_temp_dir("service");
        let paths = sample_paths(&root);
        fs::create_dir_all(paths.service_file.parent().expect("service parent"))
            .expect("service dir");
        fs::write(&paths.service_file, b"[Unit]").expect("service file");

        let service_called = Arc::new(AtomicBool::new(false));
        let service_called_for_closure = Arc::clone(&service_called);
        let report = uninstall_walt_with_paths(paths.clone(), move |service_path| {
            service_called_for_closure.store(true, Ordering::SeqCst);
            fs::remove_file(service_path).expect("remove service file");
            Ok(CleanupStatus::Removed)
        })
        .expect("uninstall");

        assert!(service_called.load(Ordering::SeqCst));
        assert_eq!(report.service, CleanupStatus::Removed);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }
}
