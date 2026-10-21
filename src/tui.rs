//! Terminal user interface.

mod animation;
mod confirm;
mod home;
mod mouse;
mod results;
mod run;
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
use crate::reporter::{ErrorInfo, ProgressInfo, Reporter};
use crate::speed::suite::{
    PRESETS, PresetResult, SuiteEvent, SuiteOptions, SuitePhase, SuiteResult, run_suite,
};
use crate::theme::Theme;
use crate::trace::Phase;
use crate::types::{ColorMode, MouseMode, TestKind};
use crate::volume::{self, VolumeInfo};

type SpeedSeries = Vec<(String, Vec<(u64, f64)>, ratatui::style::Color)>;

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
    if let Some(index) = app
        .volumes
        .iter()
        .position(|volume| volume.mount_point == target)
    {
        app.selected = index;
    }
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
        if event::poll(timeout)? && handle_terminal_event(&mut app, mouse_guard, event::read()?)? {
            break;
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
        if event::poll(timeout)? && handle_terminal_event(&mut app, mouse_guard, event::read()?)? {
            break;
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
    Suite(SuiteEvent),
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
}

impl SettingsField {
    fn next(self) -> Self {
        match self {
            SettingsField::Mode => SettingsField::Passes,
            SettingsField::Passes => SettingsField::Continue,
            SettingsField::Continue => SettingsField::Delete,
            SettingsField::Delete => SettingsField::Export,
            SettingsField::Export => SettingsField::Mode,
        }
    }

    fn previous(self) -> Self {
        match self {
            SettingsField::Mode => SettingsField::Export,
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: RunMode::WriteVerify,
            passes: 1,
            continue_on_error: false,
            delete_after: false,
            auto_export: true,
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

struct RunState {
    rx: Receiver<TuiEvent>,
    target: String,
    target_path: PathBuf,
    mode: RunMode,
    passes: u32,
    delete_after: bool,
    auto_export: bool,
    latest: Option<ProgressInfo>,
    phase: Option<Phase>,
    total: u64,
    log: VecDeque<String>,
    speed_history: Vec<(u64, f64)>,
    started: Instant,
    cancel_requested: bool,
    token: Option<CancelToken>,
    health: DriveHealth,
}

struct ResultsState {
    target: PathBuf,
    summary: Summary,
    analysis: Analysis,
    params: AnalysisParams,
    axis: Axis,
    message: String,
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
    token: CancelToken,
    latest: Option<ProgressInfo>,
    phase: Option<Phase>,
    total: u64,
    log: VecDeque<String>,
    status: TaskStatus,
    summary: Option<Summary>,
    speed_history: Vec<(u64, f64)>,
    suite: Option<SuiteView>,
    error: Option<String>,
    health: DriveHealth,
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
            ascii: std::env::var_os("URWTEST_ASCII").is_some(),
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
            cancel_pending: false,
        }
    }

    fn selected_volume(&self) -> Option<&VolumeInfo> {
        self.volumes.get(self.selected)
    }

    fn rescan(&mut self) {
        let current = self
            .volumes
            .get(self.selected)
            .map(|volume| volume.mount_point.clone());
        let old: Vec<std::path::PathBuf> = self
            .volumes
            .iter()
            .map(|volume| volume.mount_point.clone())
            .collect();
        self.volumes = volume::list_volumes();
        let added: Vec<String> = self
            .volumes
            .iter()
            .filter(|volume| !old.contains(&volume.mount_point))
            .map(|volume| volume.mount_point.display().to_string())
            .collect();
        let removed: Vec<String> = old
            .iter()
            .filter(|path| {
                !self
                    .volumes
                    .iter()
                    .any(|volume| &volume.mount_point == *path)
            })
            .map(|path| path.display().to_string())
            .collect();
        if let Some(current) = current
            && let Some(index) = self
                .volumes
                .iter()
                .position(|volume| volume.mount_point == current)
        {
            self.selected = index;
        }
        if self.selected >= self.volumes.len() {
            self.selected = self.volumes.len().saturating_sub(1);
        }
        self.checked.retain(|path| {
            self.volumes
                .iter()
                .any(|volume| &volume.mount_point == path)
        });
        if !added.is_empty() {
            self.message = format!("检测到新磁盘：{}", added.join("、"));
        } else if !removed.is_empty() {
            self.message = format!("磁盘已移除：{}", removed.join("、"));
        }
        self.last_refresh = Instant::now();
    }

    fn cached_health(&self, mount: &std::path::Path) -> DriveHealth {
        let mut cache = self.health_cache.borrow_mut();
        if let Some((cached_mount, health, at)) = cache.as_ref()
            && cached_mount == mount
            && at.elapsed() < Duration::from_secs(10)
        {
            return *health;
        }
        let health = health::read_health(mount);
        *cache = Some((mount.to_path_buf(), health, Instant::now()));
        health
    }

    fn tick(&mut self) {
        if self.screen == Screen::Home && self.last_refresh.elapsed() >= Duration::from_secs(5) {
            self.rescan();
        }
        if matches!(self.screen, Screen::Run | Screen::MultiRun)
            && self.last_active_health_refresh.elapsed() >= Duration::from_secs(30)
        {
            let run_mount = self.run.as_ref().map(|run| run.target_path.clone());
            if let Some(mount) = run_mount {
                let health = self.cached_health(&mount);
                if let Some(run) = self.run.as_mut() {
                    run.health = health;
                }
            }
            let mounts: Vec<(usize, PathBuf)> = self
                .multi
                .as_ref()
                .map(|multi| {
                    multi
                        .tasks
                        .iter()
                        .enumerate()
                        .filter(|(_, task)| task.status == TaskStatus::Running)
                        .map(|(index, task)| (index, task.target_path.clone()))
                        .collect()
                })
                .unwrap_or_default();
            let refreshed: Vec<(usize, DriveHealth)> = mounts
                .into_iter()
                .map(|(index, mount)| (index, self.cached_health(&mount)))
                .collect();
            if let Some(multi) = self.multi.as_mut() {
                for (index, health) in refreshed {
                    if let Some(task) = multi.tasks.get_mut(index) {
                        task.health = health;
                    }
                }
            }
            self.last_active_health_refresh = Instant::now();
        }
        let mut finished: Option<(Summary, bool, bool, PathBuf)> = None;
        let mut failed: Option<String> = None;
        if let Some(run) = self.run.as_mut() {
            while let Ok(event) = run.rx.try_recv() {
                match event {
                    TuiEvent::PassStart { pass, passes } => {
                        run.log.push_back(format!("pass {pass}/{passes}"));
                    }
                    TuiEvent::Info(message) | TuiEvent::Warn(message) => {
                        run.log.push_back(message);
                    }
                    TuiEvent::Error(message) => {
                        run.log.push_back(message);
                    }
                    TuiEvent::PhaseStart { phase } => {
                        run.phase = Some(phase);
                        run.log.push_back(format!("{} started", phase.label()));
                    }
                    TuiEvent::Progress(progress) => {
                        run.latest = Some(progress);
                        run.total = progress.total_bytes;
                        run.speed_history
                            .push((progress.bytes, progress.current_mibps));
                        if run.speed_history.len() > 240 {
                            run.speed_history.remove(0);
                        }
                    }
                    TuiEvent::PhaseDone { phase, stats } => {
                        run.log.push_back(format!(
                            "{}: {} in {:.1}s ({:.1} MiB/s)",
                            phase.label(),
                            format_bytes(stats.bytes),
                            stats.seconds,
                            stats.mib_per_sec()
                        ));
                    }
                    TuiEvent::Finished(summary) => {
                        finished = Some((
                            *summary,
                            run.delete_after,
                            run.auto_export,
                            run.target_path.clone(),
                        ));
                        break;
                    }
                    TuiEvent::Suite(_) | TuiEvent::SuiteFinished(_) => {}
                    TuiEvent::Failed(error) => {
                        failed = Some(error);
                        break;
                    }
                }
            }
            while run.log.len() > 50 {
                run.log.pop_front();
            }
        }
        if let Some((summary, delete_after, auto_export, target)) = finished {
            self.finish_run(summary, delete_after, auto_export, target);
        } else if let Some(error) = failed {
            self.message = error;
            self.screen = Screen::Home;
            self.run = None;
            self.rescan();
        }
        if self.screen == Screen::MultiRun || self.multi.is_some() {
            self.tick_multi();
        }
    }

    fn finish_run(
        &mut self,
        summary: Summary,
        delete_after: bool,
        auto_export: bool,
        target: PathBuf,
    ) {
        let params = AnalysisParams::default();
        let analysis = analysis::analyze(&summary.trace, &params);
        let success = summary.failed == 0
            && summary.error_pass.is_none()
            && summary.data_errors == 0
            && summary.read_errors == 0
            && summary.write_errors == 0
            && !summary.write_only
            && summary.passed > 0
            && !summary.interrupted
            && !summary.device_lost;
        let mut message = String::new();
        if auto_export {
            let directory = export_directory();
            let input = ExportInput {
                target: &target,
                summary: &summary,
                analysis: &analysis,
                params: &params,
            };
            match export::export(&directory, &input) {
                Ok(files) => {
                    message = format!(
                        "exported {} file(s) to {}",
                        files.len(),
                        directory.display()
                    );
                }
                Err(error) => message = format!("export failed: {error}"),
            }
        }
        if delete_after && success {
            match format::delete_test_files(&target) {
                Ok(count) => {
                    message.push_str(&format!("; deleted {count} test file(s)"));
                }
                Err(error) => message.push_str(&format!("; delete failed: {error}")),
            }
        }
        self.results = Some(ResultsState {
            target,
            summary,
            analysis,
            params,
            axis: Axis::Position,
            message,
        });
        self.screen = Screen::Results;
        self.run = None;
    }

    fn open_settings(&mut self, mode: RunMode) {
        if self.kind == TestKind::Speed {
            self.speed.focus = 0;
            self.screen = Screen::SpeedSettings;
            return;
        }
        let Some(volume) = self.selected_volume() else {
            self.message = "no volume selected".to_string();
            return;
        };
        let _ = volume;
        self.message.clear();
        self.settings.values.mode = mode;
        if mode == RunMode::VerifyExisting {
            self.settings.values.passes = 1;
            self.settings.values.continue_on_error = false;
        } else if mode == RunMode::WriteOnly {
            self.settings.values.continue_on_error = false;
        }
        self.settings.values.normalize();
        self.settings.passes_input = self.settings.values.passes.to_string();
        self.settings.field = SettingsField::Mode;
        self.screen = Screen::Settings;
    }

    fn launch_run(&mut self, settings: Settings) {
        let Some(volume) = self.selected_volume() else {
            self.message = "no volume selected".to_string();
            return;
        };
        let target = volume.mount_point.clone();
        let passes = settings.passes.max(1);
        let mode = settings.mode;
        let continue_on_error = settings.continue_on_error;
        cancel::reset();
        let token = CancelToken::new();
        let worker_token = token.clone();
        let (tx, rx) = mpsc::channel();
        let health = self.cached_health(&target);
        let target_for_thread = target.clone();
        thread::spawn(move || {
            let mut reporter = TuiReporter::single(tx);
            let result = match mode {
                RunMode::VerifyExisting => {
                    engine::verify_existing(&target_for_thread, &mut reporter)
                }
                RunMode::WriteOnly | RunMode::WriteVerify => {
                    let options = RunOptions {
                        target: &target_for_thread,
                        passes,
                        verify: mode == RunMode::WriteVerify,
                        continue_on_error,
                        reuse_existing: false,
                        cancel: Some(worker_token.clone()),
                    };
                    engine::run_os(&options, &mut reporter)
                }
            };
            if let Err(error) = result {
                reporter.failed(format!("{error:#}"));
            }
        });
        self.run = Some(RunState {
            rx,
            target: target.display().to_string(),
            target_path: target,
            mode,
            passes,
            delete_after: settings.delete_after,
            auto_export: settings.auto_export,
            latest: None,
            phase: None,
            total: 0,
            log: VecDeque::new(),
            speed_history: Vec::new(),
            started: Instant::now(),
            cancel_requested: false,
            token: Some(token),
            health,
        });
        self.screen = Screen::Run;
    }

    fn target_volumes(&self) -> Vec<VolumeInfo> {
        if self.checked.is_empty() {
            self.selected_volume().cloned().into_iter().collect()
        } else {
            self.volumes
                .iter()
                .filter(|volume| self.checked.contains(&volume.mount_point))
                .cloned()
                .collect()
        }
    }

    fn toggle_check(&mut self) {
        let Some(volume) = self.selected_volume() else {
            return;
        };
        let path = volume.mount_point.clone();
        if !self.checked.remove(&path) {
            self.checked.insert(path);
        }
    }

    fn select_all(&mut self) {
        self.checked = self
            .volumes
            .iter()
            .map(|volume| volume.mount_point.clone())
            .collect();
    }

    fn clear_checks(&mut self) {
        self.checked.clear();
    }

    fn selected_is_system_volume(&self, volume: &VolumeInfo) -> bool {
        volume.mount_point.join("Windows").exists()
    }

    fn needs_system_confirmation(&self) -> bool {
        let active =
            self.kind == TestKind::Quality && self.settings.values.mode != RunMode::VerifyExisting;
        active
            && self
                .target_volumes()
                .iter()
                .any(|volume| self.selected_is_system_volume(volume))
    }

    fn launch_selected(&mut self, settings: Settings) {
        if self.checked.is_empty() {
            self.launch_run(settings);
        } else {
            self.launch_multi(settings);
        }
    }

    fn launch_multi(&mut self, settings: Settings) {
        let targets = self.target_volumes();
        if targets.is_empty() {
            return;
        }
        cancel::reset();
        let (tx, rx) = mpsc::channel();
        let mut tasks = Vec::with_capacity(targets.len());
        for (index, volume) in targets.iter().enumerate() {
            let token = CancelToken::new();
            let target = volume.mount_point.clone();
            let thread_target = target.clone();
            let thread_token = token.clone();
            let tx = tx.clone();
            let mode = settings.mode;
            let passes = settings.passes;
            let continue_on_error = settings.continue_on_error;
            let health = self.cached_health(&volume.mount_point);
            thread::spawn(move || {
                let mut reporter = TuiReporter::multi(index, tx);
                let result = match mode {
                    RunMode::VerifyExisting => {
                        engine::verify_existing(&thread_target, &mut reporter)
                    }
                    RunMode::WriteOnly | RunMode::WriteVerify => {
                        let options = RunOptions {
                            target: &thread_target,
                            passes,
                            verify: mode == RunMode::WriteVerify,
                            continue_on_error,
                            reuse_existing: false,
                            cancel: Some(thread_token),
                        };
                        engine::run_os(&options, &mut reporter)
                    }
                };
                if let Err(error) = result {
                    reporter.failed(format!("{error:#}"));
                }
            });
            tasks.push(MultiTask {
                label: volume.display_name(),
                target_path: target.clone(),
                mode,
                token,
                latest: None,
                phase: None,
                total: 0,
                log: VecDeque::new(),
                status: TaskStatus::Queued,
                summary: None,
                speed_history: Vec::new(),
                suite: None,
                error: None,
                health,
                cancel_requested: false,
            });
        }
        self.multi = Some(MultiRun {
            kind: TestKind::Quality,
            tasks,
            rx,
            active: 0,
            started: Instant::now(),
            cancel_menu: false,
            export_message: String::new(),
            auto_export: settings.auto_export,
            exported: false,
        });
        self.screen = Screen::MultiRun;
    }

    fn tick_multi(&mut self) {
        let mut finished_all = false;
        if let Some(multi) = self.multi.as_mut() {
            while let Ok((index, event)) = multi.rx.try_recv() {
                let Some(task) = multi.tasks.get_mut(index) else {
                    continue;
                };
                match event {
                    TuiEvent::PassStart { pass, passes } => {
                        task.status = TaskStatus::Running;
                        task.log.push_back(format!("pass {pass}/{passes}"));
                    }
                    TuiEvent::Info(message)
                    | TuiEvent::Warn(message)
                    | TuiEvent::Error(message) => {
                        task.log.push_back(message);
                    }
                    TuiEvent::PhaseStart { phase } => {
                        task.status = TaskStatus::Running;
                        task.phase = Some(phase);
                        task.log.push_back(format!("{} started", phase.label()));
                    }
                    TuiEvent::Progress(progress) => {
                        task.status = TaskStatus::Running;
                        task.latest = Some(progress);
                        task.total = progress.total_bytes;
                        task.speed_history
                            .push((progress.bytes, progress.current_mibps));
                        if task.speed_history.len() > 240 {
                            task.speed_history.remove(0);
                        }
                    }
                    TuiEvent::PhaseDone { phase, stats } => {
                        task.log.push_back(format!(
                            "{}: {} in {:.1}s",
                            phase.label(),
                            format_bytes(stats.bytes),
                            stats.seconds
                        ));
                    }
                    TuiEvent::Finished(summary) => {
                        let failed = summary.failed > 0
                            || summary.error_pass.is_some()
                            || summary.data_errors > 0
                            || summary.read_errors > 0
                            || summary.write_errors > 0;
                        task.summary = Some(*summary);
                        task.status = if task.cancel_requested {
                            TaskStatus::Cancelled
                        } else if failed {
                            TaskStatus::Failed
                        } else {
                            TaskStatus::Done
                        };
                    }
                    TuiEvent::Suite(event) => {
                        task.status = TaskStatus::Running;
                        let suite = task.suite.get_or_insert_with(SuiteView::default);
                        match event {
                            SuiteEvent::PresetStarted { index } => {
                                suite.current = index;
                                suite.phase = None;
                                suite.done_bytes = 0;
                                suite.total_bytes = 0;
                                suite.mibps = 0.0;
                                suite.write_history.clear();
                                suite.read_history.clear();
                                if let Some(curve) = suite.write_curves.get_mut(index) {
                                    curve.clear();
                                }
                                if let Some(curve) = suite.read_curves.get_mut(index) {
                                    curve.clear();
                                }
                                if let Some(samples) = suite.write_samples.get_mut(index) {
                                    samples.clear();
                                }
                                if let Some(samples) = suite.read_samples.get_mut(index) {
                                    samples.clear();
                                }
                                if index >= 2 {
                                    suite.bar_values[(index - 2) * 2] = None;
                                    suite.bar_values[(index - 2) * 2 + 1] = None;
                                }
                                suite.error_offset = None;
                                task.log.push_back(format!(
                                    "开始 {}",
                                    PRESETS.get(index).map(|item| item.name).unwrap_or("-")
                                ));
                            }
                            SuiteEvent::PhaseStarted { index, phase } => {
                                suite.current = index;
                                suite.phase = Some(phase);
                                suite.done_bytes = 0;
                                suite.mibps = 0.0;
                                task.log.push_back(format!(
                                    "{}：{}",
                                    PRESETS.get(index).map(|item| item.name).unwrap_or("-"),
                                    match phase {
                                        SuitePhase::Write => "写入阶段",
                                        SuitePhase::Verify => "读取校验阶段",
                                    }
                                ));
                            }
                            SuiteEvent::Progress {
                                index,
                                phase,
                                done_bytes,
                                total_bytes,
                                mibps,
                            } => {
                                suite.current = index;
                                suite.phase = Some(phase);
                                suite.done_bytes = done_bytes;
                                suite.total_bytes = total_bytes;
                                suite.mibps = mibps;
                                match phase {
                                    SuitePhase::Write => {
                                        suite.write_history.push((done_bytes, mibps));
                                        if suite.write_history.len() > 240 {
                                            suite.write_history.remove(0);
                                        }
                                        if let Some(curve) = suite.write_curves.get_mut(index) {
                                            curve.push((done_bytes, mibps));
                                            if curve.len() > 240 {
                                                curve.remove(0);
                                            }
                                        }
                                        if let Some(samples) = suite.write_samples.get_mut(index) {
                                            samples.push(mibps);
                                        }
                                    }
                                    SuitePhase::Verify => {
                                        suite.read_history.push((done_bytes, mibps));
                                        if suite.read_history.len() > 240 {
                                            suite.read_history.remove(0);
                                        }
                                        if let Some(curve) = suite.read_curves.get_mut(index) {
                                            curve.push((done_bytes, mibps));
                                            if curve.len() > 240 {
                                                curve.remove(0);
                                            }
                                        }
                                        if let Some(samples) = suite.read_samples.get_mut(index) {
                                            samples.push(mibps);
                                        }
                                    }
                                }
                            }
                            SuiteEvent::Error {
                                index,
                                phase,
                                offset,
                                kind,
                            } => {
                                suite.error_offset = Some(offset);
                                task.log.push_back(format!(
                                    "{} {}：写入量 {} 处发现 {}",
                                    PRESETS.get(index).map(|item| item.name).unwrap_or("-"),
                                    match phase {
                                        SuitePhase::Write => "写入",
                                        SuitePhase::Verify => "读取",
                                    },
                                    format_bytes(offset),
                                    kind
                                ));
                            }
                            SuiteEvent::PresetFinished { index, result } => {
                                if index >= 2 {
                                    let write_mean = suite
                                        .write_samples
                                        .get(index)
                                        .map(|samples| trimmed_mean(samples))
                                        .unwrap_or(0.0);
                                    let read_mean = suite
                                        .read_samples
                                        .get(index)
                                        .map(|samples| trimmed_mean(samples))
                                        .unwrap_or(0.0);
                                    suite.bar_values[(index - 2) * 2] = Some(write_mean);
                                    suite.bar_values[(index - 2) * 2 + 1] = Some(read_mean);
                                }
                                suite.results.push(result);
                                task.log.push_back(format!(
                                    "{} 完成：写 {:.1} MiB/s，读 {:.1} MiB/s，校验错误 {}",
                                    PRESETS.get(index).map(|item| item.name).unwrap_or("-"),
                                    result.write.write_mibps,
                                    result.read.read_mibps,
                                    result.verify_errors
                                ));
                            }
                        }
                    }
                    TuiEvent::SuiteFinished(result) => {
                        if let Some(suite) = task.suite.as_mut() {
                            suite.results = result.presets;
                            suite.stopped = result.stopped;
                        }
                        task.status = if result.cancelled || task.cancel_requested {
                            TaskStatus::Cancelled
                        } else if result.stopped {
                            TaskStatus::Failed
                        } else {
                            TaskStatus::Done
                        };
                        if result.stopped {
                            self.error_flash_start = Some(Instant::now());
                            let offset = result
                                .stopped_at
                                .map(format_bytes)
                                .unwrap_or_else(|| "未知位置".to_string());
                            task.error = Some(format!(
                                "顺序项校验发现文件损坏（写入量 {offset} 处），速度测试已停止"
                            ));
                            task.log.push_back(task.error.clone().unwrap_or_default());
                        }
                    }
                    TuiEvent::Failed(error) => {
                        task.error = Some(error);
                        task.status = TaskStatus::Failed;
                    }
                }
                while task.log.len() > 30 {
                    task.log.pop_front();
                }
            }
            finished_all = multi.all_done();
        }
        if finished_all {
            self.maybe_auto_export_multi();
            self.screen = Screen::MultiResults;
        }
    }

    fn cancel_multi(&mut self, all: bool) {
        let mut needs_global = false;
        let mut active_is_verify = false;
        if let Some(multi) = self.multi.as_mut() {
            let active = multi.active.min(multi.tasks.len().saturating_sub(1));
            active_is_verify = multi.tasks[active].mode == RunMode::VerifyExisting;
            for (index, task) in multi.tasks.iter_mut().enumerate() {
                if task.mode == RunMode::VerifyExisting {
                    needs_global = true;
                }
                if all || index == active {
                    task.cancel_requested = true;
                    task.token.cancel();
                }
            }
        }
        if needs_global && (all || active_is_verify) {
            cancel::request();
        }
    }

    fn handle_multi_key(&mut self, key: KeyEvent) {
        let Some(multi) = self.multi.as_mut() else {
            return;
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            multi.cancel_menu = true;
            return;
        }
        if multi.cancel_menu {
            match key.code {
                KeyCode::Char('1') | KeyCode::Enter => {
                    multi.cancel_menu = false;
                    self.cancel_multi(false);
                }
                KeyCode::Char('2') | KeyCode::Char('S') | KeyCode::Char('s')
                    if multi.tasks.len() > 1 =>
                {
                    multi.cancel_menu = false;
                    self.cancel_multi(true);
                }
                KeyCode::Esc | KeyCode::Char('3') => multi.cancel_menu = false,
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc => multi.cancel_menu = true,
            KeyCode::Tab | KeyCode::Right => {
                multi.active = (multi.active + 1) % multi.tasks.len().max(1);
            }
            KeyCode::BackTab | KeyCode::Left => {
                multi.active = multi
                    .active
                    .checked_sub(1)
                    .unwrap_or_else(|| multi.tasks.len().saturating_sub(1));
            }
            KeyCode::Char(digit @ '1'..='9') => {
                let index = digit as usize - '1' as usize;
                if index < multi.tasks.len() {
                    multi.active = index;
                }
            }
            _ => {}
        }
    }

    fn selected_speed_presets(&self) -> Vec<crate::speed::suite::SpeedPreset> {
        PRESETS
            .iter()
            .enumerate()
            .filter(|(index, _)| self.speed.values.presets[*index])
            .map(|(_, preset)| *preset)
            .collect()
    }

    fn cycle_speed_file_size(&mut self, forward: bool) {
        const SIZES: [u64; 4] = [1u64 << 30, 2u64 << 30, 4u64 << 30, 8u64 << 30];
        let current = self.speed.values.file_size;
        let index = SIZES.iter().position(|size| *size == current).unwrap_or(2);
        let next = if forward {
            (index + 1) % SIZES.len()
        } else {
            (index + SIZES.len() - 1) % SIZES.len()
        };
        self.speed.values.file_size = SIZES[next];
    }

    fn handle_speed_settings(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.screen = Screen::Home,
            KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k') => {
                self.speed.focus = (self.speed.focus + 5) % 6;
            }
            KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => {
                self.speed.focus = (self.speed.focus + 1) % 6;
            }
            KeyCode::Char(' ') => {
                if self.speed.focus < 4 {
                    let index = self.speed.focus;
                    self.speed.values.presets[index] = !self.speed.values.presets[index];
                }
            }
            KeyCode::Char('a') => self.speed.values.presets = [true; 4],
            KeyCode::Char('n') => self.speed.values.presets = [false; 4],
            KeyCode::Left | KeyCode::Char('h') => {
                if self.speed.focus == 4 {
                    self.cycle_speed_file_size(false);
                } else if self.speed.focus == 5 {
                    self.speed.values.ignore_errors = !self.speed.values.ignore_errors;
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.speed.focus == 4 {
                    self.cycle_speed_file_size(true);
                } else if self.speed.focus == 5 {
                    self.speed.values.ignore_errors = !self.speed.values.ignore_errors;
                }
            }
            KeyCode::Enter => {
                if self.selected_speed_presets().is_empty() {
                    self.message = "请至少选择一个测试项目".to_string();
                } else {
                    self.system_confirm_input.clear();
                    self.screen = Screen::SpeedConfirm;
                }
            }
            _ => {}
        }
    }

    fn handle_speed_confirm(&mut self, key: KeyEvent) {
        let any_system = self
            .target_volumes()
            .iter()
            .any(|volume| self.selected_is_system_volume(volume));
        match key.code {
            KeyCode::Esc => {
                self.system_confirm_input.clear();
                self.screen = Screen::SpeedSettings;
            }
            KeyCode::Backspace => {
                self.system_confirm_input.pop();
            }
            KeyCode::Char(ch) if any_system && ch.is_ascii_alphabetic() => {
                self.system_confirm_input.push(ch.to_ascii_uppercase());
            }
            KeyCode::Char('n') if !any_system => self.screen = Screen::SpeedSettings,
            KeyCode::Char('y') if !any_system => self.launch_speed_multi(),
            KeyCode::Enter => {
                if any_system && !self.system_confirm_input.eq_ignore_ascii_case("YES") {
                    self.message = "请输入 YES 后再确认".to_string();
                } else {
                    self.launch_speed_multi();
                }
            }
            _ => {}
        }
    }

    fn launch_speed_multi(&mut self) {
        let targets = self.target_volumes();
        if targets.is_empty() {
            return;
        }
        let presets = self.selected_speed_presets();
        if presets.is_empty() {
            self.message = "请至少选择一个测试项目".to_string();
            return;
        }
        cancel::reset();
        let settings = self.speed.values;
        let (tx, rx) = mpsc::channel();
        let mut tasks = Vec::with_capacity(targets.len());
        for (index, volume) in targets.iter().enumerate() {
            let token = CancelToken::new();
            let target = volume.mount_point.clone();
            let thread_target = target.clone();
            let tx = tx.clone();
            let token_for_thread = token.clone();
            let presets = presets.clone();
            let health = self.cached_health(&volume.mount_point);
            thread::spawn(move || {
                let options = SuiteOptions {
                    file_size: settings.file_size,
                    ignore_sequential_errors: settings.ignore_errors,
                    seed: 0x5EED_1234_ABCD_0001,
                };
                let tx_events = tx.clone();
                let result = run_suite(
                    &thread_target,
                    &presets,
                    options,
                    token_for_thread.flag(),
                    &mut |event| {
                        let _ = tx_events.send((index, TuiEvent::Suite(event)));
                    },
                );
                let _ = std::fs::remove_file(thread_target.join("BENCH0001.DAT"));
                match result {
                    Ok(result) => {
                        let _ = tx.send((index, TuiEvent::SuiteFinished(Box::new(result))));
                    }
                    Err(error) => {
                        let _ = tx.send((index, TuiEvent::Failed(format!("{error:#}"))));
                    }
                }
            });
            tasks.push(MultiTask {
                label: volume.display_name(),
                target_path: target.clone(),
                mode: RunMode::WriteOnly,
                token,
                latest: None,
                phase: None,
                total: 0,
                log: VecDeque::new(),
                status: TaskStatus::Running,
                summary: None,
                speed_history: Vec::new(),
                suite: Some(SuiteView::default()),
                error: None,
                health,
                cancel_requested: false,
            });
        }
        self.multi = Some(MultiRun {
            kind: TestKind::Speed,
            tasks,
            rx,
            active: 0,
            started: Instant::now(),
            cancel_menu: false,
            export_message: String::new(),
            auto_export: true,
            exported: false,
        });
        self.screen = Screen::MultiRun;
    }

    fn maybe_auto_export_multi(&mut self) {
        let should = self
            .multi
            .as_ref()
            .is_some_and(|multi| multi.auto_export && !multi.exported);
        if !should {
            return;
        }
        self.export_multi_report();
        if let Some(multi) = self.multi.as_mut() {
            multi.exported = true;
        }
    }

    fn export_multi_report(&mut self) {
        let Some(multi) = self.multi.as_mut() else {
            return;
        };
        let kind = if multi.kind == TestKind::Speed {
            "speed"
        } else {
            "quality"
        };
        let rows: Vec<MultiReportRow> = if multi.kind == TestKind::Speed {
            let mut rows = Vec::new();
            for task in &multi.tasks {
                if let Some(suite) = &task.suite {
                    for item in &suite.results {
                        let preset = PRESETS[item.preset_index];
                        rows.push(MultiReportRow {
                            target: task.label.clone(),
                            mode: preset.name.to_string(),
                            status: if item.failed() {
                                "failed".to_string()
                            } else {
                                task.status.code().to_string()
                            },
                            passes: 1,
                            passed: u32::from(!item.failed()),
                            failed: u32::from(item.failed()),
                            wrote_bytes: item.write.total_bytes,
                            read_bytes: item.read.total_bytes,
                            data_errors: item.verify_errors,
                            read_errors: item.read_errors + item.write_errors,
                            temperature_c: task.health.temperature_c,
                            health: task.health.status.label().to_string(),
                            read_mibps: item.read.read_mibps,
                            write_mibps: item.write.write_mibps,
                            read_iops: item.read.read_iops,
                            write_iops: item.write.write_iops,
                            avg_latency_ns: (item.read.avg_latency_ns + item.write.avg_latency_ns)
                                / 2.0,
                            speed_errors: item.read_errors + item.write_errors + item.verify_errors,
                        });
                    }
                }
            }
            rows
        } else {
            multi
                .tasks
                .iter()
                .map(|task| {
                    let summary = task.summary.as_ref();
                    MultiReportRow {
                        target: task.label.clone(),
                        mode: match task.mode {
                            RunMode::WriteVerify => "write+verify".to_string(),
                            RunMode::WriteOnly => "write".to_string(),
                            RunMode::VerifyExisting => "verify".to_string(),
                        },
                        status: task.status.code().to_string(),
                        passes: summary.map(|value| value.passes).unwrap_or(0),
                        passed: summary.map(|value| value.passed).unwrap_or(0),
                        failed: summary.map(|value| value.failed).unwrap_or(0),
                        wrote_bytes: summary.map(|value| value.wrote_bytes).unwrap_or(0),
                        read_bytes: summary.map(|value| value.read_bytes).unwrap_or(0),
                        data_errors: summary.map(|value| value.data_errors).unwrap_or(0),
                        read_errors: summary.map(|value| value.read_errors).unwrap_or(0),
                        temperature_c: task.health.temperature_c,
                        health: task.health.status.label().to_string(),
                        read_mibps: 0.0,
                        write_mibps: 0.0,
                        read_iops: 0.0,
                        write_iops: 0.0,
                        avg_latency_ns: 0.0,
                        speed_errors: 0,
                    }
                })
                .collect()
        };
        let directory = export_directory().join("multi");
        multi.export_message = match export::export_multi(&directory, kind, &rows) {
            Ok(files) => format!(
                "exported {} file(s) to {}",
                files.len(),
                directory.display()
            ),
            Err(error) => format!("export failed: {error}"),
        };
    }

    fn render_speed_settings(&self, frame: &mut Frame) {
        let area = centered_rect(frame.area(), 70, 64);
        frame.render_widget(Clear, area);
        let values = &self.speed.values;
        let mut lines: Vec<Line> = Vec::new();
        for (index, preset) in PRESETS.iter().enumerate() {
            let focused = self.speed.focus == index;
            let marker = if focused { "▶ " } else { "  " };
            let check = if values.presets[index] { "[x]" } else { "[ ]" };
            let style = if focused {
                self.theme.selected()
            } else if values.presets[index] {
                self.theme.text()
            } else {
                self.theme.dim()
            };
            lines.push(Line::styled(
                format!("{marker}{check} {}", preset.name),
                style,
            ));
        }
        lines.push(Line::from(""));
        lines.push(self.settings_line(
            "测试文件大小",
            format!("◀ {} ▶", format_bytes(values.file_size)),
            self.speed.focus == 4,
        ));
        lines.push(self.settings_line(
            "顺序项错误继续",
            format!("◀ {} ▶", yes_no(values.ignore_errors)),
            self.speed.focus == 5,
        ));
        lines.push(Line::from(""));
        let block = Block::bordered()
            .title(format!(
                " CDM 速度测试（{} 个盘） ",
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
        for index in 0..4 {
            self.record_hotspot(
                Rect::new(
                    area.x + 1,
                    area.y + 1 + index as u16,
                    area.width.saturating_sub(2),
                    1,
                ),
                MouseAction::FocusSpeed(index),
            );
        }
        self.record_hotspot(
            Rect::new(area.x + 1, area.y + 1 + 5, area.width.saturating_sub(2), 1),
            MouseAction::FocusSpeed(4),
        );
        self.record_hotspot(
            Rect::new(area.x + 1, area.y + 1 + 6, area.width.saturating_sub(2), 1),
            MouseAction::FocusSpeed(5),
        );
        self.render_button_row(frame, chunks[1], "Enter 开始      Esc 返回");
    }

    fn mouse_event(&mut self, event: MouseEvent) -> bool {
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

    fn record_hotspot(&self, rect: Rect, action: MouseAction) {
        self.hotspots.borrow_mut().push(Hotspot { rect, action });
    }

    fn record_multi_hotspots(&self, tabs: Rect, footer: Rect, count: usize) {
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

    fn wheel(&mut self, up: bool) {
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
            Screen::Results => {
                let code = if up {
                    KeyCode::Char('[')
                } else {
                    KeyCode::Char(']')
                };
                self.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
            }
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
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
                        results.refresh();
                    }
                    KeyCode::Char('[') => {
                        match results.params.mode {
                            ThresholdMode::Percent => {
                                results.params.percent = (results.params.percent - 5.0).max(10.0)
                            }
                            ThresholdMode::Absolute => {
                                results.params.absolute_mibps =
                                    (results.params.absolute_mibps - 10.0).max(0.0)
                            }
                        }
                        results.refresh();
                    }
                    KeyCode::Char(']') => {
                        match results.params.mode {
                            ThresholdMode::Percent => {
                                results.params.percent = (results.params.percent + 5.0).min(100.0)
                            }
                            ThresholdMode::Absolute => results.params.absolute_mibps += 10.0,
                        }
                        results.refresh();
                    }
                    KeyCode::Char('e') => {
                        results.export();
                    }
                    _ => {}
                }
            }
        }
        false
    }

    fn settings_toggle(&mut self, forward: bool) {
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
            SettingsField::Passes => {}
        }
    }

    fn settings_accept(&mut self) {
        self.commit_passes();
        self.screen = Screen::Confirm;
    }

    fn focus_field(&mut self, field: SettingsField) {
        if self.settings.field == SettingsField::Passes {
            self.commit_passes();
        }
        self.settings.field = field;
        if field == SettingsField::Passes {
            self.settings.passes_input.clear();
        }
    }

    fn commit_passes(&mut self) {
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

    fn render(&self, frame: &mut Frame) {
        self.hotspots.borrow_mut().clear();
        let area = frame.area();
        if area.width < 100 || area.height < 26 {
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
    fn refresh(&mut self) {
        self.analysis = analysis::analyze(&self.summary.trace, &self.params);
    }

    fn mode_label(&self) -> String {
        match self.params.mode {
            ThresholdMode::Percent => format!("percent {:.0}%", self.params.percent),
            ThresholdMode::Absolute => format!("absolute {:.0} MiB/s", self.params.absolute_mibps),
        }
    }

    fn export(&mut self) {
        let directory = export_directory();
        let input = ExportInput {
            target: &self.target,
            summary: &self.summary,
            analysis: &self.analysis,
            params: &self.params,
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

#[cfg(test)]
mod tests {
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
        assert!(text.contains("speedMiB/s"));
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
        assert!(text.contains("speedMiB/s"));
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
            token: CancelToken::new(),
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
}
