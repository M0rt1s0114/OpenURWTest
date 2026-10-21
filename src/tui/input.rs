//! Keyboard, mouse, and setting input handling.

use super::*;

impl App {
    pub(super) fn mouse_event(&mut self, event: MouseEvent) -> bool {
        let position = ratatui::layout::Position::new(event.column, event.row);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let action = self
                    .hotspots
                    .borrow()
                    .iter()
                    .rev()
                    .find(|hotspot| hotspot.rect.contains(position))
                    .map(|hotspot| hotspot.action);
                match action {
                    Some(MouseAction::Select(index)) => {
                        self.selected = index;
                        self.message.clear();
                    }
                    Some(MouseAction::Toggle(index)) => {
                        self.selected = index;
                        self.toggle_check();
                        self.message.clear();
                    }
                    Some(MouseAction::Key(code)) => {
                        return self.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
                    }
                    Some(MouseAction::FocusSettings(field)) => {
                        self.settings.field = field;
                    }
                    Some(MouseAction::FocusSpeed(field)) => {
                        self.speed.focus = field;
                    }
                    None => {}
                }
            }
            MouseEventKind::Moved => {
                self.mouse_hover = self
                    .hotspots
                    .borrow()
                    .iter()
                    .rev()
                    .find(|hotspot| hotspot.rect.contains(position))
                    .map(|hotspot| hotspot.rect);
            }
            MouseEventKind::ScrollUp => self.wheel(true),
            MouseEventKind::ScrollDown => self.wheel(false),
            _ => {}
        }
        false
    }

    pub(super) fn record_hotspot(&self, rect: Rect, action: MouseAction) {
        self.hotspots.borrow_mut().push(Hotspot { rect, action });
    }

    pub(super) fn record_multi_hotspots(&self, tabs: Rect, footer: Rect, count: usize) {
        let inner = Rect::new(tabs.x + 1, tabs.y + 1, tabs.width.saturating_sub(2), 1);
        if count > 0 {
            let width = inner.width / count as u16;
            for index in 0..count.min(9) {
                let rect = Rect::new(
                    inner.x + index as u16 * width,
                    inner.y,
                    if index + 1 == count {
                        inner.width - index as u16 * width
                    } else {
                        width
                    },
                    1,
                );
                self.record_hotspot(
                    rect,
                    MouseAction::Key(KeyCode::Char((b'1' + index as u8) as char)),
                );
            }
        }
        self.record_hotspot(
            Rect::new(footer.x, footer.y, footer.width, 1),
            MouseAction::Key(KeyCode::Esc),
        );
    }

    pub(super) fn wheel(&mut self, up: bool) {
        match self.screen {
            Screen::Home => {
                if up {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                } else if self.selected + 1 < self.volumes.len() {
                    self.selected += 1;
                }
            }
            Screen::Settings | Screen::SpeedSettings => {
                let code = if up { KeyCode::Up } else { KeyCode::Down };
                self.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
            }
            Screen::MultiRun => {
                if let Some(multi) = self.multi.as_mut()
                    && !multi.tasks.is_empty()
                {
                    if up {
                        multi.active = multi.active.checked_sub(1).unwrap_or(multi.tasks.len() - 1);
                    } else {
                        multi.active = (multi.active + 1) % multi.tasks.len();
                    }
                }
            }
            Screen::MultiResults => {
                if let Some(multi) = self.multi.as_mut() {
                    if up {
                        multi.results_scroll = multi.results_scroll.saturating_sub(1);
                    } else {
                        multi.results_scroll = multi.results_scroll.saturating_add(1);
                    }
                }
            }
            Screen::Results => {}
            _ => {}
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> bool {
        let code = key.code;
        if code == KeyCode::F(2) {
            self.mouse_enabled = !self.mouse_enabled;
            return false;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self.screen {
            Screen::Home => match code {
                KeyCode::Char('c') if ctrl => return true,
                KeyCode::Char('q') | KeyCode::Esc => return true,
                KeyCode::Up | KeyCode::Char('k') => {
                    if self.selected > 0 {
                        self.selected -= 1;
                        self.message.clear();
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.selected + 1 < self.volumes.len() {
                        self.selected += 1;
                        self.message.clear();
                    }
                }
                KeyCode::Char('r') => {
                    self.rescan();
                    self.message = "卷列表已刷新".to_string();
                }
                KeyCode::Char(' ') => self.toggle_check(),
                KeyCode::Char('a') => self.select_all(),
                KeyCode::Char('n') => self.clear_checks(),
                KeyCode::Char('m') => {
                    self.kind = match self.kind {
                        TestKind::Quality => TestKind::Speed,
                        TestKind::Speed => TestKind::Quality,
                    };
                }
                KeyCode::Char('w') | KeyCode::Enter => {
                    if self.kind == TestKind::Speed {
                        self.speed.focus = 0;
                        self.screen = Screen::SpeedSettings;
                    } else {
                        self.open_settings(RunMode::WriteVerify);
                    }
                }
                KeyCode::Char('o') => self.open_settings(RunMode::WriteOnly),
                KeyCode::Char('v') => self.open_settings(RunMode::VerifyExisting),
                _ => {}
            },
            Screen::Settings => match code {
                KeyCode::Esc => {
                    self.screen = Screen::Home;
                    self.rescan();
                }
                KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
                    self.focus_field(self.settings.field.previous());
                }
                KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
                    self.focus_field(self.settings.field.next());
                }
                KeyCode::Left | KeyCode::Char('h') => self.settings_toggle(false),
                KeyCode::Right | KeyCode::Char('l') => self.settings_toggle(true),
                KeyCode::Backspace if self.settings.field == SettingsField::Passes => {
                    self.settings.passes_input.pop();
                }
                KeyCode::Char(digit)
                    if self.settings.field == SettingsField::Passes && digit.is_ascii_digit() =>
                {
                    if self.settings.passes_input.len() < 2 {
                        self.settings.passes_input.push(digit);
                    }
                }
                KeyCode::Enter => self.settings_accept(),
                _ => {}
            },
            Screen::Confirm => match code {
                KeyCode::Enter | KeyCode::Char('y') => {
                    if self.needs_system_confirmation() {
                        self.system_confirm_input.clear();
                        self.screen = Screen::SystemConfirm;
                    } else {
                        self.launch_selected(self.settings.values);
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') => {
                    self.screen = Screen::Settings;
                }
                _ => {}
            },
            Screen::SystemConfirm => match code {
                KeyCode::Enter => {
                    if self.system_confirm_input.eq_ignore_ascii_case("YES") {
                        self.launch_selected(self.settings.values);
                    } else {
                        self.message = "请输入 YES 后再确认".to_string();
                    }
                }
                KeyCode::Esc => {
                    self.system_confirm_input.clear();
                    self.screen = Screen::Confirm;
                }
                KeyCode::Backspace => {
                    self.system_confirm_input.pop();
                }
                KeyCode::Char(ch) if ch.is_ascii_alphabetic() => {
                    self.system_confirm_input.push(ch.to_ascii_uppercase());
                }
                _ => {}
            },
            Screen::Run => {
                if self.cancel_pending {
                    match code {
                        KeyCode::Enter | KeyCode::Char('y') => {
                            self.cancel_pending = false;
                            if let Some(run) = self.run.as_mut() {
                                run.cancel_requested = true;
                                if let Some(token) = &run.token {
                                    token.cancel();
                                }
                            }
                            cancel::request();
                        }
                        KeyCode::Esc | KeyCode::Char('n') => {
                            self.cancel_pending = false;
                        }
                        _ => {}
                    }
                } else if code == KeyCode::Esc || (ctrl && code == KeyCode::Char('c')) {
                    self.cancel_pending = true;
                }
            }
            Screen::SpeedSettings => self.handle_speed_settings(key),
            Screen::SpeedConfirm => self.handle_speed_confirm(key),
            Screen::MultiRun => self.handle_multi_key(key),
            Screen::MultiResults => match code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.screen = Screen::Home;
                    self.multi = None;
                }
                KeyCode::Char('e') => self.export_multi_report(),
                KeyCode::Up => {
                    if let Some(multi) = self.multi.as_mut() {
                        multi.results_scroll = multi.results_scroll.saturating_sub(1);
                    }
                }
                KeyCode::Down => {
                    if let Some(multi) = self.multi.as_mut() {
                        multi.results_scroll = multi.results_scroll.saturating_add(1);
                    }
                }
                KeyCode::PageUp => {
                    if let Some(multi) = self.multi.as_mut() {
                        multi.results_scroll = multi.results_scroll.saturating_sub(10);
                    }
                }
                KeyCode::PageDown => {
                    if let Some(multi) = self.multi.as_mut() {
                        multi.results_scroll = multi.results_scroll.saturating_add(10);
                    }
                }
                KeyCode::Tab | KeyCode::Right => {
                    if let Some(multi) = self.multi.as_mut() {
                        multi.active = (multi.active + 1) % multi.tasks.len().max(1);
                    }
                }
                KeyCode::BackTab | KeyCode::Left => {
                    if let Some(multi) = self.multi.as_mut() {
                        multi.active = multi
                            .active
                            .checked_sub(1)
                            .unwrap_or_else(|| multi.tasks.len().saturating_sub(1));
                    }
                }
                _ => {}
            },
            Screen::Results => {
                let mut refresh = false;
                {
                    let Some(results) = self.results.as_mut() else {
                        return false;
                    };
                    match code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            self.screen = Screen::Home;
                            self.results = None;
                            self.rescan();
                        }
                        KeyCode::Char('x') => results.axis = results.axis.toggle(),
                        KeyCode::Char('t') => {
                            results.params.mode = match results.params.mode {
                                ThresholdMode::Percent => ThresholdMode::Absolute,
                                ThresholdMode::Absolute => ThresholdMode::Percent,
                            };
                            refresh = true;
                        }
                        KeyCode::Char('[') => {
                            match results.params.mode {
                                ThresholdMode::Percent => {
                                    results.params.percent =
                                        (results.params.percent - 5.0).max(10.0)
                                }
                                ThresholdMode::Absolute => {
                                    results.params.absolute_mibps =
                                        (results.params.absolute_mibps - 10.0).max(0.0)
                                }
                            }
                            refresh = true;
                        }
                        KeyCode::Char(']') => {
                            match results.params.mode {
                                ThresholdMode::Percent => {
                                    results.params.percent =
                                        (results.params.percent + 5.0).min(100.0)
                                }
                                ThresholdMode::Absolute => results.params.absolute_mibps += 10.0,
                            }
                            refresh = true;
                        }
                        KeyCode::Char('e') => {
                            results.export();
                        }
                        _ => {}
                    }
                }
                if refresh {
                    self.refresh_results_async();
                }
            }
        }
        false
    }

    pub(super) fn settings_toggle(&mut self, forward: bool) {
        self.commit_passes();
        match self.settings.field {
            SettingsField::Mode => {
                let modes = [
                    RunMode::WriteVerify,
                    RunMode::WriteOnly,
                    RunMode::VerifyExisting,
                ];
                let index = modes
                    .iter()
                    .position(|mode| *mode == self.settings.values.mode)
                    .unwrap_or(0);
                let next = if forward {
                    (index + 1) % modes.len()
                } else {
                    (index + modes.len() - 1) % modes.len()
                };
                self.settings.values.mode = modes[next];
                self.settings.values.normalize();
                self.settings.passes_input = self.settings.values.passes.to_string();
            }
            SettingsField::Continue => {
                if self.settings.values.continue_editable() {
                    self.settings.values.continue_on_error =
                        !self.settings.values.continue_on_error;
                }
            }
            SettingsField::Delete => {
                self.settings.values.delete_after = !self.settings.values.delete_after;
            }
            SettingsField::Export => {
                self.settings.values.auto_export = !self.settings.values.auto_export;
            }
            SettingsField::ReportTemperature => {
                if self.report_temperature_supported() {
                    self.settings.values.report_temperature =
                        !self.settings.values.report_temperature;
                }
            }
            SettingsField::Passes => {}
        }
    }

    pub(super) fn settings_accept(&mut self) {
        self.commit_passes();
        self.screen = Screen::Confirm;
    }

    pub(super) fn focus_field(&mut self, field: SettingsField) {
        if self.settings.field == SettingsField::Passes {
            self.commit_passes();
        }
        self.settings.field = field;
        if field == SettingsField::Passes {
            self.settings.passes_input.clear();
        }
    }

    pub(super) fn commit_passes(&mut self) {
        if self.settings.values.passes_editable() {
            let parsed = self
                .settings
                .passes_input
                .trim()
                .parse::<u32>()
                .unwrap_or(1);
            self.settings.values.passes = parsed.clamp(1, 99);
        } else {
            self.settings.values.passes = 1;
        }
        self.settings.values.normalize();
        self.settings.passes_input = self.settings.values.passes.to_string();
    }
}
