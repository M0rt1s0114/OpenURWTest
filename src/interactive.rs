use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::console::{Console, format_bytes};
use crate::engine::{self, RunOptions};
use crate::format;
use crate::reporter::LineReporter;
use crate::types::ProgressMode;
use crate::volume::{self, VolumeInfo};

pub fn run(console: &Console, progress: ProgressMode) -> Result<()> {
    let volumes = volume::list_volumes();
    if volumes.is_empty() {
        bail!("no testable mounted volumes found");
    }

    println!("Available volumes:");
    console.print_volumes(&volumes);
    println!();

    let target = prompt_target(&volumes)?;
    let volume = volume::find_volume(&target)
        .with_context(|| format!("{} is not a mounted volume root", target.display()))?;

    let verify = prompt_yes_no("Verify test data after writing?", true)?;
    let existing = format::scan_test_files(&target)?;
    if !existing.is_empty() && !verify {
        console.info("Test data already exists on this volume; nothing to do.");
        return Ok(());
    }

    let passes = prompt_u32("Number of passes", 1)?;
    let continue_on_error = if passes > 1 {
        prompt_yes_no(
            "Continue to the next pass after verification errors?",
            false,
        )?
    } else {
        false
    };

    print_details(&volume);
    println!("Target: {}", volume.display_name());
    println!("Passes: {passes}");
    if !existing.is_empty() {
        println!("Mode:   verify existing test data");
    } else if verify {
        println!("Mode:   write then verify");
    } else {
        println!("Mode:   write only");
    }

    let options = RunOptions {
        target: &target,
        passes,
        verify,
        continue_on_error,
        cancel: None,
    };
    let mut reporter = LineReporter::new(console, progress);
    let _summary = engine::run_os(&options, &mut reporter)?;
    println!();
    print!("Press Enter to exit...");
    io::stdout().flush()?;
    let _ = read_input();
    Ok(())
}

fn print_details(volume: &VolumeInfo) {
    println!();
    println!("Device:   {}", volume.display_name());
    println!("Model:    {}", volume.device_name());
    println!(
        "Type:     {}{}",
        volume.kind.label(),
        if volume.removable { " (removable)" } else { "" }
    );
    println!("Filesystem: {}", volume.fs);
    println!(
        "Capacity: {} total, {} free",
        format_bytes(volume.total_bytes),
        format_bytes(volume.free_bytes)
    );
    println!();
}

fn prompt_target(volumes: &[VolumeInfo]) -> Result<PathBuf> {
    loop {
        print!("Enter volume number or path (for example 2 or E:): ");
        io::stdout().flush()?;

        let input = read_input()?;
        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        if let Ok(index) = input.parse::<usize>() {
            if index >= 1 && index <= volumes.len() {
                return Ok(volumes[index - 1].mount_point.clone());
            }
            eprintln!("Invalid volume number.");
            continue;
        }

        let candidate = if input.len() == 1 && input.chars().all(|ch| ch.is_ascii_alphabetic()) {
            PathBuf::from(format!("{}:", input.to_ascii_uppercase()))
        } else {
            PathBuf::from(input)
        };

        if volume::find_volume(&candidate).is_some() {
            return Ok(candidate);
        }
        eprintln!("{} is not a mounted volume root.", candidate.display());
    }
}

fn prompt_yes_no(prompt: &str, default: bool) -> Result<bool> {
    loop {
        let suffix = if default { "[Y/n]" } else { "[y/N]" };
        print!("{prompt} {suffix} ");
        io::stdout().flush()?;

        let input = read_input()?;
        let input = input.trim().to_ascii_lowercase();

        match input.as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => eprintln!("Please answer y or n."),
        }
    }
}

fn prompt_u32(prompt: &str, default: u32) -> Result<u32> {
    loop {
        print!("{prompt} [{default}] ");
        io::stdout().flush()?;

        let input = read_input()?;
        let input = input.trim();

        if input.is_empty() {
            return Ok(default);
        }
        match input.parse::<u32>() {
            Ok(value) if value >= 1 => return Ok(value),
            _ => eprintln!("Please enter a positive integer."),
        }
    }
}

fn read_input() -> Result<String> {
    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        bail!("standard input closed");
    }
    Ok(input)
}
