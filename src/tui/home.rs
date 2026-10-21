//! Home screen.

use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};

use crate::console::format_bytes;
use crate::types::TestKind;

use super::{APP_NAME, App, BANNER, MouseAction, VERSION};

impl App {
    pub(super) fn render_home(&self, frame: &mut Frame) {
        let area = frame.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Min(10),
                Constraint::Length(3),
            ])
            .split(area);

        let banner = if area.width >= 70 {
            format!("{BANNER}\n\n{APP_NAME}   v{VERSION}")
        } else {
            format!("OpenURWTest   v{VERSION}")
        };
        frame.render_widget(
            Paragraph::new(banner)
                .style(self.theme.banner())
                .alignment(ratatui::layout::Alignment::Center),
            layout[0],
        );

        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(52), Constraint::Percentage(48)])
            .split(layout[1]);

        let items: Vec<ListItem> = self
            .volumes
            .iter()
            .map(|volume| {
                let removable = if volume.removable { "*" } else { " " };
                let mark = if self.checked.contains(&volume.mount_point) {
                    "[x]"
                } else {
                    "[ ]"
                };
                ListItem::new(Line::from(format!(
                    "{mark} {removable} {:<5} {:>10}  {}",
                    volume.mount_point.display(),
                    format_bytes(volume.free_bytes),
                    volume.device_name()
                )))
                .style(
                    if self.mouse_hover.is_some_and(|rect| {
                        rect.y
                            == columns[0].y
                                + 1
                                + self
                                    .volumes
                                    .iter()
                                    .position(|item| item.mount_point == volume.mount_point)
                                    .unwrap_or(0) as u16
                    }) {
                        self.theme.key()
                    } else {
                        ratatui::style::Style::default()
                    },
                )
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::bordered()
                    .title(" 卷列表 (Up/Down) ")
                    .border_style(self.theme.border()),
            )
            .highlight_symbol("> ")
            .highlight_style(self.theme.selected());
        let mut state = ListState::default();
        if !self.volumes.is_empty() {
            state.select(Some(self.selected));
        }
        frame.render_stateful_widget(list, columns[0], &mut state);
        let row_x = columns[0].x + 1;
        let text_x = row_x + 2;
        for index in 0..self.volumes.len() {
            let y = columns[0].y + 1 + index as u16;
            if y + 1 >= columns[0].y + columns[0].height {
                break;
            }
            self.record_hotspot(
                Rect::new(row_x, y, columns[0].width.saturating_sub(2), 1),
                MouseAction::Select(index),
            );
            self.record_hotspot(Rect::new(text_x, y, 3, 1), MouseAction::Toggle(index));
        }

        let details = match self.selected_volume() {
            Some(volume) => vec![
                Line::from(format!("Volume:    {}", volume.display_name())),
                Line::from(format!("Model:     {}", volume.device_name())),
                Line::from(format!(
                    "Type:      {}{}",
                    volume.kind.label(),
                    if volume.removable { " (removable)" } else { "" }
                )),
                Line::from(format!("Filesystem: {}", volume.fs)),
                Line::from(format!(
                    "Capacity:  {} total",
                    format_bytes(volume.total_bytes)
                )),
                Line::from(format!("Free:      {}", format_bytes(volume.free_bytes))),
                {
                    let health = self.cached_health(&volume.mount_point);
                    Line::from(format!(
                        "Temp:      {}   Thermal: {}",
                        health
                            .temperature_c
                            .map(|value| format!("{value} C"))
                            .unwrap_or_else(|| "unknown".to_string()),
                        health.status.label()
                    ))
                },
                Line::from(""),
                Line::from(format!("已选:       {} 个盘", self.checked.len())),
                Line::from(format!(
                    "模式:      [{}] {}",
                    if self.kind == TestKind::Quality {
                        "QC"
                    } else {
                        "SC"
                    },
                    self.kind.label()
                )),
                Line::from(""),
                Line::from(self.message.clone()),
            ],
            None => vec![Line::from("未找到可测试的卷")],
        };
        frame.render_widget(
            Paragraph::new(details)
                .block(
                    Block::bordered()
                        .title(" 详情 ")
                        .border_style(self.theme.border()),
                )
                .wrap(Wrap { trim: true }),
            columns[1],
        );

        let hints = Paragraph::new(format!(
            "Space 选择  a 全选  n 清空  m SC/QC 模式切换  Enter 设置  r 刷新  F2 鼠标:{}  q 退出",
            if self.mouse_enabled { "开" } else { "关" }
        ))
        .style(self.theme.dim());
        let targets = self.target_volumes();
        let target_list = targets
            .iter()
            .map(|volume| volume.mount_point.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let summary = format!(
            "当前: {}   已选: {} 个盘   目标: {} 个盘 [{}]",
            self.selected_volume()
                .map(|volume| volume.display_name())
                .unwrap_or_else(|| "-".to_string()),
            self.checked.len(),
            targets.len(),
            target_list
        );
        let (mode_code, mode_style) = if self.kind == TestKind::Quality {
            ("QC", self.theme.success())
        } else {
            ("SC", self.theme.key())
        };
        let mode_badge = Span::styled(format!("[{mode_code}] {}", self.kind.label()), mode_style);
        let footer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(layout[2]);
        let buttons = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(18),
                Constraint::Length(12),
                Constraint::Length(8),
                Constraint::Length(8),
                Constraint::Min(0),
            ])
            .split(footer[0]);
        let button_specs = [
            (
                buttons[0],
                " SC/QC 模式切换 ".to_string(),
                KeyCode::Char('m'),
            ),
            (buttons[1], " 测试设置 ".to_string(), KeyCode::Char('w')),
            (buttons[2], " 开始 ".to_string(), KeyCode::Char('w')),
            (buttons[3], " 退出 ".to_string(), KeyCode::Char('q')),
        ];
        for (rect, label, code) in button_specs {
            let style = if self.mouse_hover == Some(rect) {
                self.theme.selected()
            } else {
                self.theme.key()
            };
            frame.render_widget(
                Paragraph::new(label)
                    .style(style)
                    .alignment(ratatui::layout::Alignment::Center),
                rect,
            );
            self.record_hotspot(rect, MouseAction::Key(code));
        }
        frame.render_widget(hints, footer[1]);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                mode_badge,
                Span::raw(format!("   {summary}")),
            ]))
            .style(self.theme.text()),
            footer[2],
        );
    }
}
