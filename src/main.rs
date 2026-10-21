mod analysis;
mod cancel;
mod cli;
mod console;
mod engine;
mod export;
mod format;
mod health;
mod interactive;
mod io;
mod reporter;
#[allow(dead_code)]
mod speed;
mod terminal;
mod theme;
mod trace;
mod tui;
mod types;
mod volume;

use std::io::IsTerminal;
use std::process;

use anyhow::{Result, anyhow};
use clap::Parser;

use crate::analysis::{AnalysisParams, analyze};
use crate::cli::{Cli, Commands};
use crate::console::{Console, format_bytes};
use crate::engine::{RunOptions, Summary};
use crate::export::{ExportInput, export as export_run};
use crate::reporter::LineReporter;
use crate::types::ProgressMode;

fn main() {
    if let Err(error) = real_main() {
        eprintln!("Error: {error:#}");
        process::exit(2);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let console = Console::new(cli.color);
    let progress = if cli.no_progress {
        ProgressMode::Off
    } else {
        match cli.progress {
            ProgressMode::Auto => {
                if console.progress_enabled() {
                    ProgressMode::Live
                } else {
                    ProgressMode::Off
                }
            }
            other => other,
        }
    };

    cancel::install()?;
    cancel::reset();

    match &cli.command {
        None => {
            if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
                tui::run_home(cli.color, cli.mouse)?;
            } else {
                interactive::run(&console, progress)?;
            }
        }
        Some(Commands::List) => {
            let volumes = volume::list_volumes();
            if volumes.is_empty() {
                console.warn("No testable mounted volumes found.");
            } else {
                console.print_volumes(&volumes);
            }
        }
        Some(Commands::Run(args)) => {
            let target = volume::normalize_target_path(&args.target);
            resolve_target(&target)?;
            let options = RunOptions {
                target: &target,
                passes: args.passes.max(1),
                verify: !args.no_verify,
                continue_on_error: args.continue_on_error,
                cancel: None,
            };
            let existing = format::scan_test_files(&target)?;
            if args.no_verify && !existing.is_empty() {
                console.info("Test data already exists on this volume; nothing to do.");
                return Ok(());
            }
            if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() && !cli.no_tui {
                let mode = if args.no_verify {
                    tui::RunMode::WriteOnly
                } else {
                    tui::RunMode::WriteVerify
                };
                tui::run_target(
                    &target,
                    mode,
                    args.passes.max(1),
                    args.continue_on_error,
                    cli.color,
                    cli.mouse,
                )?;
                return Ok(());
            }
            let mut reporter = LineReporter::new(&console, progress);
            let summary = engine::run_os(&options, &mut reporter)?;
            export_if_requested(&cli, &target, &summary, &console)?;
        }
        Some(Commands::Info(args)) => {
            let target = volume::normalize_target_path(&args.target);
            let volume = volume::find_volume(&target).ok_or_else(|| {
                anyhow!("target is not a mounted volume root: {}", target.display())
            })?;
            let health = health::read_health(&volume.mount_point);
            if args.json {
                println!(
                    "{{\"volume\":\"{}\",\"temperature_c\":{},\"health\":\"{}\",\"disk_number\":{}}}",
                    volume.mount_point.display(),
                    health
                        .temperature_c
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".to_string()),
                    health.status.label(),
                    health
                        .disk_number
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".to_string())
                );
            } else {
                println!("Volume:      {}", volume.display_name());
                println!("Model:       {}", volume.device_name());
                println!("Filesystem:  {}", volume.fs);
                println!("Capacity:    {}", format_bytes(volume.total_bytes));
                println!("Free:        {}", format_bytes(volume.free_bytes));
                println!(
                    "Temperature: {}",
                    health
                        .temperature_c
                        .map(|value| format!("{value} C"))
                        .unwrap_or_else(|| "unknown".to_string())
                );
                println!("Health:      {}", health.status.label());
                println!(
                    "Disk number: {}",
                    health
                        .disk_number
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                );
            }
        }
        Some(Commands::Speed(args)) => {
            speed::cli::run(args)?;
        }
        Some(Commands::Verify(args)) => {
            let target = volume::normalize_target_path(&args.target);
            resolve_target(&target)?;
            if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() && !cli.no_tui {
                tui::run_target(
                    &target,
                    tui::RunMode::VerifyExisting,
                    1,
                    false,
                    cli.color,
                    cli.mouse,
                )?;
                return Ok(());
            }
            let mut reporter = LineReporter::new(&console, progress);
            let summary = engine::verify_existing(&target, &mut reporter)?;
            export_if_requested(&cli, &target, &summary, &console)?;
        }
    }

    Ok(())
}

fn resolve_target(target: &std::path::Path) -> Result<()> {
    volume::find_volume(target)
        .map(|_| ())
        .ok_or_else(|| anyhow!("target is not a mounted volume root: {}", target.display()))
}
fn export_if_requested(
    cli: &Cli,
    target: &std::path::Path,
    summary: &Summary,
    console: &Console,
) -> Result<()> {
    let Some(directory) = &cli.export else {
        return Ok(());
    };
    let params = AnalysisParams::default();
    let analysis = analyze(&summary.trace, &params);
    let files = export_run(
        directory,
        &ExportInput {
            target,
            summary,
            analysis: &analysis,
            params: &params,
        },
    )?;
    console.info(&format!(
        "Exported {} report file(s) to {}",
        files.len(),
        directory.display()
    ));
    Ok(())
}
