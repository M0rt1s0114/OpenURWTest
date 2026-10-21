use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use plotters::prelude::*;

use crate::png::{HEIGHT, WIDTH, draw_error, drive_suffix, ensure_font};
use crate::speed::suite::{PRESETS, SuiteResult};

const MIB: f64 = 1024.0 * 1024.0;
const SEQ_WRITE_Q8: RGBColor = RGBColor(31, 119, 180);
const SEQ_READ_Q8: RGBColor = RGBColor(255, 127, 14);
const SEQ_WRITE_Q1: RGBColor = RGBColor(44, 160, 44);
const SEQ_READ_Q1: RGBColor = RGBColor(148, 103, 189);

pub fn write_speed_pngs(
    directory: &Path,
    target: &Path,
    result: &SuiteResult,
    include_temperature: bool,
) -> Result<Vec<PathBuf>> {
    ensure_font();
    std::fs::create_dir_all(directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    let mut written = Vec::new();
    for (name, random) in [("speed_seq", false), ("speed_rand", true)] {
        let path = directory.join(format!("{name}_{}.png", drive_suffix(target)));
        write_speed_png(
            &path,
            target,
            result,
            random,
            if include_temperature {
                &result.temperature_points
            } else {
                &[]
            },
        )?;
        written.push(path);
    }
    Ok(written)
}

fn write_speed_png(
    path: &Path,
    target: &Path,
    result: &SuiteResult,
    random: bool,
    temperatures: &[(u64, i32)],
) -> Result<()> {
    let root = BitMapBackend::new(path, (WIDTH, HEIGHT)).into_drawing_area();
    root.fill(&WHITE).map_err(draw_error)?;
    let entries: Vec<(usize, &crate::speed::suite::PresetResult)> = result
        .presets
        .iter()
        .filter(|item| {
            PRESETS
                .get(item.preset_index)
                .is_some_and(|preset| preset.random == random)
        })
        .map(|item| (item.preset_index, item))
        .collect();
    let max_x = entries
        .iter()
        .flat_map(|(_, item)| item.write_curve.iter().chain(item.read_curve.iter()))
        .map(|(bytes, _)| *bytes as f64 / MIB)
        .fold(result.file_size as f64 / MIB, f64::max)
        .max(1.0);
    let max_y = entries
        .iter()
        .flat_map(|(_, item)| item.write_curve.iter().chain(item.read_curve.iter()))
        .map(|(_, mibps)| *mibps)
        .fold(1.0, f64::max)
        * 1.1;
    let min_temp = temperatures
        .iter()
        .map(|(_, value)| *value as f64)
        .fold(f64::MAX, f64::min)
        - 5.0;
    let max_temp = temperatures
        .iter()
        .map(|(_, value)| *value as f64)
        .fold(f64::MIN, f64::max)
        + 5.0;
    let chart = ChartBuilder::on(&root)
        .caption(
            format!(
                "{} {}",
                drive_suffix(target),
                if random {
                    "随机读写"
                } else {
                    "顺序读写"
                }
            ),
            ("sans-serif", 32),
        )
        .margin(24)
        .x_label_area_size(54)
        .y_label_area_size(80)
        .right_y_label_area_size(80)
        .build_cartesian_2d(0.0f64..max_x, 0.0f64..max_y)
        .map_err(draw_error)?;
    let mut chart = chart.set_secondary_coord(0.0f64..max_x, min_temp..max_temp);
    chart
        .configure_mesh()
        .x_desc("文件位置 (MiB)")
        .y_desc("MiB/s")
        .draw()
        .map_err(draw_error)?;
    if !temperatures.is_empty() {
        chart
            .configure_secondary_axes()
            .y_desc("温度 (C)")
            .draw()
            .map_err(draw_error)?;
    }

    let palette: [(RGBColor, RGBColor); 2] = if random {
        [
            (RGBColor(214, 39, 40), RGBColor(140, 86, 75)),
            (RGBColor(227, 119, 194), RGBColor(127, 127, 127)),
        ]
    } else {
        [(SEQ_WRITE_Q8, SEQ_READ_Q8), (SEQ_WRITE_Q1, SEQ_READ_Q1)]
    };
    for (slot, (index, item)) in entries.iter().enumerate() {
        let preset = &PRESETS[*index];
        let (write_color, read_color) = palette[slot.min(palette.len() - 1)];
        let write: Vec<(f64, f64)> = item
            .write_curve
            .iter()
            .map(|(bytes, mibps)| (*bytes as f64 / MIB, *mibps))
            .collect();
        let read: Vec<(f64, f64)> = item
            .read_curve
            .iter()
            .map(|(bytes, mibps)| (*bytes as f64 / MIB, *mibps))
            .collect();
        if !write.is_empty() {
            chart
                .draw_series(PointSeries::of_element(
                    write.clone(),
                    4,
                    write_color,
                    &|coord, size, style| Circle::new(coord, size, style.filled()),
                ))
                .map_err(draw_error)?;
            chart
                .draw_series(LineSeries::new(write, write_color.stroke_width(2)))
                .map_err(draw_error)?
                .label(format!("{} write", preset.name))
                .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], write_color));
        }
        if !read.is_empty() {
            chart
                .draw_series(PointSeries::of_element(
                    read.clone(),
                    4,
                    read_color,
                    &|coord, size, style| TriangleMarker::new(coord, size, style.filled()),
                ))
                .map_err(draw_error)?;
            chart
                .draw_series(DashedLineSeries::new(
                    read,
                    8,
                    5,
                    read_color.stroke_width(2),
                ))
                .map_err(draw_error)?
                .label(format!("{} read", preset.name))
                .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], read_color));
        }
    }
    if !temperatures.is_empty() {
        let points: Vec<(f64, f64)> = temperatures
            .iter()
            .map(|(bytes, value)| (*bytes as f64 / MIB, *value as f64))
            .collect();
        chart
            .draw_secondary_series(LineSeries::new(
                points,
                RGBColor(214, 39, 40).stroke_width(2),
            ))
            .map_err(draw_error)?
            .label("temperature")
            .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RGBColor(214, 39, 40)));
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
