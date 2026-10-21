//! Export a run to CSV/JSON/XLSX/SVG.
//!
//! SVG charts are generated as plain text, so a full-resolution view of a long
//! run is available in any browser even when the terminal chart is downsampled.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::analysis::{Analysis, AnalysisParams, SlowKind, SpeedSample, ThresholdMode};
use crate::engine::Summary;
use crate::trace::Phase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XAxis {
    Position,
    Time,
}

pub struct ExportInput<'a> {
    pub target: &'a Path,
    pub summary: &'a Summary,
    pub analysis: &'a Analysis,
    pub params: &'a AnalysisParams,
}

pub fn export(directory: &Path, input: &ExportInput<'_>) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let mut written = Vec::new();

    let path = directory.join("samples.csv");
    write_samples(&path, &input.analysis.samples)?;
    written.push(path);

    let path = directory.join("events.csv");
    write_events(&path, input)?;
    written.push(path);

    let path = directory.join("report.json");
    write_report(&path, input)?;
    written.push(path);

    let path = directory.join("report.xlsx");
    write_xlsx(&path, input)?;
    written.push(path);

    for (name, axis) in [
        ("speed_position.svg", XAxis::Position),
        ("speed_time.svg", XAxis::Time),
    ] {
        let path = directory.join(name);
        write_svg(&path, input, axis)?;
        written.push(path);
    }

    Ok(written)
}

fn write_samples(path: &Path, samples: &[SpeedSample]) -> Result<()> {
    let mut file = fs::File::create(path)?;
    writeln!(
        file,
        "pass,phase,time_ms,position_bytes,speed_mibps,average_mibps,baseline_mibps,threshold_mibps"
    )?;
    for sample in samples {
        writeln!(
            file,
            "{},{},{},{},{:.3},{:.3},{:.3},{:.3}",
            sample.pass,
            sample.phase.label(),
            sample.run_ms,
            sample.position,
            sample.speed_mibps,
            sample.average_mibps,
            sample.baseline_mibps,
            sample.threshold_mibps
        )?;
    }
    Ok(())
}

fn write_events(path: &Path, input: &ExportInput<'_>) -> Result<()> {
    let mut file = fs::File::create(path)?;
    writeln!(
        file,
        "pass,phase,kind,start_ms,end_ms,start_position,end_position,duration_ms,min_mibps,baseline_mibps,depth_percent"
    )?;
    for event in &input.analysis.events {
        writeln!(
            file,
            "{},{},{},{},{},{},{},{},{:.3},{:.3},{:.1}",
            event.pass,
            event.phase.label(),
            event.kind.label(),
            event.start_ms,
            event.end_ms,
            event.start_position,
            event.end_position,
            event.duration_ms,
            event.min_mibps,
            event.baseline_mibps,
            event.depth_percent
        )?;
    }
    Ok(())
}

fn write_report(path: &Path, input: &ExportInput<'_>) -> Result<()> {
    let summary = input.summary;
    let params = input.params;
    let mode = match params.mode {
        ThresholdMode::Percent => "percent",
        ThresholdMode::Absolute => "absolute",
    };
    let spikes = input
        .analysis
        .events
        .iter()
        .filter(|event| event.kind == SlowKind::Spike)
        .count();
    let sustained = input.analysis.events.len() - spikes;

    let mut json = String::new();
    json.push_str("{\n");
    push_pair(
        &mut json,
        "target",
        &format!(r#""{}""#, json_escape(&input.target.display().to_string())),
    );
    push_pair(&mut json, "passes", &summary.passes.to_string());
    push_pair(&mut json, "passed", &summary.passed.to_string());
    push_pair(&mut json, "failed", &summary.failed.to_string());
    push_pair(&mut json, "wrote_bytes", &summary.wrote_bytes.to_string());
    push_pair(&mut json, "read_bytes", &summary.read_bytes.to_string());
    push_pair(&mut json, "data_errors", &summary.data_errors.to_string());
    push_pair(&mut json, "read_errors", &summary.read_errors.to_string());
    push_pair(&mut json, "write_errors", &summary.write_errors.to_string());
    push_pair(
        &mut json,
        "peak_mibps",
        &format!("{:.3}", input.analysis.peak_mibps),
    );
    push_pair(
        &mut json,
        "mean_mibps",
        &format!("{:.3}", input.analysis.mean_mibps),
    );
    push_pair(
        &mut json,
        "min_mibps",
        &format!("{:.3}", input.analysis.min_mibps),
    );
    push_pair(&mut json, "analysis_mode", &format!(r#""{mode}""#));
    push_pair(&mut json, "percent", &params.percent.to_string());
    push_pair(
        &mut json,
        "absolute_mibps",
        &params.absolute_mibps.to_string(),
    );
    push_pair(
        &mut json,
        "min_duration_ms",
        &params.min_duration_ms.to_string(),
    );
    push_pair(
        &mut json,
        "recovery_percent",
        &params.recovery_percent.to_string(),
    );
    push_pair(
        &mut json,
        "recovery_window_ms",
        &params.recovery_window_ms.to_string(),
    );
    push_pair(
        &mut json,
        "baseline_window_ms",
        &params.baseline_window_ms.to_string(),
    );
    push_pair(
        &mut json,
        "sample_interval_ms",
        &params.sample_interval_ms.to_string(),
    );
    push_pair(&mut json, "spikes", &spikes.to_string());
    push_pair(&mut json, "sustained", &sustained.to_string());
    if json.ends_with(",\n") {
        json.truncate(json.len() - 2);
        json.push('\n');
    }
    json.push_str("}\n");
    fs::write(path, json)?;
    Ok(())
}

fn push_pair(json: &mut String, key: &str, value: &str) {
    json.push_str("  ");
    json.push_str(r#"" "#.trim());
    json.push_str(key);
    json.push_str(r#"": "#);
    json.push_str(value);
    json.push_str(",\n");
}

fn json_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => output.push_str(r#"\""#),
            '\\' => output.push_str(r#"\\"#),
            '\n' => output.push_str(r#"\n"#),
            '\r' => output.push_str(r#"\r"#),
            '\t' => output.push_str(r#"\t"#),
            other => output.push(other),
        }
    }
    output
}

fn write_xlsx(path: &Path, input: &ExportInput<'_>) -> Result<()> {
    use rust_xlsxwriter::Workbook;

    let mut workbook = Workbook::new();

    {
        let sheet = workbook.add_worksheet();
        sheet.set_name("Summary")?;
        sheet.write_string(0, 0, "field")?;
        sheet.write_string(0, 1, "value")?;
        let summary = input.summary;
        let params = input.params;
        let rows: [(&str, String); 20] = [
            ("target", input.target.display().to_string()),
            ("passes", summary.passes.to_string()),
            ("passed", summary.passed.to_string()),
            ("failed", summary.failed.to_string()),
            ("wrote_bytes", summary.wrote_bytes.to_string()),
            ("read_bytes", summary.read_bytes.to_string()),
            ("data_errors", summary.data_errors.to_string()),
            ("read_errors", summary.read_errors.to_string()),
            ("write_errors", summary.write_errors.to_string()),
            ("peak_mibps", format!("{:.3}", input.analysis.peak_mibps)),
            ("mean_mibps", format!("{:.3}", input.analysis.mean_mibps)),
            ("min_mibps", format!("{:.3}", input.analysis.min_mibps)),
            (
                "analysis_mode",
                match params.mode {
                    ThresholdMode::Percent => "percent".to_string(),
                    ThresholdMode::Absolute => "absolute".to_string(),
                },
            ),
            ("percent", params.percent.to_string()),
            ("absolute_mibps", params.absolute_mibps.to_string()),
            ("min_duration_ms", params.min_duration_ms.to_string()),
            ("recovery_percent", params.recovery_percent.to_string()),
            ("recovery_window_ms", params.recovery_window_ms.to_string()),
            ("baseline_window_ms", params.baseline_window_ms.to_string()),
            ("sample_interval_ms", params.sample_interval_ms.to_string()),
        ];
        for (index, (key, value)) in rows.iter().enumerate() {
            let row = index as u32 + 1;
            sheet.write_string(row, 0, *key)?;
            sheet.write_string(row, 1, value)?;
        }
    }

    {
        let sheet = workbook.add_worksheet();
        sheet.set_name("Samples")?;
        let headers = [
            "pass",
            "phase",
            "time_ms",
            "position_bytes",
            "speed_mibps",
            "average_mibps",
            "baseline_mibps",
            "threshold_mibps",
        ];
        for (column, header) in headers.iter().enumerate() {
            sheet.write_string(0, column as u16, *header)?;
        }
        for (index, sample) in input.analysis.samples.iter().enumerate() {
            let row = index as u32 + 1;
            sheet.write_number(row, 0, sample.pass as f64)?;
            sheet.write_string(row, 1, sample.phase.label())?;
            sheet.write_number(row, 2, sample.run_ms as f64)?;
            sheet.write_number(row, 3, sample.position as f64)?;
            sheet.write_number(row, 4, sample.speed_mibps)?;
            sheet.write_number(row, 5, sample.average_mibps)?;
            sheet.write_number(row, 6, sample.baseline_mibps)?;
            sheet.write_number(row, 7, sample.threshold_mibps)?;
        }
    }

    {
        let sheet = workbook.add_worksheet();
        sheet.set_name("Events")?;
        let headers = [
            "pass",
            "phase",
            "kind",
            "start_ms",
            "end_ms",
            "start_position",
            "end_position",
            "duration_ms",
            "min_mibps",
            "baseline_mibps",
            "depth_percent",
        ];
        for (column, header) in headers.iter().enumerate() {
            sheet.write_string(0, column as u16, *header)?;
        }
        for (index, event) in input.analysis.events.iter().enumerate() {
            let row = index as u32 + 1;
            sheet.write_number(row, 0, event.pass as f64)?;
            sheet.write_string(row, 1, event.phase.label())?;
            sheet.write_string(row, 2, event.kind.label())?;
            sheet.write_number(row, 3, event.start_ms as f64)?;
            sheet.write_number(row, 4, event.end_ms as f64)?;
            sheet.write_number(row, 5, event.start_position as f64)?;
            sheet.write_number(row, 6, event.end_position as f64)?;
            sheet.write_number(row, 7, event.duration_ms as f64)?;
            sheet.write_number(row, 8, event.min_mibps)?;
            sheet.write_number(row, 9, event.baseline_mibps)?;
            sheet.write_number(row, 10, event.depth_percent)?;
        }
    }

    workbook.save(path)?;
    Ok(())
}

/// One row of a multi-drive report.
#[derive(Debug, Clone)]
pub struct MultiReportRow {
    pub target: String,
    pub mode: String,
    pub status: String,
    pub passes: u32,
    pub passed: u32,
    pub failed: u32,
    pub wrote_bytes: u64,
    pub read_bytes: u64,
    pub data_errors: u64,
    pub read_errors: u64,
    pub temperature_c: Option<i32>,
    pub health: String,
    pub read_mibps: f64,
    pub write_mibps: f64,
    pub read_iops: f64,
    pub write_iops: f64,
    pub avg_latency_ns: f64,
    pub speed_errors: u64,
}

pub fn export_multi(directory: &Path, kind: &str, rows: &[MultiReportRow]) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let mut written = Vec::new();

    let path = directory.join("multi_summary.csv");
    {
        let mut file = fs::File::create(&path)?;
        writeln!(
            file,
            "target,kind,mode,status,passes,passed,failed,wrote_bytes,read_bytes,data_errors,read_errors,temperature_c,health,read_mibps,write_mibps,read_iops,write_iops,avg_latency_ns,speed_errors"
        )?;
        for row in rows {
            writeln!(
                file,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.1},{}",
                csv_escape(&row.target),
                kind,
                csv_escape(&row.mode),
                csv_escape(&row.status),
                row.passes,
                row.passed,
                row.failed,
                row.wrote_bytes,
                row.read_bytes,
                row.data_errors,
                row.read_errors,
                row.temperature_c
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                csv_escape(&row.health),
                row.read_mibps,
                row.write_mibps,
                row.read_iops,
                row.write_iops,
                row.avg_latency_ns,
                row.speed_errors
            )?;
        }
    }
    written.push(path);

    let path = directory.join("multi_report.json");
    {
        let mut json = String::from("{\n");
        json.push_str(&format!("  \"kind\": \"{}\",\n", json_escape(kind)));
        json.push_str("  \"rows\": [\n");
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                json.push_str(",\n");
            }
            json.push_str("    {");
            json.push_str(&format!(
                "\"target\": \"{}\", \"mode\": \"{}\", \"status\": \"{}\", ",
                json_escape(&row.target),
                json_escape(&row.mode),
                json_escape(&row.status)
            ));
            json.push_str(&format!(
                "\"passes\": {}, \"passed\": {}, \"failed\": {}, ",
                row.passes, row.passed, row.failed
            ));
            json.push_str(&format!(
                "\"wrote_bytes\": {}, \"read_bytes\": {}, ",
                row.wrote_bytes, row.read_bytes
            ));
            json.push_str(&format!(
                "\"data_errors\": {}, \"read_errors\": {}, ",
                row.data_errors, row.read_errors
            ));
            json.push_str(&format!(
                "\"temperature_c\": {}, \"health\": \"{}\", ",
                row.temperature_c
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                json_escape(&row.health)
            ));
            json.push_str(&format!(
                "\"read_mibps\": {:.3}, \"write_mibps\": {:.3}, ",
                row.read_mibps, row.write_mibps
            ));
            json.push_str(&format!(
                "\"read_iops\": {:.3}, \"write_iops\": {:.3}, ",
                row.read_iops, row.write_iops
            ));
            json.push_str(&format!(
                "\"avg_latency_ns\": {:.1}, \"speed_errors\": {}",
                row.avg_latency_ns, row.speed_errors
            ));
            json.push('}');
        }
        json.push_str("\n  ]\n}\n");
        fs::write(&path, json)?;
    }
    written.push(path);

    let path = directory.join("multi_report.xlsx");
    {
        use rust_xlsxwriter::Workbook;
        let mut workbook = Workbook::new();
        let sheet = workbook.add_worksheet();
        sheet.set_name("Summary")?;
        let headers = [
            "target",
            "kind",
            "mode",
            "status",
            "passes",
            "passed",
            "failed",
            "wrote_bytes",
            "read_bytes",
            "data_errors",
            "read_errors",
            "temperature_c",
            "health",
            "read_mibps",
            "write_mibps",
            "read_iops",
            "write_iops",
            "avg_latency_ns",
            "speed_errors",
        ];
        for (column, header) in headers.iter().enumerate() {
            sheet.write_string(0, column as u16, *header)?;
        }
        for (index, row) in rows.iter().enumerate() {
            let row_index = index as u32 + 1;
            sheet.write_string(row_index, 0, &row.target)?;
            sheet.write_string(row_index, 1, kind)?;
            sheet.write_string(row_index, 2, &row.mode)?;
            sheet.write_string(row_index, 3, &row.status)?;
            sheet.write_number(row_index, 4, row.passes as f64)?;
            sheet.write_number(row_index, 5, row.passed as f64)?;
            sheet.write_number(row_index, 6, row.failed as f64)?;
            sheet.write_number(row_index, 7, row.wrote_bytes as f64)?;
            sheet.write_number(row_index, 8, row.read_bytes as f64)?;
            sheet.write_number(row_index, 9, row.data_errors as f64)?;
            sheet.write_number(row_index, 10, row.read_errors as f64)?;
            if let Some(temperature) = row.temperature_c {
                sheet.write_number(row_index, 11, temperature as f64)?;
            }
            sheet.write_string(row_index, 12, &row.health)?;
            sheet.write_number(row_index, 13, row.read_mibps)?;
            sheet.write_number(row_index, 14, row.write_mibps)?;
            sheet.write_number(row_index, 15, row.read_iops)?;
            sheet.write_number(row_index, 16, row.write_iops)?;
            sheet.write_number(row_index, 17, row.avg_latency_ns)?;
            sheet.write_number(row_index, 18, row.speed_errors as f64)?;
        }
        workbook.save(&path)?;
    }
    written.push(path);

    Ok(written)
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn write_svg(path: &Path, input: &ExportInput<'_>, axis: XAxis) -> Result<()> {
    let samples = &input.analysis.samples;
    let phase_filter = match axis {
        XAxis::Position => Some(Phase::Write),
        XAxis::Time => None,
    };
    let filtered: Vec<&SpeedSample> = samples
        .iter()
        .filter(|sample| phase_filter.is_none_or(|phase| sample.phase == phase))
        .collect();
    if filtered.is_empty() {
        fs::write(path, empty_svg(axis))?;
        return Ok(());
    }

    let width = 1000.0;
    let height = 420.0;
    let left = 70.0;
    let right = 20.0;
    let top = 30.0;
    let bottom = 55.0;
    let plot_width = width - left - right;
    let plot_height = height - top - bottom;

    let max_x = filtered
        .iter()
        .map(|sample| x_value(sample, axis))
        .fold(0.0, f64::max)
        .max(1.0);
    let max_y = filtered
        .iter()
        .map(|sample| sample.speed_mibps.max(sample.baseline_mibps))
        .fold(0.0, f64::max)
        .max(1.0)
        * 1.1;

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}' viewBox='0 0 {width} {height}'>"
    ));
    svg.push_str("<rect width='100%' height='100%' fill='#0d1117'/>");

    for step in 0..=4 {
        let fraction = step as f64 / 4.0;
        let gy = top + plot_height * (1.0 - fraction);
        let value = max_y * fraction;
        svg.push_str(&format!(
            "<line x1='{left}' y1='{gy:.1}' x2='{:.1}' y2='{gy:.1}' stroke='#30363d'/>",
            left + plot_width
        ));
        svg.push_str(&format!(
            "<text x='{:.1}' y='{:.1}' fill='#8b949e' font-size='12'>{value:.0}</text>",
            left - 8.0,
            gy + 4.0
        ));
    }
    for step in 0..=5 {
        let fraction = step as f64 / 5.0;
        let gx = left + plot_width * fraction;
        let value = max_x * fraction;
        svg.push_str(&format!(
            "<line x1='{gx:.1}' y1='{top}' x2='{gx:.1}' y2='{:.1}' stroke='#30363d'/>",
            top + plot_height
        ));
        let label = match axis {
            XAxis::Position => format!("{:.1} GiB", value / (1024.0 * 1024.0 * 1024.0)),
            XAxis::Time => format!("{:.1} min", value / 60000.0),
        };
        svg.push_str(&format!(
            "<text x='{:.1}' y='{:.1}' fill='#8b949e' font-size='12'>{label}</text>",
            gx - 12.0,
            top + plot_height + 18.0
        ));
    }

    for event in &input.analysis.events {
        if phase_filter.is_some() && phase_filter != Some(event.phase) {
            continue;
        }
        let start = match axis {
            XAxis::Position => event.start_position as f64,
            XAxis::Time => event.start_ms as f64,
        };
        let end = match axis {
            XAxis::Position => event.end_position as f64,
            XAxis::Time => event.end_ms as f64,
        };
        let x0 = left + start / max_x * plot_width;
        let x1 = left + end / max_x * plot_width;
        let color = match event.kind {
            SlowKind::Spike => "#f85149",
            SlowKind::Sustained => "#d29922",
        };
        svg.push_str(&format!(
            "<rect x='{x0:.1}' y='{top}' width='{:.1}' height='{plot_height}' fill='{color}' fill-opacity='0.15'/>",
            (x1 - x0).max(1.0)
        ));
    }

    let mut speed_points = String::new();
    let mut baseline_points = String::new();
    for sample in &filtered {
        let px = left + x_value(sample, axis) / max_x * plot_width;
        let py = top + plot_height - sample.speed_mibps / max_y * plot_height;
        speed_points.push_str(&format!("{px:.1},{py:.1} "));
        let by = top + plot_height - sample.baseline_mibps / max_y * plot_height;
        baseline_points.push_str(&format!("{px:.1},{by:.1} "));
    }
    svg.push_str(&format!(
        "<polyline points='{}' fill='none' stroke='#8b949e' stroke-width='1' stroke-dasharray='4 4'/>",
        baseline_points.trim()
    ));
    svg.push_str(&format!(
        "<polyline points='{}' fill='none' stroke='#58a6ff' stroke-width='1.5'/>",
        speed_points.trim()
    ));
    svg.push_str(&format!(
        "<text x='{left}' y='18' fill='#c9d1d9' font-size='14'>speed (MiB/s) vs {}</text>",
        match axis {
            XAxis::Position => "write position",
            XAxis::Time => "time",
        }
    ));
    svg.push_str("</svg>");
    fs::write(path, svg)?;
    Ok(())
}

fn x_value(sample: &SpeedSample, axis: XAxis) -> f64 {
    match axis {
        XAxis::Position => sample.position as f64,
        XAxis::Time => sample.run_ms as f64,
    }
}

fn empty_svg(axis: XAxis) -> String {
    format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='1000' height='420'><rect width='100%' height='100%' fill='#0d1117'/><text x='40' y='200' fill='#c9d1d9'>no {} data</text></svg>",
        match axis {
            XAxis::Position => "position",
            XAxis::Time => "time",
        }
    )
}
#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::analysis::analyze;
    use crate::engine::Summary;
    use crate::trace::RunTrace;

    fn sample_trace() -> RunTrace {
        let mut trace = RunTrace::default();
        trace.record(1, Phase::Write, 0, 0, 0);
        for index in 1..=100u64 {
            trace.record(1, Phase::Write, index * 1024 * 1024, index * 10, index * 10);
        }
        for index in 1..=50u64 {
            trace.record(
                1,
                Phase::Verify,
                index * 1024 * 1024,
                index * 10,
                1000 + index * 10,
            );
        }
        trace
    }

    #[test]
    fn exports_multi_drive_report() {
        let directory =
            std::env::temp_dir().join(format!("urwtest-rs-multi-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let rows = vec![MultiReportRow {
            target: "X:".to_string(),
            mode: "write+verify".to_string(),
            status: "done".to_string(),
            passes: 1,
            passed: 1,
            failed: 0,
            wrote_bytes: 1024,
            read_bytes: 1024,
            data_errors: 0,
            read_errors: 0,
            temperature_c: Some(41),
            health: "ok".to_string(),
            read_mibps: 0.0,
            write_mibps: 100.0,
            read_iops: 0.0,
            write_iops: 10.0,
            avg_latency_ns: 1000.0,
            speed_errors: 0,
        }];
        let files = export_multi(&directory, "quality", &rows).expect("export multi");
        assert_eq!(files.len(), 3);
        let csv = std::fs::read_to_string(directory.join("multi_summary.csv")).expect("csv");
        assert!(csv.contains("X:"));
        let json = std::fs::read_to_string(directory.join("multi_report.json")).expect("json");
        assert!(json.contains("\"target\""));
        let xlsx = std::fs::read(directory.join("multi_report.xlsx")).expect("xlsx");
        assert!(xlsx.starts_with(b"PK"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn exports_all_report_files() {
        let directory =
            std::env::temp_dir().join(format!("urwtest-rs-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let summary = Summary {
            passes: 1,
            passed: 1,
            trace: sample_trace(),
            ..Summary::default()
        };
        let params = AnalysisParams::default();
        let analysis = analyze(&summary.trace, &params);
        let input = ExportInput {
            target: Path::new("X:"),
            summary: &summary,
            analysis: &analysis,
            params: &params,
        };
        let files = export(&directory, &input).expect("export");
        assert_eq!(files.len(), 6);
        let samples = std::fs::read_to_string(directory.join("samples.csv")).expect("samples");
        assert!(samples.lines().count() > 10);
        let svg = std::fs::read_to_string(directory.join("speed_position.svg")).expect("svg");
        assert!(svg.contains("<polyline"));
        assert!(svg.contains("speed (MiB/s)"));
        let json = std::fs::read_to_string(directory.join("report.json")).expect("json");
        assert!(json.contains("analysis_mode"));
        let xlsx = std::fs::read(directory.join("report.xlsx")).expect("xlsx");
        assert!(xlsx.starts_with(b"PK"));
        assert!(xlsx.len() > 1024);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
