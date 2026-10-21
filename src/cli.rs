use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::types::{ColorMode, MouseMode, ProgressMode};

#[derive(Debug, Parser)]
#[command(
    name = "urwtest-rs",
    version,
    about = "OpenURWTest - mounted-volume read/write and capacity test"
)]
pub struct Cli {
    /// Control ANSI colors.
    #[arg(long, global = true, value_enum, default_value_t = ColorMode::Auto)]
    pub color: ColorMode,

    /// Progress output mode.
    #[arg(long, global = true, value_enum, default_value_t = ProgressMode::Auto)]
    pub progress: ProgressMode,

    /// Disable progress output (same as --progress off).
    #[arg(long, global = true, hide = true)]
    pub no_progress: bool,

    /// Export CSV/JSON/XLSX/PNG reports to this directory after the run.
    #[arg(long, global = true, value_name = "DIR")]
    pub export: Option<PathBuf>,

    /// Force the plain text interface even when a terminal is available.
    #[arg(long, global = true)]
    pub no_tui: bool,

    /// Mouse support in the TUI.
    #[arg(long, global = true, value_enum, default_value_t = MouseMode::Auto)]
    pub mouse: MouseMode,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// List mounted volumes that can be tested.
    List,

    /// Write test data to a mounted volume and verify it.
    Run(RunArgs),

    /// Verify test data that already exists on a mounted volume.
    Verify(VerifyArgs),

    /// Run a speed test on a volume or directory.
    Speed(SpeedArgs),

    /// Show volume information and temperature/health.
    Info(InfoArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Volume root to test, for example E:\ or /mnt/usb.
    #[arg(short, long)]
    pub target: PathBuf,

    /// Number of write/verify passes.
    #[arg(short, long, default_value_t = 1)]
    pub passes: u32,

    /// Write test data only; skip verification.
    #[arg(long)]
    pub no_verify: bool,

    /// Keep going to the next pass when verification reports errors.
    #[arg(long)]
    pub continue_on_error: bool,

    /// Allow a quality run against a volume that contains a Windows directory.
    #[arg(long)]
    pub allow_system: bool,
}

#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Volume root that contains test data.
    #[arg(short, long)]
    pub target: PathBuf,
}

#[derive(Debug, Args)]
pub struct SpeedArgs {
    /// Target volume root or directory; repeat for multiple drives.
    #[arg(short, long, required = true)]
    pub target: Vec<PathBuf>,

    /// CDM preset: seq1m-q8t1, seq1m-q1t1, rnd4k-q32t1, rnd4k-q1t1, or all.
    /// Repeat or use commas; defaults to all four.
    #[arg(long, value_delimiter = ',')]
    pub preset: Vec<String>,

    /// Test file size, for example 4g, 2g, 1g.
    #[arg(long, default_value = "4g")]
    pub size: String,

    /// Continue sequential tests after the first verification or read error.
    #[arg(long)]
    pub ignore_errors: bool,

    /// Random seed.
    #[arg(long)]
    pub seed: Option<u64>,

    /// Export PNG speed charts to this directory (default: `CWD/urwtest-export/<timestamp>`).
    #[arg(long)]
    pub export: Option<PathBuf>,

    /// Include temperature in speed charts.
    #[arg(long)]
    pub export_temperature: bool,

    /// Print a JSON result.
    #[arg(long)]
    pub json: bool,
}
#[derive(Debug, Args)]
pub struct InfoArgs {
    /// Target volume root, for example C:\.
    #[arg(short, long)]
    pub target: PathBuf,

    /// Print a JSON result.
    #[arg(long)]
    pub json: bool,
}
