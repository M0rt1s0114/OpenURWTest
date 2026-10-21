//! speed_run.

use super::*;

impl App {
    pub(super) fn render_speed_lines(
        &self,
        frame: &mut Frame,
        area: Rect,
        series: SpeedSeries,
        x_max: u64,
        title: &str,
    ) {
        let y_max = series
            .iter()
            .flat_map(|(_, points, _)| points.iter().map(|(_, speed)| *speed))
            .fold(0.0f64, f64::max)
            .max(1.0)
            * 1.1;
        let data: Vec<Vec<(f64, f64)>> = series
            .iter()
            .map(|(_, points, _)| {
                points
                    .iter()
                    .map(|(position, speed)| (*position as f64, *speed))
                    .collect()
            })
            .collect();
        let datasets: Vec<Dataset> = series
            .iter()
            .enumerate()
            .map(|(index, (name, _, color))| {
                Dataset::default()
                    .name(name.clone())
                    .marker(if self.ascii {
                        Marker::Dot
                    } else {
                        Marker::Braille
                    })
                    .graph_type(GraphType::Line)
                    .style(ratatui::style::Style::default().fg(*color))
                    .data(&data[index])
            })
            .collect();
        let chart = Chart::new(datasets)
            .block(
                Block::bordered()
                    .title(format!(" {title} "))
                    .border_style(self.theme.border()),
            )
            .x_axis(
                ChartAxis::default()
                    .bounds([0.0, x_max.max(1) as f64])
                    .labels([
                        "0".to_string(),
                        format_axis_bytes(x_max / 2),
                        format_axis_bytes(x_max),
                    ]),
            )
            .y_axis(ChartAxis::default().bounds([0.0, y_max]).labels([
                "0".to_string(),
                format!("{:.0}", y_max / 2.0),
                format!("{:.0}", y_max),
            ]));
        frame.render_widget(chart, area);
    }

    pub(super) fn render_speed_bars(&self, frame: &mut Frame, area: Rect, bars: &[(&str, f64)]) {
        let max = bars
            .iter()
            .map(|(_, value)| *value)
            .fold(0.0f64, f64::max)
            .max(1.0)
            * 1.2;
        let len = bars.len().max(1);
        let write_color = self
            .theme
            .gauge_write()
            .fg
            .unwrap_or_else(|| self.theme.speed_color());
        let verify_color = self
            .theme
            .gauge_verify()
            .fg
            .unwrap_or_else(|| self.theme.speed_color());
        let chart = Canvas::default()
            .block(
                Block::bordered()
                    .title(" 4K trimmed-mean MiB/s ")
                    .border_style(self.theme.border()),
            )
            .x_bounds([0.0, len as f64 + 1.0])
            .y_bounds([-max * 0.2, max])
            .paint(|context| {
                context.print(0.02, max * 0.95, format!("{max:.0}"));
                context.print(0.02, max * 0.47, format!("{:.0}", max / 2.0));
                context.print(0.02, 0.0, "0");
                for (index, (label, value)) in bars.iter().enumerate() {
                    context.print(index as f64 + 0.85, -max * 0.12, label.to_string());
                    context.draw(&Rectangle {
                        x: index as f64 + 0.9,
                        y: 0.0,
                        width: 0.8,
                        height: value.max(0.0),
                        color: if index % 2 == 0 {
                            write_color
                        } else {
                            verify_color
                        },
                    });
                    context.print(
                        index as f64 + 1.1,
                        value.max(0.0) + max * 0.03,
                        format!("{value:.0}"),
                    );
                }
            });
        frame.render_widget(chart, area);
    }

    pub(super) fn render_speed_run(&self, frame: &mut Frame, multi: &MultiRun) {
        let area = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(4),
                Constraint::Length(6),
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(2),
            ])
            .split(area);
        let active = multi.active.min(multi.tasks.len() - 1);
        let mut spans = Vec::new();
        for (index, task) in multi.tasks.iter().enumerate() {
            let style = if index == active {
                self.theme.selected()
            } else {
                self.theme.dim()
            };
            let marker = if task.status == TaskStatus::Running {
                format!("{} ", self.tab_activity_glyph(TestKind::Speed))
            } else {
                String::new()
            };
            spans.push(Span::styled(
                format!(
                    " [{} {}{} {}] ",
                    index + 1,
                    marker,
                    task.label,
                    task.status.label()
                ),
                style,
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans))
                .block(Block::bordered().border_style(self.theme.border())),
            layout[0],
        );
        let task = &multi.tasks[active];
        let suite = task.suite.as_ref();
        let current = suite.map(|value| value.current).unwrap_or(0);
        let preset = PRESETS.get(current).copied().unwrap_or(PRESETS[0]);
        let selected_count = self
            .speed
            .values
            .presets
            .iter()
            .filter(|item| **item)
            .count();
        let ordinal = if self
            .speed
            .values
            .presets
            .get(current)
            .copied()
            .unwrap_or(false)
        {
            self.speed.values.presets[..=current.min(3)]
                .iter()
                .filter(|item| **item)
                .count()
        } else {
            0
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    " {}  速度测试  elapsed {}  temp {}",
                    task.label,
                    format_seconds(multi.started.elapsed().as_secs()),
                    task.health
                        .temperature_c
                        .map(|value| format!("{value} C"))
                        .unwrap_or_else(|| "unknown".to_string())
                )),
                Line::from(format!(
                    "当前 {}/{}  {}   文件 {}   顺序项错误继续 {}",
                    ordinal,
                    selected_count,
                    preset.name,
                    format_bytes(self.speed.values.file_size),
                    yes_no(self.speed.values.ignore_errors)
                )),
            ])
            .block(Block::bordered().border_style(self.theme.border())),
            layout[1],
        );

        let mut status_lines: Vec<Line> = Vec::new();
        if let Some(error) = &task.error {
            let style = if self.error_pulse_on() {
                self.theme
                    .danger()
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                self.theme.danger()
            };
            status_lines.push(Line::styled(error.clone(), style));
        }
        if let Some(suite) = suite {
            if let Some(phase) = suite.phase {
                status_lines.push(Line::styled(
                    format!(
                        "{} {}   {:.1} MiB/s   {}/{}",
                        self.spinner_glyph(),
                        match phase {
                            SuitePhase::Write => "写入",
                            SuitePhase::Verify => "读取校验",
                        },
                        suite.mibps,
                        format_bytes(suite.done_bytes),
                        format_bytes(suite.total_bytes.max(self.speed.values.file_size))
                    ),
                    self.theme.text(),
                ));
            }
            if let Some(offset) = suite.error_offset {
                status_lines.push(Line::styled(
                    format!("错误位置：写入量 {}", format_bytes(offset)),
                    self.theme.danger(),
                ));
            }
            if !suite.results.is_empty() {
                status_lines.push(Line::styled(
                    format!("已完成 {} 项", suite.results.len()),
                    self.theme.dim(),
                ));
            }
            let total_bytes = selected_count as u64 * 2 * self.speed.values.file_size;
            let completed = suite.results.len() as u64 * 2 * self.speed.values.file_size;
            let current_bytes = if suite.phase.is_some() {
                suite.done_bytes
            } else {
                0
            };
            let overall = completed.saturating_add(current_bytes).min(total_bytes);
            let percent = if total_bytes == 0 {
                0.0
            } else {
                overall as f64 / total_bytes as f64 * 100.0
            };
            let elapsed = multi.started.elapsed().as_secs_f64();
            let eta = if overall > 0 && overall < total_bytes {
                elapsed * (total_bytes - overall) as f64 / overall as f64
            } else {
                0.0
            };
            status_lines.push(Line::styled(
                format!(
                    "总进度 {percent:.0}%   已用 {}   ETA {}",
                    format_seconds(elapsed as u64),
                    if eta > 0.0 {
                        format_seconds(eta as u64)
                    } else {
                        "-".to_string()
                    }
                ),
                self.theme.dim(),
            ));
        }
        if status_lines.is_empty() {
            status_lines.push(Line::styled("准备测试文件...", self.theme.dim()));
        }
        frame.render_widget(
            Paragraph::new(status_lines).block(
                Block::bordered()
                    .title(" 状态 ")
                    .border_style(self.theme.border()),
            ),
            layout[2],
        );

        let done = suite.map(|value| value.done_bytes).unwrap_or(0);
        let total = suite
            .map(|value| value.total_bytes)
            .filter(|value| *value > 0)
            .unwrap_or(self.speed.values.file_size);
        let ratio = if total > 0 {
            (done as f64 / total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let gauge = Gauge::default()
            .block(
                Block::bordered()
                    .title(
                        suite
                            .and_then(|value| value.phase)
                            .map(|phase| phase.label())
                            .unwrap_or("starting"),
                    )
                    .border_style(self.theme.border()),
            )
            .gauge_style(
                if suite.and_then(|value| value.phase) == Some(SuitePhase::Verify) {
                    self.theme.gauge_verify()
                } else {
                    self.theme.gauge_write()
                },
            )
            .ratio(ratio)
            .label(format!("{} / {}", format_bytes(done), format_bytes(total)));
        frame.render_widget(gauge, layout[3]);

        if let (Some(_suite), Some(offset)) = (suite, suite.and_then(|value| value.error_offset))
            && total > 0
        {
            let inner_x = layout[3].x + 1;
            let inner_width = layout[3].width.saturating_sub(2);
            let marker_x = inner_x
                + ((offset as f64 / total as f64).clamp(0.0, 1.0) * inner_width as f64) as u16;
            let marker = Rect::new(
                marker_x.min(inner_x + inner_width.saturating_sub(2)),
                layout[3].y + 1,
                2,
                layout[3].height.saturating_sub(2),
            );
            frame.render_widget(
                Block::default().style(
                    ratatui::style::Style::default().bg(self
                        .theme
                        .danger()
                        .fg
                        .unwrap_or(ratatui::style::Color::Red)),
                ),
                marker,
            );
        }

        let series = vec![
            (
                "write".to_string(),
                suite
                    .and_then(|view| view.write_curves.get(current))
                    .cloned()
                    .unwrap_or_default(),
                self.theme
                    .chart_speed()
                    .fg
                    .unwrap_or_else(|| self.theme.speed_color()),
            ),
            (
                "read".to_string(),
                suite
                    .and_then(|view| view.read_curves.get(current))
                    .cloned()
                    .unwrap_or_default(),
                self.theme
                    .gauge_verify()
                    .fg
                    .unwrap_or_else(|| self.theme.speed_color()),
            ),
        ];
        self.render_speed_lines(
            frame,
            layout[4],
            series,
            self.speed.values.file_size,
            &format!("速度 MiB/s · {}", preset.name),
        );

        let speed_footer = if multi.tasks.len() > 1 {
            format!(
                "Tab 切换  1-9 直达  Esc/Ctrl+C 取消菜单   F2 鼠标:{}",
                if self.mouse_enabled { "开" } else { "关" }
            )
        } else {
            format!(
                "Esc/Ctrl+C 取消菜单   F2 鼠标:{}",
                if self.mouse_enabled { "开" } else { "关" }
            )
        };
        frame.render_widget(
            Paragraph::new(speed_footer).style(self.theme.dim()),
            layout[5],
        );
        self.record_multi_hotspots(layout[0], layout[5], multi.tasks.len());
        if multi.cancel_menu {
            let popup = centered_rect(frame.area(), 46, 30);
            frame.render_widget(Clear, popup);
            let mut menu_lines = vec![
                Line::styled("取消测试", self.theme.warning()),
                Line::from(""),
            ];
            if multi.tasks.len() > 1 {
                menu_lines.push(Line::from("1 / Enter 停止当前盘"));
                menu_lines.push(Line::from("2 / S 停止全部"));
                menu_lines.push(Line::from("3 / Esc 返回"));
            } else {
                menu_lines.push(Line::from("Enter 停止当前盘"));
                menu_lines.push(Line::from("Esc 返回"));
            }
            frame.render_widget(
                Paragraph::new(menu_lines)
                    .block(
                        Block::bordered()
                            .title(" 确认 ")
                            .border_style(self.theme.warning()),
                    )
                    .wrap(Wrap { trim: true }),
                popup,
            );
        }
    }
}
