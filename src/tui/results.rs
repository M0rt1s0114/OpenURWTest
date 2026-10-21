//! results.

use super::*;

impl App {
    pub(super) fn render_multi(&self, frame: &mut Frame) {
        let Some(multi) = &self.multi else {
            return;
        };
        if multi.tasks.is_empty() {
            return;
        }
        if multi.kind == TestKind::Speed {
            self.render_speed_run(frame, multi);
            return;
        }
        let area = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(4),
                Constraint::Length(3),
                Constraint::Length(4),
                Constraint::Min(8),
                Constraint::Length(2),
            ])
            .split(area);
        let active = multi.active.min(multi.tasks.len() - 1);

        let mut spans = Vec::new();
        for (index, task) in multi.tasks.iter().enumerate() {
            let selected = index == active;
            let style = if selected {
                self.theme.selected()
            } else {
                self.theme.dim()
            };
            let marker = if task.status == TaskStatus::Running {
                format!("{} ", self.tab_activity_glyph(TestKind::Quality))
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
        let temperature = task
            .health
            .temperature_c
            .map(|value| format!("{value} C"))
            .unwrap_or_else(|| "unknown".to_string());
        let stats = task
            .latest
            .map(|progress| {
                format!(
                    "current {:.1} MiB/s   average {:.1} MiB/s   left {}   ETA {}",
                    progress.current_mibps,
                    progress.average_mibps,
                    format_bytes(progress.remaining_bytes),
                    format_seconds(progress.eta_secs)
                )
            })
            .unwrap_or_else(|| "waiting for the first sample...".to_string());
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    " {}  [{}]  temp {}  thermal {}",
                    task.label,
                    match task.mode {
                        RunMode::WriteVerify => "写入并校验",
                        RunMode::WriteOnly => "仅写入",
                        RunMode::VerifyExisting => "仅校验",
                    },
                    temperature,
                    task.health.status.label()
                )),
                Line::from(stats),
            ])
            .block(Block::bordered().border_style(self.theme.border())),
            layout[1],
        );

        let (bytes, total) = task
            .latest
            .map(|progress| (progress.bytes, progress.total_bytes))
            .unwrap_or((0, task.total.max(1)));
        let ratio = if total > 0 {
            (bytes as f64 / total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let gauge = Gauge::default()
            .block(
                Block::bordered()
                    .title(task.phase.map(Phase::label).unwrap_or("starting"))
                    .border_style(self.theme.border()),
            )
            .gauge_style(if task.phase == Some(Phase::Verify) {
                self.theme.gauge_verify()
            } else {
                self.theme.gauge_write()
            })
            .ratio(ratio)
            .label(format!("{} / {}", format_bytes(bytes), format_bytes(total)));
        frame.render_widget(gauge, layout[2]);

        let log_lines: Vec<Line> = task
            .log
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|entry| Line::styled(entry.clone(), self.theme.dim()))
            .collect();
        frame.render_widget(
            Paragraph::new(log_lines).block(
                Block::bordered()
                    .title(" Log ")
                    .border_style(self.theme.border()),
            ),
            layout[3],
        );

        self.render_history_chart(
            frame,
            layout[4],
            &task.speed_history,
            task.total,
            if task.phase == Some(Phase::Verify) {
                self.theme.gauge_verify()
            } else {
                self.theme.gauge_write()
            },
        );

        let footer_text = if multi.tasks.len() > 1 {
            format!(
                "Tab 切换  1-9 直达  Esc/Ctrl+C 取消菜单   elapsed {}   F2 鼠标:{}",
                format_seconds(multi.started.elapsed().as_secs()),
                if self.mouse_enabled { "开" } else { "关" }
            )
        } else {
            format!(
                "Esc/Ctrl+C 取消菜单   elapsed {}   F2 鼠标:{}",
                format_seconds(multi.started.elapsed().as_secs()),
                if self.mouse_enabled { "开" } else { "关" }
            )
        };
        frame.render_widget(
            Paragraph::new(footer_text).style(self.theme.dim()),
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

    pub(super) fn render_multi_results(&self, frame: &mut Frame) {
        let Some(multi) = &self.multi else {
            return;
        };
        let area = frame.area();
        if multi.kind == TestKind::Speed {
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::styled(
                format!(
                    "{:<14} {:<14} {:>9} {:>9} {:>9} {:>9} {:>7}",
                    "target", "test", "write MiB", "read MiB", "write IOPS", "read IOPS", "errors"
                ),
                self.theme.dim(),
            ));
            for task in &multi.tasks {
                if let Some(suite) = &task.suite {
                    for item in &suite.results {
                        let preset = PRESETS[item.preset_index];
                        let (write_mibps, read_mibps) = if preset.random {
                            let base = (item.preset_index - 2) * 2;
                            (
                                suite.bar_values[base].unwrap_or(item.write.write_mibps),
                                suite.bar_values[base + 1].unwrap_or(item.read.read_mibps),
                            )
                        } else {
                            (item.write.write_mibps, item.read.read_mibps)
                        };
                        let errors = item.verify_errors + item.read_errors + item.write_errors;
                        let style = if item.failed() || item.stopped {
                            self.theme.danger()
                        } else {
                            self.theme.success()
                        };
                        lines.push(Line::styled(
                            format!(
                                "{} {} {:>9.1} {:>9.1} {:>9.0} {:>9.0} {:>7}",
                                pad_display(&task.label, 14),
                                pad_display(preset.name, 14),
                                write_mibps,
                                read_mibps,
                                item.write.write_iops,
                                item.read.read_iops,
                                errors
                            ),
                            style,
                        ));
                    }
                }
                if let Some(error) = &task.error {
                    lines.push(Line::styled(
                        format!("{}: {}", task.label, error),
                        self.theme.danger(),
                    ));
                }
            }
            lines.push(Line::from(""));
            if !multi.export_message.is_empty() {
                lines.push(Line::styled(
                    multi.export_message.clone(),
                    self.theme.success(),
                ));
            }
            lines.push(Line::styled(
                "4K 表格与柱均使用去掉最高/最低值的平均数。",
                self.theme.dim(),
            ));
            lines.push(Line::styled(
                "Tab 切换盘  ↑↓/PgUp/PgDn 滚动   Esc 返回主页   e 导出",
                self.theme.dim(),
            ));
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(7),
                    Constraint::Min(9),
                    Constraint::Length(2),
                ])
                .split(area);
            frame.render_widget(
                Paragraph::new(lines)
                    .block(
                        Block::bordered()
                            .title(" CDM 速度测试结果 ")
                            .border_style(self.theme.border()),
                    )
                    .scroll((multi.results_scroll as u16, 0)),
                layout[0],
            );
            if let Some((task, suite)) = multi
                .tasks
                .get(multi.active)
                .and_then(|task| task.suite.as_ref().map(|suite| (task, suite)))
            {
                let graph = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
                    .split(layout[1]);
                let write_color = self
                    .theme
                    .chart_speed()
                    .fg
                    .unwrap_or_else(|| self.theme.speed_color());
                let verify_color = self
                    .theme
                    .gauge_verify()
                    .fg
                    .unwrap_or_else(|| self.theme.speed_color());
                let mut series = Vec::new();
                for (index, preset) in PRESETS.iter().enumerate().take(2) {
                    if index == 0 || index == 1 {
                        let write = suite.write_curves.get(index).cloned().unwrap_or_default();
                        if !write.is_empty() {
                            series.push((format!("{} write", preset.name), write, write_color));
                        }
                        let read = suite.read_curves.get(index).cloned().unwrap_or_default();
                        if !read.is_empty() {
                            series.push((format!("{} read", preset.name), read, verify_color));
                        }
                    }
                }
                self.render_speed_lines(
                    frame,
                    graph[0],
                    series,
                    self.speed.values.file_size,
                    &format!("speed MiB/s · {}", task.label),
                );
                let labels = ["Q32 W", "Q32 R", "Q1 W", "Q1 R"];
                let bars: Vec<(&str, f64)> = labels
                    .iter()
                    .enumerate()
                    .filter_map(|(index, label)| {
                        suite.bar_values[index].map(|value| (*label, value))
                    })
                    .collect();
                if !bars.is_empty() {
                    self.render_speed_bars(frame, graph[1], &bars);
                }
            }
            let footer = layout[2];
            let half = footer.width / 2;
            self.record_hotspot(
                Rect::new(footer.x, footer.y, half, 1),
                MouseAction::Key(KeyCode::Esc),
            );
            self.record_hotspot(
                Rect::new(footer.x + half, footer.y, footer.width - half, 1),
                MouseAction::Key(KeyCode::Char('e')),
            );
            return;
        }
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::styled(
            format!(
                "{:<16} {:<8} {:<8} {:>8} {:>8} {:>8} {:>6}",
                "target", "mode", "status", "passed", "failed", "errors", "temp"
            ),
            self.theme.dim(),
        ));
        for task in &multi.tasks {
            let summary = task.summary.as_ref();
            let status = if let Some(error) = &task.error {
                format!("FAIL ({error})")
            } else {
                task.status.label().to_string()
            };
            lines.push(Line::styled(
                format!(
                    "{} {} {} {:>8} {:>8} {:>8} {:>6}",
                    pad_display(&task.label, 16),
                    pad_display(
                        match task.mode {
                            RunMode::WriteVerify => "write+ver",
                            RunMode::WriteOnly => "write",
                            RunMode::VerifyExisting => "verify",
                        },
                        8
                    ),
                    pad_display(&status.chars().take(8).collect::<String>(), 8),
                    summary.map(|value| value.passed).unwrap_or(0),
                    summary.map(|value| value.failed).unwrap_or(0),
                    summary
                        .map(|value| value.data_errors + value.read_errors)
                        .unwrap_or(0),
                    task.health
                        .temperature_c
                        .map(|value| format!("{value}C"))
                        .unwrap_or_else(|| "-".to_string())
                ),
                if task.status == TaskStatus::Done && summary.is_some_and(|value| value.failed == 0)
                {
                    self.theme.success()
                } else if task.status == TaskStatus::Failed {
                    self.theme.danger()
                } else {
                    self.theme.warning()
                },
            ));
        }
        lines.push(Line::from(""));
        if let Some(multi) = &self.multi
            && !multi.export_message.is_empty()
        {
            lines.push(Line::styled(
                multi.export_message.clone(),
                self.theme.success(),
            ));
        }
        lines.push(Line::styled(
            "↑↓/PgUp/PgDn 滚动   Esc 返回主页   e 导出",
            self.theme.dim(),
        ));
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::bordered()
                        .title(" 多盘结果 ")
                        .border_style(self.theme.border()),
                )
                .scroll((multi.results_scroll as u16, 0)),
            area,
        );
    }

    pub(super) fn render_results(&self, frame: &mut Frame) {
        let Some(results) = &self.results else {
            return;
        };
        let area = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(12),
                Constraint::Min(6),
                Constraint::Length(2),
            ])
            .split(area);

        let summary = &results.summary;
        let failed = summary.failed > 0
            || summary.error_pass.is_some()
            || summary.data_errors > 0
            || summary.read_errors > 0
            || summary.write_errors > 0;
        let incomplete = summary.interrupted || summary.device_lost;
        let verified = !summary.write_only && summary.read_bytes > 0;
        let verdict = if failed {
            "FAIL"
        } else if incomplete {
            "INCOMPLETE"
        } else if !verified {
            "UNVERIFIED"
        } else {
            "PASS"
        };
        let verdict_style = match verdict {
            "PASS" => self.theme.success(),
            "FAIL" => self.theme.danger(),
            _ => self.theme.warning(),
        };
        let verdict_line = Line::from(vec![
            Span::styled(format!("{verdict}   "), verdict_style),
            Span::raw(format!(
                "{} pass(es): {} passed, {} failed   data errors {}   read errors {}   write errors {}",
                summary.passes,
                summary.passed,
                summary.failed,
                summary.data_errors,
                summary.read_errors,
                summary.write_errors
            )),
        ]);
        let stats_line = Line::styled(
            format!(
                "peak {:.1} MiB/s   mean {:.1} MiB/s   min {:.1} MiB/s",
                results.analysis.peak_mibps,
                results.analysis.mean_mibps,
                results.analysis.min_mibps
            ),
            self.theme.dim(),
        );
        frame.render_widget(
            Paragraph::new(vec![verdict_line, stats_line]).block(
                Block::bordered()
                    .title(" 结果 ")
                    .border_style(self.theme.border()),
            ),
            layout[0],
        );

        self.render_chart(frame, layout[1], results);

        let mut event_lines: Vec<Line> = Vec::new();
        if results.analysis.events.is_empty() {
            event_lines.push(Line::styled(
                "no slowdown events with the current threshold",
                self.theme.dim(),
            ));
        } else {
            for event in results.analysis.events.iter().take(8) {
                let style = match event.kind {
                    SlowKind::Spike => self.theme.danger(),
                    SlowKind::Sustained => self.theme.warning(),
                };
                event_lines.push(Line::styled(
                    format!(
                        "{}  {}  {:.1} MiB/s  depth {:.0}%  duration {}  at {}",
                        event.kind.label(),
                        event.phase.label(),
                        event.min_mibps,
                        event.depth_percent,
                        format_millis(event.duration_ms),
                        format_bytes(event.start_position)
                    ),
                    style,
                ));
            }
        }
        frame.render_widget(
            Paragraph::new(event_lines).block(
                Block::bordered()
                    .title(format!(
                        " Slowdown events ({}) ",
                        results.analysis.events.len()
                    ))
                    .border_style(self.theme.border()),
            ),
            layout[2],
        );

        frame.render_widget(
            Paragraph::new(format!(
                "threshold: {}   x axis: {}   [ ] adjust   t mode   e export   Esc back     {}",
                results.mode_label(),
                match results.axis {
                    Axis::Position => "position",
                    Axis::Time => "time",
                },
                results.message
            ))
            .style(self.theme.dim()),
            layout[3],
        );
        let footer = layout[3];
        let third = footer.width / 3;
        self.record_hotspot(
            Rect::new(footer.x, footer.y, third, 1),
            MouseAction::Key(KeyCode::Esc),
        );
        self.record_hotspot(
            Rect::new(footer.x + third, footer.y, third, 1),
            MouseAction::Key(KeyCode::Char('x')),
        );
        self.record_hotspot(
            Rect::new(footer.x + 2 * third, footer.y, footer.width - 2 * third, 1),
            MouseAction::Key(KeyCode::Char('e')),
        );
    }

    pub(super) fn render_chart(&self, frame: &mut Frame, area: Rect, results: &ResultsState) {
        let samples: Vec<_> = results
            .analysis
            .samples
            .iter()
            .filter(|sample| match results.axis {
                Axis::Position => sample.phase == Phase::Write,
                Axis::Time => true,
            })
            .collect();
        if samples.is_empty() {
            frame.render_widget(
                Paragraph::new("no samples to draw").block(
                    Block::bordered()
                        .title(" Speed ")
                        .border_style(self.theme.border()),
                ),
                area,
            );
            return;
        }
        let mut pass_base = std::collections::BTreeMap::new();
        if results.axis == Axis::Position {
            let mut base = 0u64;
            let mut passes: Vec<u32> = samples.iter().map(|sample| sample.pass).collect();
            passes.sort_unstable();
            passes.dedup();
            for pass in passes {
                let max = samples
                    .iter()
                    .filter(|sample| sample.pass == pass)
                    .map(|sample| sample.position)
                    .max()
                    .unwrap_or(0);
                pass_base.insert(pass, base);
                base = base.saturating_add(max).saturating_add(1);
            }
        }
        let x_of = |sample: &crate::analysis::SpeedSample| match results.axis {
            Axis::Position => {
                (pass_base.get(&sample.pass).copied().unwrap_or(0) + sample.position) as f64
            }
            Axis::Time => sample.run_ms as f64,
        };
        let max_x = samples
            .iter()
            .map(|sample| x_of(sample))
            .fold(0.0, f64::max)
            .max(1.0);
        let max_y = samples
            .iter()
            .map(|sample| sample.speed_mibps.max(sample.threshold_mibps))
            .fold(0.0, f64::max)
            .max(1.0);
        let stride = (samples.len() / 600).max(1);
        let mut write_points: Vec<(f64, f64)> = Vec::new();
        let mut verify_points: Vec<(f64, f64)> = Vec::new();
        let mut baseline_points: Vec<(f64, f64)> = Vec::new();
        let mut threshold_points: Vec<(f64, f64)> = Vec::new();
        for chunk in samples.chunks(stride) {
            let mut min = chunk[0];
            let mut max = chunk[0];
            for sample in chunk {
                if sample.speed_mibps < min.speed_mibps {
                    min = sample;
                }
                if sample.speed_mibps > max.speed_mibps {
                    max = sample;
                }
            }
            let mut picks = [min, max];
            picks.sort_by(|a, b| x_of(a).total_cmp(&x_of(b)));
            for sample in picks {
                let x = x_of(sample);
                baseline_points.push((x, sample.baseline_mibps));
                threshold_points.push((x, sample.threshold_mibps));
                let point = (x, sample.speed_mibps);
                match sample.phase {
                    Phase::Write => write_points.push(point),
                    Phase::Verify => verify_points.push(point),
                }
            }
        }
        let write_color = self
            .theme
            .chart_speed()
            .fg
            .unwrap_or_else(|| self.theme.speed_color());
        let verify_color = self
            .theme
            .gauge_verify()
            .fg
            .unwrap_or(ratatui::style::Color::Green);
        let mut datasets = Vec::new();
        if !write_points.is_empty() {
            datasets.push(
                Dataset::default()
                    .name("write")
                    .marker(if self.ascii {
                        Marker::Dot
                    } else {
                        Marker::Braille
                    })
                    .graph_type(GraphType::Line)
                    .style(ratatui::style::Style::default().fg(write_color))
                    .data(&write_points),
            );
        }
        if !verify_points.is_empty() {
            datasets.push(
                Dataset::default()
                    .name("read+verify")
                    .marker(if self.ascii {
                        Marker::Dot
                    } else {
                        Marker::Braille
                    })
                    .graph_type(GraphType::Line)
                    .style(ratatui::style::Style::default().fg(verify_color))
                    .data(&verify_points),
            );
        }
        if baseline_points.len() > 1 {
            datasets.push(
                Dataset::default()
                    .name("baseline")
                    .marker(Marker::Dot)
                    .graph_type(GraphType::Line)
                    .style(
                        ratatui::style::Style::default().fg(self
                            .theme
                            .dim()
                            .fg
                            .unwrap_or(ratatui::style::Color::Gray)),
                    )
                    .data(&baseline_points),
            );
        }
        if threshold_points.len() > 1 {
            datasets.push(
                Dataset::default()
                    .name("threshold")
                    .marker(Marker::Dot)
                    .graph_type(GraphType::Line)
                    .style(
                        ratatui::style::Style::default().fg(self
                            .theme
                            .warning()
                            .fg
                            .unwrap_or(ratatui::style::Color::Yellow)),
                    )
                    .data(&threshold_points),
            );
        }
        let event_points: Vec<(f64, f64)> = results
            .analysis
            .events
            .iter()
            .map(|event| {
                let x = match results.axis {
                    Axis::Position => {
                        (pass_base.get(&event.pass).copied().unwrap_or(0) + event.start_position)
                            as f64
                    }
                    Axis::Time => event.start_ms as f64,
                };
                (x, event.min_mibps)
            })
            .collect();
        if !event_points.is_empty() {
            datasets.push(
                Dataset::default()
                    .name("event")
                    .marker(Marker::Dot)
                    .graph_type(GraphType::Scatter)
                    .style(
                        ratatui::style::Style::default().fg(self
                            .theme
                            .danger()
                            .fg
                            .unwrap_or(ratatui::style::Color::Red)),
                    )
                    .data(&event_points),
            );
        }
        let x_mid = max_x / 2.0;
        let x_end = match results.axis {
            Axis::Position => format_axis_bytes(max_x as u64),
            Axis::Time => format_seconds((max_x / 1000.0) as u64),
        };
        let x_mid_label = match results.axis {
            Axis::Position => format_axis_bytes(x_mid as u64),
            Axis::Time => format_seconds((x_mid / 1000.0) as u64),
        };
        let chart = Chart::new(datasets)
            .block(
                Block::bordered()
                    .title(format!(
                        " Speed: {} (MiB/s) ",
                        match results.axis {
                            Axis::Position => "write position",
                            Axis::Time => "time",
                        }
                    ))
                    .border_style(self.theme.border()),
            )
            .x_axis(ChartAxis::default().bounds([0.0, max_x]).labels([
                "0".to_string(),
                x_mid_label,
                x_end,
            ]))
            .y_axis(ChartAxis::default().bounds([0.0, max_y * 1.1]).labels([
                "0".to_string(),
                format!("{:.0}", max_y / 2.0),
                format!("{:.0}", max_y),
            ]));
        frame.render_widget(chart, area);
    }
}
