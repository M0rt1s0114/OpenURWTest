//! Terminal user interface.

mod animation;
mod confirm;
mod home;
mod input;
mod mouse;
mod results;
mod run;
mod runtime;
mod settings;
mod speed_run;
mod text;

use mouse::{Hotspot, MouseAction, MouseGuard};
use text::*;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Rectangle};
use ratatui::widgets::{
    Axis as ChartAxis, Block, Chart, Clear, Dataset, Gauge, GraphType, Paragraph, Wrap,
};

use crate::analysis::{self, Analysis, AnalysisParams, SlowKind, ThresholdMode};
use crate::cancel::{self, CancelToken};
use crate::console::format_bytes;
use crate::engine::{self, PhaseStats, RunOptions, Summary};
use crate::export::{self, ExportInput, MultiReportRow};
use crate::format;
use crate::health::{self, DriveHealth};
use crate::pause::PauseToken;
use crate::png_speed;
use crate::reporter::{ErrorInfo, ProgressInfo, Reporter};
use crate::speed::suite::{
    PRESETS, PresetResult, SuiteEvent, SuiteOptions, SuitePhase, SuiteResult, run_suite,
};
use crate::theme::Theme;
use crate::trace::Phase;
use crate::types::{ColorMode, MouseMode, TestKind};
use crate::volume::{self, VolumeInfo};

type SpeedSeries = Vec<(String, Vec<(u64, f64)>, ratatui::style::Color)>;

fn completion_notification(title: &str) {
    let _ = title;
    #[cfg(not(test))]
    {
        use std::io::Write;
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x07");
        let _ = out.flush();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::SetTitle(title.to_string())
        );
    }
}

const APP_NAME: &str = "OpenURWTest";
const VERSION: &str = env!("CARGO_PKG_VERSION");

const BANNER: &str = concat!(
    "█████ ████  █████ █   █ █   █ ████  █   █ █████ █████ █████ █████\n",
    "█   █ █   █ █     ██  █ █   █ █   █ █   █   █   █     █       █  \n",
    "█   █ ████  ████  █ █ █ █   █ ████  █ █ █   █   ████  ████    █  \n",
    "█   █ █     █     █  ██ █   █ █  █  ██ ██   █   █         █   █  \n",
    "█████ █     █████ █   █ █████ █   █ █   █   █   █████ █████   █  ",
);

pub fn run_target(
    target: &std::path::Path,
    mode: RunMode,
    passes: u32,
    continue_on_error: bool,
    color: ColorMode,
    mouse: MouseMode,
) -> Result<()> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        anyhow::bail!("the TUI requires a terminal");
    }
    let mut terminal = ratatui::init();
    mouse::install_panic_hook();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle(format!("{APP_NAME} v{VERSION}"))
    );
    let mut mouse_guard = MouseGuard::new(mouse != MouseMode::Off);
    let result = app_loop_target(
        &mut mouse_guard,
        &mut terminal,
        target,
        mode,
        passes,
        continue_on_error,
        color,
    );
    drop(mouse_guard);
    ratatui::restore();
    result
}

fn app_loop_target(
    mouse_guard: &mut MouseGuard,
    terminal: &mut ratatui::DefaultTerminal,
    target: &std::path::Path,
    mode: RunMode,
    passes: u32,
    continue_on_error: bool,
    color: ColorMode,
) -> Result<()> {
    let mut app = App::new(color);
    let selected_volume = volume::find_volume(target).ok_or_else(|| {
        anyhow::anyhow!("target is not a mounted volume root: {}", target.display())
    })?;
    app.selected = app
        .volumes
        .iter()
        .position(|volume| volume.mount_point == selected_volume.mount_point)
        .ok_or_else(|| anyhow::anyhow!("target volume is not in the volume list"))?;
    app.open_settings(mode);
    app.settings.values.passes = passes.max(1);
    app.settings.values.continue_on_error = continue_on_error;
    app.settings.values.normalize();
    app.settings.passes_input = app.settings.values.passes.to_string();
    loop {
        terminal.draw(|frame| app.render(frame))?;
        let timeout = if app.animating() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(200)
        };
        if event::poll(timeout)? {
            if handle_terminal_event(&mut app, mouse_guard, event::read()?)? {
                break;
            }
            if drain_events(&mut app, mouse_guard)? {
                break;
            }
        }
        app.tick();
    }
    Ok(())
}

pub fn run_home(color: ColorMode, mouse: MouseMode) -> Result<()> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        anyhow::bail!("the TUI requires a terminal");
    }
    let mut terminal = ratatui::init();
    mouse::install_panic_hook();
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::terminal::SetTitle(format!("{APP_NAME} v{VERSION}"))
    );
    let mut mouse_guard = MouseGuard::new(mouse != MouseMode::Off);
    let result = app_loop(&mut mouse_guard, &mut terminal, color);
    drop(mouse_guard);
    ratatui::restore();
    result
}

fn handle_terminal_event(
    app: &mut App,
    mouse_guard: &mut MouseGuard,
    event: Event,
) -> Result<bool> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if app.handle_key(key) {
                return Ok(true);
            }
            if app.mouse_enabled != mouse_guard.enabled {
                mouse_guard.set(app.mouse_enabled)?;
            }
        }
        Event::Mouse(mouse) => {
            if app.mouse_event(mouse) {
                return Ok(true);
            }
        }
        Event::Resize(_, _) => {}
        _ => {}
    }
    Ok(false)
}

fn drain_events(app: &mut App, mouse_guard: &mut MouseGuard) -> Result<bool> {
    while event::poll(Duration::ZERO)? {
        if handle_terminal_event(app, mouse_guard, event::read()?)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn app_loop(
    mouse_guard: &mut MouseGuard,
    terminal: &mut ratatui::DefaultTerminal,
    color: ColorMode,
) -> Result<()> {
    let mut app = App::new(color);
    loop {
        terminal.draw(|frame| app.render(frame))?;
        let timeout = if app.animating() {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(200)
        };
        if event::poll(timeout)? {
            if handle_terminal_event(&mut app, mouse_guard, event::read()?)? {
                break;
            }
            if drain_events(&mut app, mouse_guard)? {
                break;
            }
        }
        app.tick();
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    WriteVerify,
    WriteOnly,
    VerifyExisting,
}

enum TuiEvent {
    PassStart { pass: u32, passes: u32 },
    Info(String),
    Warn(String),
    Error(String),
    PhaseStart { phase: Phase },
    Progress(ProgressInfo),
    PhaseDone { phase: Phase, stats: PhaseStats },
    Finished(Box<Summary>),
    Suite(Box<SuiteEvent>),
    SuiteFinished(Box<SuiteResult>),
    Failed(String),
}

enum EventSink {
    Single(mpsc::Sender<TuiEvent>),
    Multi {
        index: usize,
        tx: mpsc::Sender<(usize, TuiEvent)>,
    },
}

struct TuiReporter {
    sink: EventSink,
}

impl TuiReporter {
    fn single(tx: mpsc::Sender<TuiEvent>) -> Self {
        Self {
            sink: EventSink::Single(tx),
        }
    }

    fn multi(index: usize, tx: mpsc::Sender<(usize, TuiEvent)>) -> Self {
        Self {
            sink: EventSink::Multi { index, tx },
        }
    }

    fn failed(&self, message: String) {
        self.send(TuiEvent::Failed(message));
    }

    fn send(&self, event: TuiEvent) {
        match &self.sink {
            EventSink::Single(tx) => {
                let _ = tx.send(event);
            }
            EventSink::Multi { index, tx } => {
                let _ = tx.send((*index, event));
            }
        }
    }
}

impl Reporter for TuiReporter {
    fn pass_started(&mut self, pass: u32, passes: u32) {
        self.send(TuiEvent::PassStart { pass, passes });
    }
    fn info(&mut self, message: &str) {
        self.send(TuiEvent::Info(message.to_string()));
    }
    fn warn(&mut self, message: &str) {
        self.send(TuiEvent::Warn(message.to_string()));
    }
    fn error(&mut self, error: &ErrorInfo<'_>) {
        self.send(TuiEvent::Error(format!(
            "{} error at pass {} block {} ({}): {}",
            error.phase.label(),
            error.pass,
            error.block,
            format_bytes(error.block.saturating_mul(format::BLOCK_SIZE)),
            error.message
        )));
    }
    fn phase_started(&mut self, _pass: u32, phase: Phase, _total: u64) {
        self.send(TuiEvent::PhaseStart { phase });
    }
    fn progress(&mut self, progress: &ProgressInfo) {
        self.send(TuiEvent::Progress(*progress));
    }
    fn phase_finished(&mut self, _pass: u32, phase: Phase, stats: &PhaseStats) {
        self.send(TuiEvent::PhaseDone {
            phase,
            stats: *stats,
        });
    }
    fn finished(&mut self, summary: &Summary) {
        self.send(TuiEvent::Finished(Box::new(summary.clone())));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsField {
    Mode,
    Passes,
    Continue,
    Delete,
    Export,
    ReportTemperature,
}

impl SettingsField {
    fn next(self) -> Self {
        match self {
            SettingsField::Mode => SettingsField::Passes,
            SettingsField::Passes => SettingsField::Continue,
            SettingsField::Continue => SettingsField::Delete,
            SettingsField::Delete => SettingsField::Export,
            SettingsField::Export => SettingsField::ReportTemperature,
            SettingsField::ReportTemperature => SettingsField::Mode,
        }
    }

    fn previous(self) -> Self {
        match self {
            SettingsField::Mode => SettingsField::ReportTemperature,
            SettingsField::ReportTemperature => SettingsField::Export,
            SettingsField::Passes => SettingsField::Mode,
            SettingsField::Continue => SettingsField::Passes,
            SettingsField::Delete => SettingsField::Continue,
            SettingsField::Export => SettingsField::Delete,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Settings {
    mode: RunMode,
    passes: u32,
    continue_on_error: bool,
    delete_after: bool,
    auto_export: bool,
    report_temperature: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: RunMode::WriteVerify,
            passes: 1,
            continue_on_error: false,
            delete_after: false,
            auto_export: true,
            report_temperature: false,
        }
    }
}

impl Settings {
    fn passes_editable(&self) -> bool {
        !matches!(self.mode, RunMode::VerifyExisting)
    }
    fn continue_editable(&self) -> bool {
        matches!(self.mode, RunMode::WriteVerify) && self.passes > 1
    }
    fn normalize(&mut self) {
        if !self.passes_editable() {
            self.passes = 1;
        }
        if !self.continue_editable() {
            self.continue_on_error = false;
        }
    }
}

struct SettingsState {
    values: Settings,
    field: SettingsField,
    passes_input: String,
}

impl SettingsState {
    fn new(values: Settings) -> Self {
        Self {
            values,
            field: SettingsField::Mode,
            passes_input: values.passes.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
struct TaskControl {
    cancel: CancelToken,
    pause: PauseToken,
}

impl TaskControl {
    fn new(cancel: CancelToken, pause: PauseToken) -> Self {
        Self { cancel, pause }
    }
}

struct TaskViewState {
    latest: Option<ProgressInfo>,
    phase: Option<Phase>,
    total: u64,
    log: VecDeque<String>,
    speed_history: Vec<(u64, f64)>,
    health: DriveHealth,
}

impl TaskViewState {
    fn new(health: DriveHealth) -> Self {
        Self {
            latest: None,
            phase: None,
            total: 0,
            log: VecDeque::new(),
            speed_history: Vec::new(),
            health,
        }
    }
}

struct RunState {
    rx: Receiver<TuiEvent>,
    target: String,
    target_path: PathBuf,
    mode: RunMode,
    passes: u32,
    delete_after: bool,
    auto_export: bool,
    report_temperature: bool,
    view: TaskViewState,
    started: Instant,
    cancel_requested: bool,
    control: TaskControl,
}

struct ResultsState {
    target: PathBuf,
    summary: Summary,
    analysis: Analysis,
    params: AnalysisParams,
    axis: Axis,
    message: String,
    include_temperature: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Position,
    Time,
}

impl Axis {
    fn toggle(self) -> Self {
        match self {
            Axis::Position => Axis::Time,
            Axis::Time => Axis::Position,
        }
    }
}

trait TaskView {
    fn task_label(&self) -> &str;
    fn task_paused(&self) -> bool;
    fn task_status(&self) -> TaskStatus;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Home,
    Settings,
    Confirm,
    SystemConfirm,
    Run,
    Results,
    MultiRun,
    MultiResults,
    SpeedSettings,
    SpeedConfirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TaskStatus {
    #[default]
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl TaskStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Queued => "排队",
            Self::Running => "运行中",
            Self::Done => "完成",
            Self::Failed => "失败",
            Self::Cancelled => "已停止",
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

struct MultiTask {
    label: String,
    target_path: PathBuf,
    mode: RunMode,
    control: TaskControl,
    view: TaskViewState,
    status: TaskStatus,
    summary: Option<Summary>,
    suite: Option<SuiteView>,
    error: Option<String>,
    cancel_requested: bool,
}

struct SuiteView {
    current: usize,
    phase: Option<SuitePhase>,
    done_bytes: u64,
    total_bytes: u64,
    mibps: f64,
    write_history: Vec<(u64, f64)>,
    read_history: Vec<(u64, f64)>,
    write_curves: Vec<Vec<(u64, f64)>>,
    read_curves: Vec<Vec<(u64, f64)>>,
    write_samples: Vec<Vec<f64>>,
    read_samples: Vec<Vec<f64>>,
    bar_values: [Option<f64>; 4],
    error_offset: Option<u64>,
    results: Vec<PresetResult>,
    stopped: bool,
}

impl Default for SuiteView {
    fn default() -> Self {
        Self {
            current: 0,
            phase: None,
            done_bytes: 0,
            total_bytes: 0,
            mibps: 0.0,
            write_history: Vec::new(),
            read_history: Vec::new(),
            write_curves: vec![Vec::new(); 4],
            read_curves: vec![Vec::new(); 4],
            write_samples: vec![Vec::new(); 4],
            read_samples: vec![Vec::new(); 4],
            bar_values: [None; 4],
            error_offset: None,
            results: Vec::new(),
            stopped: false,
        }
    }
}

struct MultiRun {
    kind: TestKind,
    tasks: Vec<MultiTask>,
    rx: mpsc::Receiver<(usize, TuiEvent)>,
    active: usize,
    started: Instant,
    cancel_menu: bool,
    export_message: String,
    auto_export: bool,
    exported: bool,
    notified: bool,
    results_scroll: usize,
}

impl MultiRun {
    fn all_done(&self) -> bool {
        self.tasks.iter().all(|task| {
            matches!(
                task.status,
                TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled
            )
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct SpeedSettings {
    presets: [bool; 4],
    file_size: u64,
    ignore_errors: bool,
}

impl Default for SpeedSettings {
    fn default() -> Self {
        Self {
            presets: [true; 4],
            file_size: 4 * 1024 * 1024 * 1024,
            ignore_errors: false,
        }
    }
}

struct SpeedState {
    values: SpeedSettings,
    focus: usize,
}

struct App {
    theme: Theme,
    mouse_enabled: bool,
    checked: std::collections::BTreeSet<PathBuf>,
    kind: TestKind,
    speed: SpeedState,
    ascii: bool,
    system_confirm_input: String,
    hotspots: RefCell<Vec<Hotspot>>,
    mouse_hover: Option<Rect>,
    animation_epoch: Instant,
    error_flash_start: Option<Instant>,
    multi: Option<MultiRun>,
    screen: Screen,
    volumes: Vec<VolumeInfo>,
    selected: usize,
    message: String,
    last_refresh: Instant,
    last_active_health_refresh: Instant,
    health_cache: RefCell<Option<(PathBuf, DriveHealth, Instant)>>,
    settings: SettingsState,
    run: Option<RunState>,
    results: Option<ResultsState>,
    export_rx: Option<mpsc::Receiver<String>>,
    multi_export_rx: Option<mpsc::Receiver<String>>,
    analysis_rx: Option<mpsc::Receiver<crate::analysis::Analysis>>,
    analysis_pending: bool,
    cancel_pending: bool,
}

impl App {
    fn new(color: ColorMode) -> Self {
        Self {
            theme: Theme::new(color),
            mouse_enabled: true,
            checked: std::collections::BTreeSet::new(),
            kind: TestKind::Quality,
            speed: SpeedState {
                values: SpeedSettings::default(),
                focus: 0,
            },
            ascii: std::env::var_os("URWTEST_ASCII").is_some()
                || (cfg!(not(test))
                    && cfg!(windows)
                    && std::env::var_os("WT_SESSION").is_none()
                    && std::env::var_os("TERM_PROGRAM").is_none()),
            system_confirm_input: String::new(),
            hotspots: RefCell::new(Vec::new()),
            mouse_hover: None,
            animation_epoch: Instant::now(),
            error_flash_start: None,
            multi: None,
            screen: Screen::Home,
            volumes: volume::list_volumes(),
            selected: 0,
            message: String::new(),
            last_refresh: Instant::now(),
            last_active_health_refresh: Instant::now(),
            health_cache: RefCell::new(None),
            settings: SettingsState::new(Settings::default()),
            run: None,
            results: None,
            export_rx: None,
            multi_export_rx: None,
            analysis_rx: None,
            analysis_pending: false,
            cancel_pending: false,
        }
    }

    fn compact_run_line(&self) -> Option<String> {
        match self.screen {
            Screen::Run => {
                let run = self.run.as_ref()?;
                let (bytes, total) = run
                    .view
                    .latest
                    .map(|progress| (progress.bytes, progress.total_bytes))
                    .unwrap_or((0, run.view.total.max(1)));
                let percent = bytes.saturating_mul(100).checked_div(total).unwrap_or(0);
                let state = if run.control.pause.is_paused() {
                    "已暂停"
                } else {
                    "运行中"
                };
                Some(format!(
                    "{}  {}%  {} / {}",
                    state,
                    percent,
                    format_bytes(bytes),
                    format_bytes(total)
                ))
            }
            Screen::MultiRun => {
                let multi = self.multi.as_ref()?;
                let task = multi
                    .tasks
                    .get(multi.active.min(multi.tasks.len().saturating_sub(1)))?;
                let (bytes, total) = if let Some(suite) = &task.suite {
                    (suite.done_bytes, suite.total_bytes)
                } else {
                    task.view
                        .latest
                        .map(|progress| (progress.bytes, progress.total_bytes))
                        .unwrap_or((0, task.view.total.max(1)))
                };
                let percent = bytes.saturating_mul(100).checked_div(total).unwrap_or(0);
                let state = if task.control.pause.is_paused() {
                    "已暂停"
                } else {
                    "运行中"
                };
                Some(format!(
                    "{}  {}  {}%  {} / {}",
                    state,
                    task.label,
                    percent,
                    format_bytes(bytes),
                    format_bytes(total)
                ))
            }
            _ => None,
        }
    }

    fn render(&self, frame: &mut Frame) {
        self.hotspots.borrow_mut().clear();
        let area = frame.area();
        if area.width < 100 || area.height < 26 {
            if let Some(line) = self.compact_run_line() {
                frame.render_widget(Clear, area);
                frame.render_widget(
                    Paragraph::new(line)
                        .alignment(ratatui::layout::Alignment::Center)
                        .block(Block::bordered().title(" 运行中 ")),
                    area,
                );
                return;
            }
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from("终端窗口过小"),
                    Line::from("请调整到至少 100 x 26"),
                    Line::from("当前尺寸："),
                    Line::from(format!("{} x {}", area.width, area.height)),
                ])
                .alignment(ratatui::layout::Alignment::Center)
                .block(Block::bordered().title(" 终端尺寸 ")),
                area,
            );
            return;
        }
        match self.screen {
            Screen::Home => self.render_home(frame),
            Screen::Settings => self.render_settings(frame),
            Screen::Confirm => self.render_confirm(frame),
            Screen::SystemConfirm => self.render_system_confirm(frame),
            Screen::Run => self.render_run(frame),
            Screen::Results => self.render_results(frame),
            Screen::MultiRun => self.render_multi(frame),
            Screen::MultiResults => self.render_multi_results(frame),
            Screen::SpeedSettings => self.render_speed_settings(frame),
            Screen::SpeedConfirm => self.render_speed_confirm(frame),
        }
    }
}

impl ResultsState {
    fn mode_label(&self) -> String {
        match self.params.mode {
            ThresholdMode::Percent => format!("percent {:.0}%", self.params.percent),
            ThresholdMode::Absolute => format!("absolute {:.0} MiB/s", self.params.absolute_mibps),
        }
    }

    fn export(&mut self) {
        let directory = export_directory_for(&self.target);
        let input = ExportInput {
            target: &self.target,
            summary: &self.summary,
            analysis: &self.analysis,
            params: &self.params,
            include_temperature: self.include_temperature,
        };
        match export::export(&directory, &input) {
            Ok(files) => {
                self.message = format!(
                    "exported {} file(s) to {}",
                    files.len(),
                    directory.display()
                )
            }
            Err(error) => self.message = format!("export failed: {error}"),
        }
    }
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

impl TaskView for RunState {
    fn task_label(&self) -> &str {
        &self.target
    }

    fn task_paused(&self) -> bool {
        self.control.pause.is_paused()
    }

    fn task_status(&self) -> TaskStatus {
        TaskStatus::Running
    }
}

impl TaskView for MultiTask {
    fn task_label(&self) -> &str {
        &self.label
    }

    fn task_paused(&self) -> bool {
        self.control.pause.is_paused()
    }

    fn task_status(&self) -> TaskStatus {
        self.status
    }
}

#[cfg(test)]
mod tests;
