//! Confirmation dialogs.

use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::{App, MouseAction, RunMode, centered_rect, yes_no};
use crate::console::format_bytes;

impl App {
    pub(super) fn render_speed_confirm(&self, frame: &mut Frame) {
        let area = centered_rect(frame.area(), 68, 56);
        frame.render_widget(Clear, area);
        let targets = self.target_volumes();
        if targets.is_empty() {
            return;
        }
        let values = &self.speed.values;
        let selected = self.selected_speed_presets();
        let mut lines: Vec<Line> = Vec::new();
        if targets
            .iter()
            .any(|volume| self.selected_is_system_volume(volume))
        {
            lines.push(Line::styled(
                "警告：目标包含系统盘，确认后会写入基准测试文件。",
                self.theme.danger(),
            ));
            lines.push(Line::styled(
                "系统盘保护：输入 YES 后按 Enter 开始。",
                self.theme.warning(),
            ));
            lines.push(Line::from(format!(
                "确认输入: {}",
                if self.system_confirm_input.is_empty() {
                    "____"
                } else {
                    self.system_confirm_input.as_str()
                }
            )));
            lines.push(Line::from(""));
        }
        lines.push(Line::from(format!("目标（{} 个盘）:", targets.len())));
        for volume in targets.iter().take(8) {
            lines.push(Line::from(format!(
                "  {}  {}  free {}",
                volume.mount_point.display(),
                volume.device_name(),
                format_bytes(volume.free_bytes)
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!("测试项目（{} 项）:", selected.len())));
        for preset in &selected {
            lines.push(Line::from(format!("  {}", preset.name)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "文件大小: {}",
            format_bytes(values.file_size)
        )));
        lines.push(Line::from(format!(
            "错误中断: {}",
            yes_no(!values.ignore_errors)
        )));
        lines.push(Line::from(""));
        let block = Block::bordered()
            .title(" 确认速度测试 ")
            .border_style(self.theme.border());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), chunks[0]);
        self.render_button_row(frame, chunks[1], "Enter 开始      Esc 返回");
    }

    pub(super) fn render_confirm(&self, frame: &mut Frame) {
        let area = centered_rect(frame.area(), 68, 56);
        frame.render_widget(Clear, area);
        let targets = self.target_volumes();
        if targets.is_empty() {
            return;
        }
        let values = &self.settings.values;
        let mode_text = match values.mode {
            RunMode::WriteVerify => "写入并校验",
            RunMode::WriteOnly => "仅写入",
            RunMode::VerifyExisting => "仅校验已有数据",
        };
        let mut lines: Vec<Line> = Vec::new();
        if values.mode != RunMode::VerifyExisting {
            if targets
                .iter()
                .any(|volume| self.selected_is_system_volume(volume))
            {
                lines.push(Line::styled(
                    "你似乎正在尝试对一个包含 Windows 系统的磁盘进行测试，是否确认继续？",
                    self.theme.danger(),
                ));
                lines.push(Line::from(""));
            }
            let fixed: Vec<String> = targets
                .iter()
                .filter(|volume| !volume.removable && !self.selected_is_system_volume(volume))
                .map(|volume| volume.mount_point.display().to_string())
                .collect();
            if !fixed.is_empty() {
                lines.push(Line::styled(
                    format!("固定磁盘：{}，确认继续吗？", fixed.join("、")),
                    self.theme.warning(),
                ));
                lines.push(Line::from(""));
            }
        }
        lines.push(Line::styled(
            format!("目标（{} 个盘）:", targets.len()),
            self.theme.text(),
        ));
        for volume in targets.iter().take(8) {
            lines.push(Line::from(format!(
                "  {}  {}  free {}",
                volume.mount_point.display(),
                volume.device_name(),
                format_bytes(volume.free_bytes)
            )));
        }
        if targets.len() > 8 {
            lines.push(Line::from(format!("  ... 还有 {} 个", targets.len() - 8)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(format!("模式:     {mode_text}")));
        lines.push(Line::from(format!("圈数:     {}", values.passes)));
        lines.push(Line::from(format!(
            "出错继续: {}",
            yes_no(values.continue_on_error)
        )));
        lines.push(Line::from(format!(
            "成功后删除: {}",
            yes_no(values.delete_after)
        )));
        lines.push(Line::from(format!(
            "自动导出: {}",
            yes_no(values.auto_export)
        )));
        lines.push(Line::from(""));
        let block = Block::bordered()
            .title(format!(" 确认测试（{} 个盘） ", targets.len()))
            .border_style(self.theme.border());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), chunks[0]);
        self.render_button_row(frame, chunks[1], "Enter 立即开始      Esc 返回设置");
    }

    pub(super) fn render_button_row(&self, frame: &mut Frame, area: Rect, hint: &str) {
        frame.render_widget(
            Paragraph::new(Line::styled(hint.to_string(), self.theme.dim()))
                .alignment(ratatui::layout::Alignment::Center),
            area,
        );
        let half = area.width / 2;
        self.record_hotspot(
            Rect::new(area.x, area.y, half, 1),
            MouseAction::Key(KeyCode::Enter),
        );
        self.record_hotspot(
            Rect::new(area.x + half, area.y, area.width - half, 1),
            MouseAction::Key(KeyCode::Esc),
        );
    }

    pub(super) fn render_system_confirm(&self, frame: &mut Frame) {
        let area = centered_rect(frame.area(), 62, 40);
        frame.render_widget(Clear, area);
        let targets = self.target_volumes();
        if targets.is_empty() {
            return;
        }
        let lines = vec![
            Line::styled(
                "为了防止误触，请再次确认要对一个包含 Windows 系统的磁盘进行跑圈测试？",
                self.theme.danger(),
            ),
            Line::from(""),
            Line::from(format!(
                "盘数: {}  系统盘: {}",
                targets.len(),
                targets
                    .iter()
                    .filter(|volume| self.selected_is_system_volume(volume))
                    .map(|volume| volume.mount_point.display().to_string())
                    .collect::<Vec<_>>()
                    .join("、")
            )),
            Line::from(""),
            Line::styled(
                "确认后会写满该磁盘，测试文件默认保留。",
                self.theme.warning(),
            ),
            Line::from(""),
            Line::styled(
                format!(
                    "请输入 YES 确认：{}",
                    if self.system_confirm_input.is_empty() {
                        "____"
                    } else {
                        self.system_confirm_input.as_str()
                    }
                ),
                self.theme.text(),
            ),
        ];
        let block = Block::bordered()
            .title(" 再次确认 ")
            .border_style(self.theme.danger());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), chunks[0]);
        self.render_button_row(frame, chunks[1], "输入 YES 后 Enter 确认      Esc 返回");
    }
}
