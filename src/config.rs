use clap::{Parser, ValueEnum};
use serde::Deserialize;

const DEFAULT_INTERVAL_SECS: u64 = 1;
const DEFAULT_CONFIG_FILE: &str = "resource-tracker-rs.toml";
const DEFAULT_UPLOAD_TIMEOUT_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// Output format
// ---------------------------------------------------------------------------

/// Output format emitted to stdout on each polling interval.
#[derive(Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum OutputFormat {
    /// JSON Lines - one JSON object per line (default).
    Json,
    /// CSV - header on first line, one row per interval.
    /// Columns mirror Python resource-tracker's SystemTracker output.
    Csv,
}

// ---------------------------------------------------------------------------
// TOML file structure
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct TomlConfig {
    job: Option<TomlJob>,
    tracker: Option<TomlTracker>,
}

#[derive(Debug, Deserialize)]
struct TomlJob {
    /// Human-readable label attached to every sample (e.g. "benchmark-run-42").
    name: Option<String>,
    /// Root PID of the process tree whose CPU usage should be attributed.
    pid: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct TomlTracker {
    /// How often to emit a sample, in seconds. Default: 5.
    interval_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// CLI arguments (clap derive)
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(
    name = "resource-tracker-rs",
    about = "Lightweight Linux resource & GPU tracker",
    version
)]
struct Cli {
    /// Job name / metadata label attached to every sample.
    #[arg(short = 'n', long, value_name = "NAME")]
    job_name: Option<String>,

    /// Root PID of the process tree to track CPU usage for.
    #[arg(short = 'p', long, value_name = "PID")]
    pid: Option<i32>,

    /// Polling interval in seconds [default: 5].
    #[arg(short = 'i', long, value_name = "SECS")]
    interval: Option<u64>,

    /// Path to TOML config file [default: sparecores.toml].
    #[arg(short = 'c', long, value_name = "FILE", default_value = DEFAULT_CONFIG_FILE)]
    config: String,

    /// Output format: json (default) or csv.
    #[arg(short = 'f', long, value_name = "FORMAT", default_value = "json")]
    format: OutputFormat,
}

// ---------------------------------------------------------------------------
// Merged config - the single source of truth for the rest of the program
// ---------------------------------------------------------------------------

/// Resolved configuration after merging CLI args > config file > defaults.
#[derive(Debug, Clone)]
pub struct Config {
    /// Optional job label included in every emitted sample.
    pub job_name: Option<String>,
    /// Root PID for per-process CPU attribution. None = system-wide only.
    pub pid: Option<i32>,
    /// Polling interval in seconds.
    pub interval_secs: u64,
    /// Output format emitted to stdout.
    pub format: OutputFormat,
}

impl Config {
    /// Parse CLI args, optionally load the TOML config file, and merge with
    /// defaults.  CLI flags always win; config file wins over defaults.
    pub fn load() -> Self {
        let cli = Cli::parse();

        // Silently skip missing or unparseable config files - the tool should
        // work with zero configuration.
        let toml: TomlConfig = std::fs::read_to_string(&cli.config)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();

        Config {
            job_name: cli
                .job_name
                .or_else(|| toml.job.as_ref().and_then(|j| j.name.clone())),

            pid: cli.pid.or_else(|| toml.job.as_ref().and_then(|j| j.pid)),

            interval_secs: cli
                .interval
                .or_else(|| toml.tracker.as_ref().and_then(|t| t.interval_secs))
                .unwrap_or(DEFAULT_INTERVAL_SECS),

            format: cli.format,
        }
    }
}
