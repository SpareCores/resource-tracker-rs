use clap::{ArgAction, Parser, ValueEnum};
use serde::Deserialize;

const DEFAULT_INTERVAL_SECS: u64 = 1;
const DEFAULT_CONFIG_FILE: &str = "resource-tracker-rs.toml";

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
    /// How often to emit a sample, in seconds. Default: 1.
    interval_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// Job metadata (Section 9.3) - sent to Sentinel API at run registration
// ---------------------------------------------------------------------------

/// All optional metadata fields from Section 9.3 of the spec.
/// Accepted via CLI flags and TRACKER_* environment variables.
/// Used when registering a run with the Sentinel API (Priority 4).
#[derive(Debug, Clone, Default)]
pub struct JobMetadata {
    pub job_name:        Option<String>,
    pub project_name:    Option<String>,
    pub stage_name:      Option<String>,
    pub task_name:       Option<String>,
    pub team:            Option<String>,
    pub env:             Option<String>,
    pub language:        Option<String>,
    pub orchestrator:    Option<String>,
    pub executor:        Option<String>,
    pub external_run_id: Option<String>,
    pub container_image: Option<String>,
    /// Arbitrary key=value tags supplied via repeated --tag flags.
    pub tags:            Vec<String>,
}

// ---------------------------------------------------------------------------
// CLI arguments (clap derive)
// ---------------------------------------------------------------------------

#[derive(Debug, Parser)]
#[command(
    name = "resource-tracker-rs",
    about = "Lightweight Linux resource & GPU tracker.\n\n\
             Shell-wrapper mode: resource-tracker-rs [FLAGS] -- <command> [args...]\n\
             The tracker will spawn <command>, monitor it, and exit when it exits.",
    version
)]
struct Cli {
    // -- Core flags ----------------------------------------------------------

    /// Job name attached to every sample and to the Sentinel run record.
    #[arg(short = 'n', long, value_name = "NAME", env = "TRACKER_JOB_NAME")]
    job_name: Option<String>,

    /// Root PID of the process tree to track CPU usage for.
    /// Overridden automatically in shell-wrapper mode.
    #[arg(short = 'p', long, value_name = "PID")]
    pid: Option<i32>,

    /// Polling interval in seconds (must be >= 1).
    #[arg(short = 'i', long, value_name = "SECS")]
    interval: Option<u64>,

    /// Path to TOML config file.
    #[arg(short = 'c', long, value_name = "FILE", default_value = DEFAULT_CONFIG_FILE)]
    config: String,

    /// Output format: json (default) or csv.
    #[arg(short = 'f', long, value_name = "FORMAT", default_value = "json")]
    format: OutputFormat,

    /// Write metric output to FILE instead of stdout.
    /// Useful in shell-wrapper mode to keep the tracked app's stdout clean.
    #[arg(short = 'o', long, value_name = "FILE", env = "TRACKER_OUTPUT")]
    output: Option<String>,

    /// Suppress metric output entirely (no stdout, no file).
    /// Useful when streaming to Sentinel and local output is not needed.
    #[arg(long, env = "TRACKER_QUIET")]
    quiet: bool,

    // -- Section 9.3 metadata flags ------------------------------------------

    /// Project name for Sentinel run registration.
    #[arg(long, value_name = "NAME", env = "TRACKER_PROJECT_NAME")]
    project_name: Option<String>,

    /// Stage name (e.g. "train", "eval") for Sentinel run registration.
    #[arg(long, value_name = "NAME", env = "TRACKER_STAGE_NAME")]
    stage_name: Option<String>,

    /// Task name for Sentinel run registration.
    #[arg(long, value_name = "NAME", env = "TRACKER_TASK_NAME")]
    task_name: Option<String>,

    /// Team name for Sentinel run registration.
    #[arg(long, value_name = "NAME", env = "TRACKER_TEAM")]
    team: Option<String>,

    /// Environment label (e.g. "prod", "staging") for Sentinel run registration.
    #[arg(long, value_name = "ENV", env = "TRACKER_ENV")]
    env: Option<String>,

    /// Programming language label for Sentinel run registration.
    #[arg(long, value_name = "LANG", env = "TRACKER_LANGUAGE")]
    language: Option<String>,

    /// Orchestrator label (e.g. "airflow", "prefect") for Sentinel run registration.
    #[arg(long, value_name = "NAME", env = "TRACKER_ORCHESTRATOR")]
    orchestrator: Option<String>,

    /// Executor label (e.g. "kubernetes", "slurm") for Sentinel run registration.
    #[arg(long, value_name = "NAME", env = "TRACKER_EXECUTOR")]
    executor: Option<String>,

    /// External run ID from the calling system for Sentinel run registration.
    #[arg(long, value_name = "ID", env = "TRACKER_EXTERNAL_RUN_ID")]
    external_run_id: Option<String>,

    /// Container image name/tag for Sentinel run registration.
    #[arg(long, value_name = "IMAGE", env = "TRACKER_CONTAINER_IMAGE")]
    container_image: Option<String>,

    /// Arbitrary key=value tag. May be repeated: --tag key1=val1 --tag key2=val2
    #[arg(long = "tag", value_name = "KEY=VALUE", action = ArgAction::Append)]
    tags: Vec<String>,

    // -- Shell-wrapper mode --------------------------------------------------

    /// Command to spawn and monitor. All tokens after -- are the command + args.
    /// Example: resource-tracker-rs -- Rscript model.R --epochs 10
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "COMMAND")]
    command: Vec<String>,
}

// ---------------------------------------------------------------------------
// Merged config
// ---------------------------------------------------------------------------

/// Resolved configuration after merging CLI args > TOML file > defaults.
#[derive(Debug, Clone)]
pub struct Config {
    /// Root PID for per-process CPU attribution. None = system-wide only.
    /// Set automatically from the spawned child PID in shell-wrapper mode.
    pub pid: Option<i32>,
    /// Polling interval in seconds.
    pub interval_secs: u64,
    /// Output format (JSON or CSV).
    pub format: OutputFormat,
    /// Write metric output to this file path instead of stdout.
    /// None = write to stdout.
    pub output_file: Option<String>,
    /// Suppress all metric output (no stdout, no file).
    pub quiet: bool,
    /// Section 9.3 job metadata (used for Sentinel API registration).
    pub metadata: JobMetadata,
    /// Shell-wrapper command. Empty = standalone mode.
    pub command: Vec<String>,
}

impl Config {
    /// Parse CLI args, optionally load the TOML config file, and merge with
    /// defaults.  CLI flags always win; config file wins over defaults.
    pub fn load() -> Self {
        let cli = Cli::parse();

        // Silently skip missing or unparseable config files.
        let toml: TomlConfig = std::fs::read_to_string(&cli.config)
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();

        let interval_secs = cli
            .interval
            .or_else(|| toml.tracker.as_ref().and_then(|t| t.interval_secs))
            .unwrap_or(DEFAULT_INTERVAL_SECS);

        if interval_secs == 0 {
            eprintln!("error: --interval must be >= 1 (got 0)");
            std::process::exit(1);
        }

        let pid = cli.pid.or_else(|| toml.job.as_ref().and_then(|j| j.pid));

        let metadata = JobMetadata {
            job_name: cli.job_name
                .or_else(|| toml.job.as_ref().and_then(|j| j.name.clone())),
            project_name:    cli.project_name,
            stage_name:      cli.stage_name,
            task_name:       cli.task_name,
            team:            cli.team,
            env:             cli.env,
            language:        cli.language,
            orchestrator:    cli.orchestrator,
            executor:        cli.executor,
            external_run_id: cli.external_run_id,
            container_image: cli.container_image,
            tags:            cli.tags,
        };

        Config {
            pid,
            interval_secs,
            format:      cli.format,
            output_file: cli.output,
            quiet:       cli.quiet,
            metadata,
            command:     cli.command,
        }
    }
}
