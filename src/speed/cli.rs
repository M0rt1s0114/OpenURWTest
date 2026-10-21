//! CLI entry point for the CDM-style speed suite.

use std::thread;

use anyhow::{Context, Result, bail};

use crate::cli::SpeedArgs;
use crate::speed::suite::{PRESETS, SpeedPreset, SuiteOptions, SuiteResult, run_suite};

struct TargetResult {
    target: String,
    result: Result<SuiteResult, String>,
}

pub fn run(args: &SpeedArgs) -> Result<i32> {
    let file_size = parse_size(&args.size).context("invalid file size")?;
    if args.target.is_empty() {
        bail!("at least one --target is required");
    }
    let presets = selected_presets(&args.preset)?;
    crate::cancel::reset();
    let cancel = crate::cancel::flag();
    let mut handles = Vec::with_capacity(args.target.len());
    let mut seen = std::collections::HashSet::new();

    for target in &args.target {
        if !target.exists() {
            bail!("target does not exist: {}", target.display());
        }
        if !target.is_dir() {
            bail!("target is not a directory: {}", target.display());
        }
        let canonical = std::fs::canonicalize(target)
            .with_context(|| format!("failed to resolve {}", target.display()))?;
        if !seen.insert(canonical) {
            bail!("duplicate target: {}", target.display());
        }
        let target = target.clone();
        let presets = presets.clone();
        let cancel = cancel.clone();
        let seed = args.seed.unwrap_or(0x5EED_1234_ABCD_0001);
        let ignore_errors = args.ignore_errors;
        handles.push(thread::spawn(move || {
            let options = SuiteOptions {
                file_size,
                ignore_sequential_errors: ignore_errors,
                seed,
            };
            let result = run_suite(&target, &presets, options, cancel, &mut |_| {})
                .map_err(|error| format!("{error:#}"));
            let _ = std::fs::remove_file(target.join("BENCH0001.DAT"));
            TargetResult {
                target: target.display().to_string(),
                result,
            }
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        results.push(
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("speed suite thread panicked"))?,
        );
    }

    if args.json {
        println!("{}", json_results(&results));
    } else {
        for target in &results {
            print_text_target(target);
        }
    }
    Ok(suite_exit_code(&results))
}

fn suite_exit_code(results: &[TargetResult]) -> i32 {
    let mut code = 0;
    for target in results {
        match &target.result {
            Err(_) => code = code.max(1),
            Ok(result) if result.cancelled => code = 130,
            Ok(result) => {
                if result.stopped || result.presets.iter().any(|preset| preset.failed()) {
                    code = code.max(1);
                }
            }
        }
    }
    code
}

fn selected_presets(values: &[String]) -> Result<Vec<SpeedPreset>> {
    if values.is_empty() {
        return Ok(PRESETS.to_vec());
    }
    let mut selected = Vec::new();
    for value in values {
        for part in value.split(',') {
            let part = part.trim().to_ascii_lowercase();
            if part.is_empty() {
                continue;
            }
            if part == "all" {
                return Ok(PRESETS.to_vec());
            }
            let preset = PRESETS
                .iter()
                .find(|preset| {
                    preset.id.eq_ignore_ascii_case(&part)
                        || preset.name.eq_ignore_ascii_case(&part)
                        || preset.name.to_ascii_lowercase().replace(' ', "-") == part
                })
                .copied()
                .with_context(|| format!("unknown preset '{part}'"))?;
            if !selected
                .iter()
                .any(|item: &SpeedPreset| item.id == preset.id)
            {
                selected.push(preset);
            }
        }
    }
    if selected.is_empty() {
        bail!("no speed preset selected");
    }
    Ok(selected)
}

fn print_text_target(target: &TargetResult) {
    match &target.result {
        Ok(result) => {
            println!("target:   {}", target.target);
            for item in &result.presets {
                let preset = PRESETS[item.preset_index];
                println!(
                    "{:<14} write {:>9.1} MiB/s {:>9.0} IOPS   read {:>9.1} MiB/s {:>9.0} IOPS   verify errors {}   read errors {}",
                    preset.name,
                    item.write.write_mibps,
                    item.write.write_iops,
                    item.read.read_mibps,
                    item.read.read_iops,
                    item.verify_errors,
                    item.read_errors
                );
                if item.write.total_ios > 0 {
                    println!(
                        "  latency p50 {} ns  p95 {} ns  p99 {} ns  max {} ns  ({} I/O, {:.2}s)",
                        item.write.p50_latency_ns,
                        item.write.p95_latency_ns,
                        item.write.p99_latency_ns,
                        item.write.max_latency_ns,
                        item.write.total_ios,
                        item.write.elapsed_secs
                    );
                }
            }
            if result.stopped {
                println!(
                    "stopped:  verification error at {} bytes (preset {})",
                    result
                        .stopped_at
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "?".to_string()),
                    result
                        .stopped_preset
                        .map(|index| PRESETS[index].name)
                        .unwrap_or("-")
                );
            }
            println!();
        }
        Err(error) => {
            println!("target:   {}", target.target);
            println!("error:    {error}");
            println!();
        }
    }
}

fn json_results(results: &[TargetResult]) -> String {
    let entries: Vec<String> = results.iter().map(json_target).collect();
    format!("[{}]", entries.join(","))
}

fn json_target(target: &TargetResult) -> String {
    let mut output = format!("\"target\":\"{}\"", json_escape(&target.target));
    match &target.result {
        Ok(result) => {
            output.push_str(&format!(
                ",\"stopped\":{},\"cancelled\":{},\"stopped_at\":{}",
                result.stopped,
                result.cancelled,
                result
                    .stopped_at
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "null".to_string())
            ));
            let presets: Vec<String> = result
                .presets
                .iter()
                .map(|item| {
                    let preset = PRESETS[item.preset_index];
                    format!(
                        "{{\"name\":\"{}\",\"write_mibps\":{:.3},\"read_mibps\":{:.3},\"write_iops\":{:.3},\"read_iops\":{:.3},\"write_avg_latency_ns\":{:.1},\"read_avg_latency_ns\":{:.1},\"write_errors\":{},\"read_errors\":{},\"verify_errors\":{},\"stopped\":{}}}",
                        preset.name,
                        item.write.write_mibps,
                        item.read.read_mibps,
                        item.write.write_iops,
                        item.read.read_iops,
                        item.write.avg_latency_ns,
                        item.read.avg_latency_ns,
                        item.write_errors,
                        item.read_errors,
                        item.verify_errors,
                        item.stopped
                    )
                })
                .collect();
            output.push_str(&format!(",\"presets\":[{}]", presets.join(",")));
        }
        Err(error) => {
            output.push_str(&format!(",\"error\":\"{}\"", json_escape(error)));
        }
    }
    format!("{{{output}}}")
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch < ' ' => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

pub fn parse_size(value: &str) -> Result<u64> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() {
        bail!("empty size");
    }
    let (number, suffix) = match value.find(|c: char| !c.is_ascii_digit()) {
        Some(index) => value.split_at(index),
        None => (value.as_str(), ""),
    };
    let number: u64 = number
        .parse()
        .with_context(|| format!("invalid number '{number}'"))?;
    let multiplier = match suffix {
        "" | "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        "t" | "tb" => 1024u64.pow(4),
        other => bail!("unknown size suffix '{other}'"),
    };
    number
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow::anyhow!("size is too large"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes_with_suffixes() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("4k").unwrap(), 4096);
        assert_eq!(parse_size("1m").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("2g").unwrap(), 2 * 1024 * 1024 * 1024);
        assert!(parse_size("").is_err());
        assert!(parse_size("12x").is_err());
    }

    #[test]
    fn selects_all_presets_by_default() {
        assert_eq!(selected_presets(&[]).expect("presets").len(), 4);
    }

    #[test]
    fn exit_code_reflects_suite_outcome() {
        let ok = TargetResult {
            target: "X:\\".to_string(),
            result: Ok(SuiteResult::default()),
        };
        assert_eq!(suite_exit_code(&[ok]), 0);

        let cancelled = TargetResult {
            target: "X:\\".to_string(),
            result: Ok(SuiteResult {
                cancelled: true,
                ..SuiteResult::default()
            }),
        };
        assert_eq!(suite_exit_code(&[cancelled]), 130);

        let failed = TargetResult {
            target: "X:\\".to_string(),
            result: Ok(SuiteResult {
                presets: vec![crate::speed::suite::PresetResult {
                    verify_errors: 1,
                    ..Default::default()
                }],
                ..SuiteResult::default()
            }),
        };
        assert_eq!(suite_exit_code(&[failed]), 1);

        let error = TargetResult {
            target: "X:\\".to_string(),
            result: Err("broken".to_string()),
        };
        assert_eq!(suite_exit_code(&[error]), 1);
    }

    #[test]
    fn selects_named_presets() {
        let presets = selected_presets(&["seq1m-q8t1".to_string()]).expect("presets");
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].id, "SEQ1M_Q8T1");
    }
}
