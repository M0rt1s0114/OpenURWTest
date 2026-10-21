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
    target
        .to_string_lossy()
        .chars()
        .find(|ch| ch.is_ascii_alphabetic())
        .map(|ch| ch.to_ascii_uppercase().to_string())
        .unwrap_or_else(|| "disk".to_string())
}

pub fn quality_file_name(target: &Path) -> String {
    format!("quality_speed_temperature_{}.png", drive_suffix(target))
}

static FONT_INIT: Once = Once::new();

pub(super) fn ensure_font() {
    FONT_INIT.call_once(|| {
        let candidates: &[&str] = if cfg!(windows) {
            &[
                r"C:\Windows\Fonts\msyh.ttc",
                r"C:\Windows\Fonts\simhei.ttf",
                r"C:\Windows\Fonts\arial.ttf",
            ]
        } else {
            &[
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "/usr/share/fonts/TTF/DejaVuSans.ttf",
            ]
        };
        for candidate in candidates {
            if let Ok(bytes) = std::fs::read(candidate) {
                let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
                let _ = register_font("sans-serif", FontStyle::Normal, leaked);
                let _ = register_font("sans-serif", FontStyle::Bold, leaked);
                return;
            }
        }
    });
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
    let min_temp = temperature
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::MAX, f64::min)
        - 5.0;
    let max_temp = temperature
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::MIN, f64::max)
        + 5.0;
    let chart = ChartBuilder::on(&root)
        .caption(
            format!("{} 速度与温度", drive_suffix(target)),
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
            .y_desc("温度 (C)")
            .draw()
            .map_err(draw_error)?;
    }

    let write: Vec<(f64, f64)> = analysis
        .samples
        .iter()
        .filter(|sample| sample.phase == Phase::Write)
        .map(|sample| (sample.run_ms as f64 / 1000.0, sample.speed_mibps))
        .collect();
    let read: Vec<(f64, f64)> = analysis
        .samples
        .iter()
        .filter(|sample| sample.phase == Phase::Verify)
        .map(|sample| (sample.run_ms as f64 / 1000.0, sample.speed_mibps))
        .collect();
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
