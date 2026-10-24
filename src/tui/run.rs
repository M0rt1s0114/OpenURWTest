//! run.

use super::*;

impl App {
    pub(super) fn render_history_chart(
        &self,
        frame: &mut Frame,
        area: Rect,
        history: &[(u64, f64)],
        x_max: u64,
        style: ratatui::style::Style,
    ) {
        let y_max = history
            .iter()
            .map(|(_, speed)| *speed)
            .fold(0.0f64, f64::max)
            .max(1.0)
            * 1.1;
        let x_max = x_max.max(1);
        let points: Vec<(f64, f64)> = history
            .iter()
            .map(|(position, speed)| (*position as f64, *speed))
            .collect();
        let dataset = Dataset::default()
            .name("speed")
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(style)
            .data(&points);
        let chart = Chart::new(vec![dataset])
            .block(
                Block::bordered()
                    .title(" speed MiB/s ")
                    .border_style(self.theme.border()),
            )
            .x_axis(ChartAxis::default().bounds([0.0, x_max as f64]).labels([
                "0".to_string(),
                format_axis_bytes(x_max / 2),
                format_axis_bytes(x_max),
            ]))
            .y_axis(ChartAxis::default().bounds([0.0, y_max]).labels([
                "0".to_string(),
                format!("{:.0}", y_max / 2.0),
                format!("{:.0}", y_max),
            ]));
        frame.render_widget(chart, area);
    }

    pub(super) fn render_run(&self, frame: &mut Frame) {
        let Some(run) = &self.run else {
            return;
        };
        let area = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(4),
                Constraint::Min(8),
                Constraint::Length(2),
            ])
            .split(area);

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    " {}  [{}]  圈数: {}",
                    run.target,
                    match run.mode {
                        RunMode::WriteVerify => "写入并校验",
                        RunMode::WriteOnly => "仅写入",
                        RunMode::VerifyExisting => "仅校验已有数据",
                    },
                    run.passes
                )),
                Line::from(format!(
                    " 温度 {}   热状态 {}",
                    run.view
                        .health
                        .temperature_c
                        .map(|value| format!("{value} C"))
                        .unwrap_or_else(|| "unknown".to_string()),
                    run.view.health.status.label()
                )),
            ])
            .block(Block::bordered().border_style(self.theme.border())),
            layout[0],
        );

        if let Some(progress) = run.view.latest {
            frame.render_widget(
                Paragraph::new(format!(
                    "当前 {:.1} MiB/s   平均 {:.1} MiB/s   剩余 {}   ETA {}",
                    progress.current_mibps,
                    progress.average_mibps,
                    format_bytes(progress.remaining_bytes),
                    format_seconds(progress.eta_secs)
                ))
                .style(self.theme.text()),
                layout[1],
            );
        }

        let (bytes, total) = run
            .view
            .latest
            .map(|progress| (progress.bytes, progress.total_bytes))
            .unwrap_or((0, run.view.total.max(1)));
        let ratio = if total > 0 {
            (bytes as f64 / total as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let paused = run.task_paused();
        let gauge = Gauge::default()
            .block(
                Block::bordered()
                    .title(if paused {
                        " 已暂停 ".to_string()
                    } else {
                        format!(
                            " {} ",
                            run.view.phase.map(Phase::label).unwrap_or("starting")
                        )
                    })
                    .border_style(if paused {
                        self.theme.warning()
                    } else {
                        self.theme.border()
                    }),
            )
            .gauge_style(if paused {
                self.theme.warning()
            } else if run.view.phase == Some(Phase::Verify) {
                self.theme.gauge_verify()
            } else {
                self.theme.gauge_write()
            })
            .ratio(ratio)
            .label(if paused {
                format!("已暂停  {} / {}", format_bytes(bytes), format_bytes(total))
            } else {
                format!("{} / {}", format_bytes(bytes), format_bytes(total))
            });
        frame.render_widget(gauge, layout[2]);

        let log_lines: Vec<Line> = run
            .view
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
            &run.view.speed_history,
            run.view.total,
            if run.view.phase == Some(Phase::Verify) {
                self.theme.gauge_verify()
            } else {
                self.theme.gauge_write()
            },
        );

        let footer = if run.control.pause.is_paused() {
            format!(
                "已暂停 - 按 p 继续   已用 {}",
                format_seconds(run.started.elapsed().as_secs())
            )
        } else if run.cancel_requested {
            format!(
                "正在取消 - 数据会保留在目标盘   已用 {}",
                format_seconds(run.started.elapsed().as_secs())
            )
        } else {
            format!(
                "{} Esc / Ctrl+C 取消（保留测试数据）   已用 {}",
                self.spinner_glyph(),
                format_seconds(run.started.elapsed().as_secs())
            )
        };
        frame.render_widget(Paragraph::new(footer).style(self.theme.dim()), layout[5]);

        if self.cancel_pending {
            let popup = centered_rect(frame.area(), 52, 30);
            frame.render_widget(Clear, popup);
            let lines = vec![
                Line::styled("取消这次测试？", self.theme.warning()),
                Line::from(""),
                Line::from("已写入的数据会保留在目标盘上。"),
                Line::from(""),
                Line::styled("Enter / y 取消测试      Esc / n 继续测试", self.theme.dim()),
            ];
            frame.render_widget(
                Paragraph::new(lines)
                    .block(
                        Block::bordered()
                            .title(" 确认取消 ")
                            .border_style(self.theme.warning()),
                    )
                    .wrap(Wrap { trim: true }),
                popup,
            );
        }
    }
}
