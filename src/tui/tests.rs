//! TUI unit tests.

use super::*;

#[test]
fn tab_activity_glyphs_are_mode_specific() {
    let app = App::new(ColorMode::Auto);
    let quality = app.tab_activity_glyph(TestKind::Quality);
    assert!(matches!(quality, "◜" | "◝" | "◞" | "◟"));
    let speed = app.tab_activity_glyph(TestKind::Speed);
    assert_eq!(speed.chars().count(), 3);
    assert!(speed.chars().all(|ch| "▁▃▅▇".contains(ch)));
}

#[test]
fn home_shows_mode_badge_and_switch_label() {
    let app = App::new(ColorMode::Auto);
    let text = normalized(&render_text(&app));
    assert!(text.contains("SC/QC模式切换"));
    assert!(text.contains("[QC]质量测试"));
}

#[test]
fn cdm_speed_settings_support_multi_select() {
    let mut app = App::new(ColorMode::Auto);
    app.screen = Screen::SpeedSettings;
    app.speed.focus = 0;
    app.handle_speed_settings(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
    assert_eq!(app.selected_speed_presets().len(), 0);
    app.handle_speed_settings(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_speed_settings(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let selected = app.selected_speed_presets();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, "SEQ1M_Q1T1");
    for _ in 0..3 {
        app.handle_speed_settings(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    assert_eq!(app.speed.focus, 4);
    app.handle_speed_settings(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.handle_speed_settings(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.speed.values.file_size, 1024 * 1024 * 1024);
}

fn app_with_finished_speed_suite() -> App {
    let mut app = app_with_multi(1);
    app.multi.as_mut().unwrap().kind = TestKind::Speed;
    let suite = app.multi.as_mut().unwrap().tasks[0]
        .suite
        .get_or_insert_with(SuiteView::default);
    suite.write_curves[0] = vec![(0, 100.0), (512 * 1024 * 1024, 200.0)];
    suite.read_curves[0] = vec![(0, 90.0), (512 * 1024 * 1024, 180.0)];
    suite.bar_values = [Some(300.0), Some(200.0), Some(100.0), Some(50.0)];
    suite.results = vec![
        PresetResult {
            preset_index: 0,
            ..PresetResult::default()
        },
        PresetResult {
            preset_index: 1,
            ..PresetResult::default()
        },
        PresetResult {
            preset_index: 2,
            ..PresetResult::default()
        },
        PresetResult {
            preset_index: 3,
            ..PresetResult::default()
        },
    ];
    app.screen = Screen::MultiResults;
    app
}

#[test]
fn results_screen_combines_line_and_bar_charts() {
    let app = app_with_finished_speed_suite();
    let text = normalized(&render_text(&app));
    assert!(text.contains("CDM速度测试结果"));
    assert!(text.contains("速度MiB/s"));
    assert!(text.contains("4Ktrimmed-meanMiB/s"));
    assert!(text.contains("Q32W"));
}

#[test]
fn random_run_uses_a_line_chart() {
    let mut app = app_with_multi(1);
    app.multi.as_mut().unwrap().kind = TestKind::Speed;
    let suite = app.multi.as_mut().unwrap().tasks[0]
        .suite
        .get_or_insert_with(SuiteView::default);
    suite.current = 2;
    suite.phase = Some(SuitePhase::Verify);
    suite.mibps = 123.0;
    suite.read_curves[2] = vec![(0, 100.0), (4096, 123.0)];
    app.screen = Screen::MultiRun;
    let text = normalized(&render_text(&app));
    assert!(text.contains("速度MiB/s"));
    assert!(!text.contains("currentthroughput"));
}

#[test]
fn system_confirmation_requires_typed_yes() {
    let mut app = App::new(ColorMode::Auto);
    app.screen = Screen::SystemConfirm;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.screen, Screen::SystemConfirm);
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert_eq!(app.system_confirm_input, "Y");
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.screen, Screen::Confirm);
}

#[test]
fn trimmed_mean_drops_extremes() {
    assert_eq!(trimmed_mean(&[1.0, 2.0, 3.0, 4.0, 5.0]), 3.0);
    assert_eq!(trimmed_mean(&[2.0, 4.0]), 3.0);
    assert_eq!(trimmed_mean(&[]), 0.0);
}

#[test]
fn cdm_presets_and_axis_units_are_stable() {
    assert_eq!(PRESETS.len(), 4);
    assert_eq!(PRESETS[0].name, "SEQ1M Q8T1");
    assert_eq!(PRESETS[2].name, "RND4K Q32T1");
    assert_eq!(format_axis_bytes(1024 * 1024), "1.0M");
    assert_eq!(format_axis_bytes(1024 * 1024 * 1024), "1.0G");
}

#[test]
fn verify_existing_forces_single_pass_and_no_continue() {
    let mut settings = Settings {
        mode: RunMode::VerifyExisting,
        passes: 5,
        continue_on_error: true,
        ..Settings::default()
    };
    settings.normalize();
    assert_eq!(settings.passes, 1);
    assert!(!settings.continue_on_error);
    assert!(!settings.passes_editable());
}

fn test_multi_task() -> MultiTask {
    MultiTask {
        label: "X:".to_string(),
        target_path: PathBuf::from("X:\\"),
        mode: RunMode::WriteOnly,
        control: TaskControl::new(CancelToken::new(), PauseToken::new()),
        latest: None,
        phase: None,
        total: 0,
        log: VecDeque::new(),
        status: TaskStatus::Running,
        summary: None,
        speed_history: Vec::new(),
        suite: None,
        error: None,
        health: DriveHealth::default(),
        cancel_requested: false,
    }
}

fn app_with_multi(count: usize) -> App {
    let mut app = App::new(ColorMode::Auto);
    let (_tx, rx) = mpsc::channel();
    let tasks = (0..count).map(|_| test_multi_task()).collect();
    app.multi = Some(MultiRun {
        kind: TestKind::Quality,
        tasks,
        rx,
        active: 0,
        started: Instant::now(),
        cancel_menu: false,
        export_message: String::new(),
        auto_export: false,
        exported: false,
        notified: false,
        results_scroll: 0,
    });
    app.screen = Screen::MultiRun;
    app
}

#[test]
fn cancel_shortcuts_open_the_prompt() {
    for code in [KeyCode::Esc, KeyCode::Char('c')] {
        let mut app = app_with_multi(2);
        let modifiers = if code == KeyCode::Char('c') {
            KeyModifiers::CONTROL
        } else {
            KeyModifiers::NONE
        };
        app.handle_key(KeyEvent::new(code, modifiers));
        assert!(app.multi.as_ref().unwrap().cancel_menu);
    }
}

#[test]
fn s_shortcut_does_nothing() {
    let mut app = app_with_multi(2);
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE));
    let multi = app.multi.as_ref().unwrap();
    assert!(!multi.cancel_menu);
    assert!(!multi.tasks.iter().any(|task| task.cancel_requested));
}

#[test]
fn enter_in_prompt_cancels_only_the_active_drive() {
    let mut app = app_with_multi(2);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let multi = app.multi.as_ref().unwrap();
    assert!(!multi.cancel_menu);
    assert!(multi.tasks[0].cancel_requested);
    assert!(!multi.tasks[1].cancel_requested);
}

#[test]
fn prompt_still_offers_stop_all_for_multiple_drives() {
    for code in [KeyCode::Char('2'), KeyCode::Char('S')] {
        let mut app = app_with_multi(2);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        let multi = app.multi.as_ref().unwrap();
        assert!(multi.tasks.iter().all(|task| task.cancel_requested));
    }
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn render_text(app: &App) -> String {
    let backend = ratatui::backend::TestBackend::new(120, 30);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("draw");
    let buffer = terminal.backend().buffer();
    let mut output = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            output.push_str(buffer[(x, y)].symbol());
        }
        output.push('\n');
    }
    output
}

#[test]
fn small_terminal_shows_size_guard() {
    let app = App::new(ColorMode::Auto);
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("draw");
    let buffer = terminal.backend().buffer();
    let mut output = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            output.push_str(buffer[(x, y)].symbol());
        }
    }
    let text = normalized(&output);
    assert!(text.contains("终端窗口过小"));
    assert!(text.contains("100x26"));
}

#[test]
fn speed_run_shows_total_progress_and_selected_ordinal() {
    let mut app = app_with_multi(1);
    app.multi.as_mut().unwrap().kind = TestKind::Speed;
    let suite = app.multi.as_mut().unwrap().tasks[0]
        .suite
        .get_or_insert_with(SuiteView::default);
    suite.current = 0;
    suite.phase = Some(SuitePhase::Write);
    suite.done_bytes = 512 * 1024 * 1024;
    suite.total_bytes = 1024 * 1024 * 1024;
    app.screen = Screen::MultiRun;
    let text = normalized(&render_text(&app));
    assert!(text.contains("当前1/4"));
    assert!(text.contains("总进度"));
}

#[test]
fn single_drive_prompt_hides_stop_all() {
    let mut app = app_with_multi(1);
    app.multi.as_mut().unwrap().cancel_menu = true;
    let text = normalized(&render_text(&app));
    assert!(text.contains("Enter停止当前盘"));
    assert!(text.contains("Esc返回"));
    assert!(!text.contains("1/"));
    assert!(!text.contains("2停止全部"));
    assert!(!text.contains("3/"));
}

#[test]
fn multiple_drive_prompt_shows_stop_all() {
    let mut app = app_with_multi(2);
    app.multi.as_mut().unwrap().cancel_menu = true;
    let text = normalized(&render_text(&app));
    assert!(text.contains("1/Enter停止当前盘"));
    assert!(text.contains("2/S停止全部"));
    assert!(text.contains("3/Esc返回"));
}

#[test]
fn continue_requires_verify_and_multiple_passes() {
    let mut settings = Settings {
        mode: RunMode::WriteVerify,
        passes: 1,
        continue_on_error: true,
        ..Settings::default()
    };
    settings.normalize();
    assert!(!settings.continue_on_error);

    settings.passes = 2;
    settings.continue_on_error = true;
    settings.normalize();
    assert!(settings.continue_on_error);
    assert!(settings.continue_editable());
}
