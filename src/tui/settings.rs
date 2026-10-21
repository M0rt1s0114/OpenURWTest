//! Quality test settings.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::{App, MouseAction, RunMode, SettingsField, centered_rect, yes_no};

impl App {
    pub(super) fn render_settings(&self, frame: &mut Frame) {
        let area = centered_rect(frame.area(), 64, 60);
        frame.render_widget(Clear, area);
        let settings = &self.settings;
        let values = &settings.values;
        let focused = |field: SettingsField| settings.field == field;
        let mut lines: Vec<Line> = Vec::new();

        let mode_text = match values.mode {
            RunMode::WriteVerify => "写入并校验",
            RunMode::WriteOnly => "仅写入",
            RunMode::VerifyExisting => "仅校验已有数据",
        };
        lines.push(self.settings_line(
            "测试模式",
            format!("◀ {mode_text} ▶"),
            focused(SettingsField::Mode),
        ));
        let passes_text = if values.passes_editable() {
            if focused(SettingsField::Passes) {
                format!("[ {} ]", settings.passes_input)
            } else {
                format!("[ {} ]", values.passes)
            }
        } else {
            "1 (仅校验固定为 1 圈)".to_string()
        };
        lines.push(self.settings_line("圈数", passes_text, focused(SettingsField::Passes)));
        let continue_text = if values.continue_editable() {
            yes_no(values.continue_on_error).to_string()
        } else {
            "否 (需校验且圈数 > 1)".to_string()
        };
        lines.push(self.settings_line(
            "出错后继续下一圈",
            continue_text.to_string(),
            focused(SettingsField::Continue),
        ));
        lines.push(self.settings_line(
            "成功后删除测试文件",
            yes_no(values.delete_after).to_string(),
            focused(SettingsField::Delete),
        ));
        lines.push(self.settings_line(
            "结束后自动导出报告",
            yes_no(values.auto_export).to_string(),
            focused(SettingsField::Export),
        ));
        lines.push(Line::from(""));
        let block = Block::bordered()
            .title(format!(
                " 测试设置（{} 个盘） ",
                self.target_volumes().len()
            ))
            .border_style(self.theme.border());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), chunks[0]);
        let fields = [
            SettingsField::Mode,
            SettingsField::Passes,
            SettingsField::Continue,
            SettingsField::Delete,
            SettingsField::Export,
        ];
        for (index, field) in fields.iter().enumerate() {
            self.record_hotspot(
                Rect::new(
                    area.x + 1,
                    area.y + 1 + index as u16,
                    area.width.saturating_sub(2),
                    1,
                ),
                MouseAction::FocusSettings(*field),
            );
        }
        self.render_button_row(frame, chunks[1], "Enter 确定      Esc 取消");
    }

    pub(super) fn settings_line(&self, label: &str, value: String, focused: bool) -> Line<'static> {
        let label_style = if focused {
            self.theme.key()
        } else {
            self.theme.dim()
        };
        let marker = if focused { "▶ " } else { "  " };
        let value_style = if focused {
            self.theme.selected()
        } else {
            self.theme.text()
        };
        Line::from(vec![
            Span::styled(format!("{marker}{label:<18}"), label_style),
            Span::styled(value, value_style),
        ])
    }
}
