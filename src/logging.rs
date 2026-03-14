use std::{
    collections::VecDeque,
    env, fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};

use chrono::Local;
use log::{Level, LevelFilter, Log, Metadata, Record};

const LOG_DIR: &str = "walt/logs";
const LOG_FILE: &str = "walt.log";
const MAX_LOG_LINES: usize = 500;

static LOGGER: WaltLogger = WaltLogger;
static LOGGER_RUNTIME: OnceLock<LoggerRuntime> = OnceLock::new();
static LOGGER_INIT: OnceLock<()> = OnceLock::new();
static STDERR_MIRRORING_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn init_logging() {
    LOGGER_INIT.get_or_init(|| {
        let runtime = match LoggerRuntime::new() {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("Failed to initialize Walt logging: {error}");
                return;
            }
        };

        let level = runtime.level;
        let _ = LOGGER_RUNTIME.set(runtime);

        if let Err(error) = log::set_logger(&LOGGER) {
            eprintln!("Failed to install Walt logger: {error}");
            return;
        }

        log::set_max_level(level);
    });
}

pub fn set_stderr_mirroring_enabled(enabled: bool) {
    STDERR_MIRRORING_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn log_file_path() -> anyhow::Result<PathBuf> {
    Ok(log_dir_path()?.join(LOG_FILE))
}

struct LoggerRuntime {
    level: LevelFilter,
    path: PathBuf,
    state: Mutex<LoggerState>,
}

impl LoggerRuntime {
    fn new() -> anyhow::Result<Self> {
        let path = log_file_path()?;
        let state = LoggerState::load(&path)?;
        Ok(Self {
            level: parse_log_level(env::var("WALT_LOG").ok().as_deref()),
            path,
            state: Mutex::new(state),
        })
    }
}

#[derive(Debug)]
struct LoggerState {
    lines: VecDeque<String>,
}

impl LoggerState {
    fn load(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = fs::read_to_string(path).unwrap_or_default();
        let mut lines = content
            .lines()
            .map(ToOwned::to_owned)
            .collect::<VecDeque<_>>();
        trim_retained_lines(&mut lines);
        persist_lines(path, &lines)?;
        Ok(Self { lines })
    }

    fn push(&mut self, path: &Path, line: String) -> anyhow::Result<()> {
        self.lines.push_back(line);
        trim_retained_lines(&mut self.lines);
        persist_lines(path, &self.lines)
    }
}

struct WaltLogger;

impl Log for WaltLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        LOGGER_RUNTIME
            .get()
            .map(|runtime| metadata.level() <= runtime.level)
            .unwrap_or(false)
    }

    fn log(&self, record: &Record<'_>) {
        let Some(runtime) = LOGGER_RUNTIME.get() else {
            return;
        };

        if !self.enabled(record.metadata()) {
            return;
        }

        let line = format_log_line(record);

        if should_mirror_to_stderr(record.level()) {
            eprintln!("{line}");
        }

        if let Ok(mut state) = runtime.state.lock() {
            let _ = state.push(&runtime.path, line);
        }
    }

    fn flush(&self) {}
}

fn should_mirror_to_stderr(level: Level) -> bool {
    STDERR_MIRRORING_ENABLED.load(Ordering::Relaxed) && matches!(level, Level::Warn | Level::Error)
}

fn format_log_line(record: &Record<'_>) -> String {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let message = record.args().to_string().replace('\n', "\\n");
    format!(
        "{timestamp} {:<5} {} {message}",
        record.level(),
        record.target()
    )
}

fn trim_retained_lines(lines: &mut VecDeque<String>) {
    while lines.len() > MAX_LOG_LINES {
        lines.pop_front();
    }
}

fn persist_lines(path: &Path, lines: &VecDeque<String>) -> anyhow::Result<()> {
    let temp_path = temp_log_path(path);
    let payload = if lines.is_empty() {
        String::new()
    } else {
        let mut content = lines.iter().cloned().collect::<Vec<_>>().join("\n");
        content.push('\n');
        content
    };

    fs::write(&temp_path, payload)?;
    fs::rename(&temp_path, path)?;
    Ok(())
}

fn temp_log_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| format!("{}.tmp", name.to_string_lossy()))
        .unwrap_or_else(|| format!("{LOG_FILE}.tmp"));
    path.with_file_name(file_name)
}

fn log_dir_path() -> anyhow::Result<PathBuf> {
    Ok(dirs::cache_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not find cache directory"))?
        .join(LOG_DIR))
}

fn parse_log_level(value: Option<&str>) -> LevelFilter {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("error") => LevelFilter::Error,
        Some("warn") => LevelFilter::Warn,
        Some("info") => LevelFilter::Info,
        Some("debug") => LevelFilter::Debug,
        Some("trace") => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_log_line, parse_log_level, persist_lines, temp_log_path, trim_retained_lines,
        LoggerState, MAX_LOG_LINES,
    };
    use log::{Level, Record};
    use std::{
        collections::VecDeque,
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("walt-logging-test-{unique}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn defaults_to_info_log_level() {
        assert_eq!(parse_log_level(None), log::LevelFilter::Info);
        assert_eq!(parse_log_level(Some("bogus")), log::LevelFilter::Info);
    }

    #[test]
    fn parses_supported_log_levels() {
        assert_eq!(parse_log_level(Some("error")), log::LevelFilter::Error);
        assert_eq!(parse_log_level(Some("warn")), log::LevelFilter::Warn);
        assert_eq!(parse_log_level(Some("info")), log::LevelFilter::Info);
        assert_eq!(parse_log_level(Some("debug")), log::LevelFilter::Debug);
        assert_eq!(parse_log_level(Some("trace")), log::LevelFilter::Trace);
    }

    #[test]
    fn trims_log_lines_to_retention_limit() {
        let mut lines = (0..(MAX_LOG_LINES + 5))
            .map(|index| format!("line-{index}"))
            .collect::<VecDeque<_>>();

        trim_retained_lines(&mut lines);

        assert_eq!(lines.len(), MAX_LOG_LINES);
        assert_eq!(lines.front(), Some(&"line-5".to_string()));
        assert_eq!(lines.back(), Some(&format!("line-{}", MAX_LOG_LINES + 4)));
    }

    #[test]
    fn persists_only_newest_lines() {
        let dir = temp_dir();
        let path = dir.join("walt.log");
        let lines = (0..(MAX_LOG_LINES + 2))
            .map(|index| format!("line-{index}"))
            .collect::<VecDeque<_>>();

        let mut retained = lines.clone();
        trim_retained_lines(&mut retained);
        persist_lines(&path, &retained).expect("persist log lines");

        let written = fs::read_to_string(&path).expect("read log file");
        assert!(!written.contains("line-0"));
        assert!(written.contains(&format!("line-{}", MAX_LOG_LINES + 1)));
        fs::remove_dir_all(dir).expect("cleanup temp dir");
    }

    #[test]
    fn flattens_multiline_messages() {
        let record = Record::builder()
            .args(format_args!("hello\nworld"))
            .level(Level::Info)
            .target("walt::tests")
            .build();

        let line = format_log_line(&record);
        assert!(line.contains("hello\\nworld"));
    }

    #[test]
    fn logger_state_load_is_idempotent() {
        let dir = temp_dir();
        let path = dir.join("walt.log");
        fs::write(&path, "alpha\nbeta\n").expect("seed log");

        let state = LoggerState::load(&path).expect("load logger state");
        let content_after_first_load = fs::read_to_string(&path).expect("read first");
        let _ = LoggerState::load(&path).expect("load logger state again");
        let content_after_second_load = fs::read_to_string(&path).expect("read second");

        assert_eq!(state.lines.len(), 2);
        assert_eq!(content_after_first_load, content_after_second_load);
        fs::remove_dir_all(dir).expect("cleanup temp dir");
    }

    #[test]
    fn temp_log_rewrite_leaves_final_file_intact() {
        let dir = temp_dir();
        let path = dir.join("walt.log");
        let lines = VecDeque::from(["alpha".to_string(), "beta".to_string()]);

        persist_lines(&path, &lines).expect("persist");

        assert!(path.exists());
        assert!(!temp_log_path(&path).exists());
        assert_eq!(
            fs::read_to_string(&path).expect("read log"),
            "alpha\nbeta\n"
        );
        fs::remove_dir_all(dir).expect("cleanup temp dir");
    }
}
