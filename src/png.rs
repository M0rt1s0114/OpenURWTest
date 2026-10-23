//! Direct PNG chart export using plotters.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use plotters::prelude::*;
use plotters::style::{FontStyle, register_font};
use std::sync::Once;

use crate::analysis::Analysis;
use crate::health::TemperatureSample;
use crate::trace::Phase;

pub(super) const WIDTH: u32 = 1920;
pub(super) const HEIGHT: u32 = 1080;

pub fn drive_suffix(target: &Path) -> String {
    let path = target.to_string_lossy();
    let Some(letter) = path.chars().find(|ch| ch.is_ascii_alphabetic()) else {
        return "disk".to_string();
    };
    let mut suffix = letter.to_ascii_uppercase().to_string();
    let trimmed = path.trim_end_matches(['\\', '/']);
    let is_drive_path = trimmed.as_bytes().get(1) == Some(&b':');
    if (is_drive_path && trimmed.len() > 2) || trimmed.contains(['/', '\\']) {
        let mut hash = 0x811c_9dc5u32;
        let hash_path = if cfg!(windows) {
            trimmed.to_ascii_lowercase()
        } else {
            trimmed.to_string()
        };
        for byte in hash_path.bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
        suffix.push_str(&format!("_{hash:08x}"));
    }
    suffix
}

pub fn quality_file_name(target: &Path) -> String {
    format!("quality_speed_temperature_{}.png", drive_suffix(target))
}

static FONT_INIT: Once = Once::new();

pub(super) fn ensure_font() {
    FONT_INIT.call_once(|| {
        let bytes = include_bytes!("../assets/DejaVuSansMono.ttf");
        let _ = register_font("sans-serif", FontStyle::Normal, bytes);
        let _ = register_font("sans-serif", FontStyle::Bold, bytes);
    });
}

type Bucket = ((f64, f64), (f64, f64));

fn compact(points: &[(f64, f64)], max_x: f64) -> Vec<(f64, f64)> {
    const BUCKETS: usize = 1600;
    if points.len() <= BUCKETS {
        return points.to_vec();
    }
    let mut buckets: Vec<Option<Bucket>> = vec![None; BUCKETS];
    for &(x, y) in points {
        let index = ((x / max_x.max(f64::MIN_POSITIVE)) * BUCKETS as f64) as usize;
        let index = index.min(BUCKETS - 1);
        match &mut buckets[index] {
            Some((low, high)) => {
                if y < low.1 {
                    *low = (x, y);
                }
                if y > high.1 {
                    *high = (x, y);
                }
            }
            slot => *slot = Some(((x, y), (x, y))),
        }
    }
    let mut out = Vec::with_capacity(BUCKETS * 2);
    for slot in buckets.into_iter().flatten() {
        out.push(slot.0);
        if slot.0 != slot.1 {
            out.push(slot.1);
        }
    }
    out
}

pub fn write_quality_png(
    path: &Path,
    target: &Path,
    analysis: &Analysis,
    temperatures: &[TemperatureSample],
) -> Result<()> {
    ensure_font();
    let root = BitMapBackend::new(path, (WIDTH, HEIGHT)).into_drawing_area();
    root.fill(&WHITE).map_err(draw_error)?;
    if analysis.samples.is_empty() {
        root.draw(&Text::new(
            "no samples",
            (WIDTH as i32 / 2 - 40, HEIGHT as i32 / 2),
            ("sans-serif", 32).into_font().color(&BLACK),
        ))
        .map_err(draw_error)?;
        root.present().map_err(draw_error)?;
        return Ok(());
    }

    let max_time = analysis
        .samples
        .iter()
        .map(|sample| sample.run_ms as f64 / 1000.0)
        .fold(1.0, f64::max);
    let max_speed = analysis
        .samples
        .iter()
        .map(|sample| sample.speed_mibps)
        .fold(1.0, f64::max)
        * 1.1;
    let temperature: Vec<(f64, f64)> = temperatures
        .iter()
        .map(|sample| {
            (
                sample.elapsed_ms as f64 / 1000.0,
                sample.temperature_c as f64,
            )
        })
        .collect();
    let temperature = compact(&temperature, max_time);
    let (min_temp, max_temp) = if temperature.is_empty() {
        (0.0, 1.0)
    } else {
        (
            temperature
                .iter()
                .map(|(_, value)| *value)
                .fold(f64::MAX, f64::min)
                - 5.0,
            temperature
                .iter()
                .map(|(_, value)| *value)
                .fold(f64::MIN, f64::max)
                + 5.0,
        )
    };
    let chart = ChartBuilder::on(&root)
        .caption(
            format!("{} speed and temperature", drive_suffix(target)),
            ("sans-serif", 32),
        )
        .margin(24)
        .x_label_area_size(54)
        .y_label_area_size(80)
        .right_y_label_area_size(80)
        .build_cartesian_2d(0.0f64..max_time, 0.0f64..max_speed)
        .map_err(draw_error)?;
    let mut chart = chart.set_secondary_coord(0.0f64..max_time, min_temp..max_temp);
    chart
        .configure_mesh()
        .x_desc("time (s)")
        .y_desc("MiB/s")
        .draw()
        .map_err(draw_error)?;
    if !temperature.is_empty() {
        chart
            .configure_secondary_axes()
            .y_desc("Temperature (C)")
            .draw()
            .map_err(draw_error)?;
    }

    let write: Vec<(f64, f64)> = analysis
        .samples
        .iter()
        .filter(|sample| sample.phase == Phase::Write)
        .map(|sample| (sample.run_ms as f64 / 1000.0, sample.speed_mibps))
        .collect();
    let write = compact(&write, max_time);
    let read: Vec<(f64, f64)> = analysis
        .samples
        .iter()
        .filter(|sample| sample.phase == Phase::Verify)
        .map(|sample| (sample.run_ms as f64 / 1000.0, sample.speed_mibps))
        .collect();
    let read = compact(&read, max_time);
    if !write.is_empty() {
        chart
            .draw_series(LineSeries::new(write, BLUE.stroke_width(2)))
            .map_err(draw_error)?
            .label("write")
            .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], BLUE.stroke_width(2)));
    }
    if !read.is_empty() {
        chart
            .draw_series(LineSeries::new(read, GREEN.stroke_width(2)))
            .map_err(draw_error)?
            .label("read+verify")
            .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREEN.stroke_width(2)));
    }
    if !temperature.is_empty() {
        chart
            .draw_secondary_series(LineSeries::new(temperature, RED.stroke_width(2)))
            .map_err(draw_error)?
            .label("temperature")
            .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RED.stroke_width(2)));
    }
    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.9))
        .border_style(BLACK)
        .draw()
        .map_err(draw_error)?;
    root.present().map_err(draw_error)?;
    Ok(())
}

pub(super) fn draw_error<E: std::fmt::Debug>(error: E) -> anyhow::Error {
    anyhow!("failed to draw PNG: {error:?}")
}

pub fn write_png(
    path: &Path,
    target: &Path,
    analysis: &Analysis,
    temperatures: &[TemperatureSample],
) -> Result<()> {
    write_quality_png(path, target, analysis, temperatures)
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_keeps_small_series_intact() {
        let points = vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0)];
        assert_eq!(compact(&points, 2.0), points);
    }

    #[test]
    fn compact_preserves_bucket_extremes() {
        let mut points = Vec::new();
        for index in 0..5000 {
            let value = if index == 2500 { 999.0 } else { 10.0 };
            points.push((index as f64 / 5000.0 * 100.0, value));
        }
        let compacted = compact(&points, 100.0);
        assert!(compacted.len() <= 3200);
        assert!(compacted.iter().any(|(_, value)| *value == 999.0));
    }

    #[test]
    fn quality_file_name_uses_drive_letter() {
        assert_eq!(
            quality_file_name(Path::new("E:\\")),
            "quality_speed_temperature_E.png"
        );
    }

    #[test]
    fn drive_suffix_falls_back_for_non_drive_paths() {
        assert_eq!(drive_suffix(Path::new("tmp")), "T");
    }

    #[test]
    fn subdirectories_get_distinct_suffixes() {
        assert_ne!(
            quality_file_name(Path::new("E:\\a")),
            quality_file_name(Path::new("E:\\b"))
        );
        assert_ne!(
            quality_file_name(Path::new("/mnt/a")),
            quality_file_name(Path::new("/mnt/b"))
        );
    }

    #[test]
    fn embedded_font_can_render_png_text() {
        let path =
            std::env::temp_dir().join(format!("urwtest-font-test-{}.png", std::process::id()));
        write_quality_png(&path, Path::new("E:\\"), &Analysis::default(), &[])
            .expect("quality png");
        let bytes = std::fs::read(&path).expect("png bytes");
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        let _ = std::fs::remove_file(&path);
    }
}
