//! Run lifecycle, health refresh, and export orchestration.

use super::*;
use crate::png;

impl App {
    pub(super) fn selected_volume(&self) -> Option<&VolumeInfo> {
        self.volumes.get(self.selected)
    }

    pub(super) fn report_temperature_supported(&self) -> bool {
        self.target_volumes().first().is_some_and(|volume| {
            self.cached_health(&volume.mount_point)
                .temperature_c
                .is_some()
        })
    }

    pub(super) fn rescan(&mut self) {
        if self.rescan_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(volume::list_volumes());
        });
        self.rescan_rx = Some(rx);
    }

    fn apply_rescan(&mut self, volumes: Vec<VolumeInfo>) {
        let current = self
            .volumes
            .get(self.selected)
            .map(|volume| volume.mount_point.clone());
        let old: Vec<std::path::PathBuf> = self
            .volumes
            .iter()
            .map(|volume| volume.mount_point.clone())
            .collect();
        self.volumes = volumes;
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

    fn update_health_focus(&self) {
        let mut targets = Vec::new();
        if let Some(volume) = self.volumes.get(self.selected) {
            targets.push(volume.mount_point.clone());
        }
        if let Some(run) = &self.run {
            targets.push(run.target_path.clone());
        }
        if let Some(multi) = &self.multi {
            for task in &multi.tasks {
                if task.status == TaskStatus::Running {
                    targets.push(task.target_path.clone());
                }
            }
        }
        self.health_cache.set_focus(targets);
    }

    pub(super) fn cached_health(&self, mount: &std::path::Path) -> DriveHealth {
        self.health_cache.get(mount)
    }

    pub(super) fn tick(&mut self) {
        if let Some(rx) = self.rescan_rx.take() {
            match rx.try_recv() {
                Ok(volumes) => self.apply_rescan(volumes),
                Err(mpsc::TryRecvError::Empty) => self.rescan_rx = Some(rx),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        self.update_health_focus();
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
                    run.view.health = health;
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
                        task.view.health = health;
                    }
                }
            }
            self.last_active_health_refresh = Instant::now();
        }
        let mut finished: Option<(Summary, bool, bool, bool, PathBuf)> = None;
        let mut failed: Option<String> = None;
        if let Some(run) = self.run.as_mut() {
            while let Ok(event) = run.rx.try_recv() {
                match event {
                    TuiEvent::PassStart { pass, passes } => {
                        run.view.log.push_back(format!("pass {pass}/{passes}"));
                    }
                    TuiEvent::Info(message) | TuiEvent::Warn(message) => {
                        run.view.log.push_back(message);
                    }
                    TuiEvent::Error(message) => {
                        run.view.log.push_back(message);
                    }
                    TuiEvent::PhaseStart { phase } => {
                        run.view.phase = Some(phase);
                        run.view.speed_history.clear();
                        run.view.log.push_back(format!("{} started", phase.label()));
                    }
                    TuiEvent::Progress(progress) => {
                        run.view.latest = Some(progress);
                        run.view.total = progress.total_bytes;
                        run.view
                            .speed_history
                            .push((progress.bytes, progress.current_mibps));
                    }
                    TuiEvent::PhaseDone { phase, stats } => {
                        run.view.log.push_back(format!(
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
                            run.report_temperature,
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
            while run.view.log.len() > 50 {
                run.view.log.pop_front();
            }
        }
        if let Some((summary, delete_after, auto_export, report_temperature, target)) = finished {
            self.finish_run(
                summary,
                delete_after,
                auto_export,
                report_temperature,
                target,
            );
        } else if let Some(error) = failed {
            self.message = error;
            self.screen = Screen::Home;
            self.run = None;
            self.rescan();
        }
        if let Some(rx) = self.export_rx.take() {
            match rx.try_recv() {
                Ok(message) => {
                    if let Some(results) = self.results.as_mut() {
                        results.message = format!("{}; {}", results.message, message);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => self.export_rx = Some(rx),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        if let Some(rx) = self.multi_export_rx.take() {
            match rx.try_recv() {
                Ok(message) => {
                    if let Some(multi) = self.multi.as_mut() {
                        multi.export_message = message;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => self.multi_export_rx = Some(rx),
                Err(mpsc::TryRecvError::Disconnected) => {}
            }
        }
        let mut analysis_finished = false;
        if let Some(rx) = self.analysis_rx.take() {
            match rx.try_recv() {
                Ok((generation, analysis)) => {
                    if let Some(results) = self.results.as_mut()
                        && results.generation == generation
                    {
                        results.analysis = Some(analysis);
                        results.message = "thresholds recalculated".to_string();
                    }
                    analysis_finished = true;
                }
                Err(mpsc::TryRecvError::Empty) => self.analysis_rx = Some(rx),
                Err(mpsc::TryRecvError::Disconnected) => analysis_finished = true,
            }
        }
        if analysis_finished && self.analysis_pending {
            self.analysis_pending = false;
            self.refresh_results_async();
        }
        if self.screen == Screen::MultiRun || self.multi.is_some() {
            self.tick_multi();
        }
    }

    pub(super) fn refresh_results_async(&mut self) {
        if self.analysis_rx.is_some() {
            self.analysis_pending = true;
            return;
        }
        let Some(results) = &self.results else {
            return;
        };
        let summary = results.summary.clone();
        let params = results.params;
        self.analysis_generation += 1;
        let generation = self.analysis_generation;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let analysis = analysis::analyze(&summary.trace, &params);
            let _ = tx.send((generation, analysis));
        });
        self.analysis_rx = Some(rx);
        if let Some(results) = self.results.as_mut() {
            results.generation = generation;
            results.message = "recalculating thresholds...".to_string();
        }
    }

    pub(super) fn export_results_async(&mut self) {
        let Some(results) = &self.results else {
            return;
        };
        let directory = export_directory_for(&results.target);
        let target = results.target.clone();
        let summary = results.summary.clone();
        let params = results.params;
        let include_temperature = results.include_temperature;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let analysis = analysis::analyze(&summary.trace, &params);
            let input = ExportInput {
                target: &target,
                summary: &summary,
                analysis: &analysis,
                params: &params,
                include_temperature,
            };
            let message = match export::export(&directory, &input) {
                Ok(files) => format!(
                    "exported {} file(s) to {}",
                    files.len(),
                    directory.display()
                ),
                Err(error) => format!("export failed: {error}"),
            };
            let _ = tx.send(message);
        });
        self.export_rx = Some(rx);
        if let Some(results) = self.results.as_mut() {
            results.message = "export started in background".to_string();
        }
    }

    pub(super) fn finish_run(
        &mut self,
        summary: Summary,
        delete_after: bool,
        auto_export: bool,
        report_temperature: bool,
        target: PathBuf,
    ) {
        let params = AnalysisParams::default();
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
            let directory = export_directory_for(&target);
            let export_target = target.clone();
            let export_summary = summary.clone();
            let export_params = params;
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let export_analysis = analysis::analyze(&export_summary.trace, &export_params);
                let input = ExportInput {
                    target: &export_target,
                    summary: &export_summary,
                    analysis: &export_analysis,
                    params: &export_params,
                    include_temperature: report_temperature,
                };
                let result = match export::export(&directory, &input) {
                    Ok(files) => format!(
                        "exported {} file(s) to {}",
                        files.len(),
                        directory.display()
                    ),
                    Err(error) => format!("export failed: {error}"),
                };
                let _ = tx.send(result);
            });
            self.export_rx = Some(rx);
            message = "export started in background".to_string();
        }
        if delete_after && success {
            match format::delete_test_files(&target) {
                Ok(count) => {
                    message.push_str(&format!("; deleted {count} test file(s)"));
                }
                Err(error) => message.push_str(&format!("; delete failed: {error}")),
            }
        }
        let analysis_summary = summary.clone();
        self.analysis_generation += 1;
        let generation = self.analysis_generation;
        let (analysis_tx, analysis_rx) = mpsc::channel();
        thread::spawn(move || {
            let analysis = analysis::analyze(&analysis_summary.trace, &params);
            let _ = analysis_tx.send((generation, analysis));
        });
        self.analysis_rx = Some(analysis_rx);
        self.results = Some(ResultsState {
            target,
            summary,
            analysis: None,
            generation,
            params,
            axis: Axis::Position,
            message,
            include_temperature: report_temperature,
        });
        self.screen = Screen::Results;
        completion_notification(if success {
            "OpenURWTest - QC 完成"
        } else {
            "OpenURWTest - QC 有错误"
        });
        self.run = None;
    }

    pub(super) fn open_settings(&mut self, mode: RunMode) {
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

    pub(super) fn launch_run(&mut self, settings: Settings) {
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
        let pause = PauseToken::new();
        let worker_pause = pause.clone();
        let (tx, rx) = mpsc::channel();
        let health = self.cached_health(&target);
        let target_for_thread = target.clone();
        let panic_tx = tx.clone();
        thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut reporter = TuiReporter::single(tx);
                let result = match mode {
                    RunMode::VerifyExisting => engine::verify_existing_with_control(
                        &target_for_thread,
                        Some(&worker_token),
                        Some(&worker_pause),
                        &mut reporter,
                    ),
                    RunMode::WriteOnly | RunMode::WriteVerify => {
                        let options = RunOptions {
                            target: &target_for_thread,
                            passes,
                            verify: mode == RunMode::WriteVerify,
                            continue_on_error,
                            reuse_existing: false,
                            cancel: Some(worker_token.clone()),
                            pause: Some(worker_pause.clone()),
                        };
                        engine::run_os(&options, &mut reporter)
                    }
                };
                if let Err(error) = result {
                    reporter.failed(format!("{error:#}"));
                }
            }));
            if outcome.is_err() {
                let _ = panic_tx.send(TuiEvent::Failed(
                    "worker thread panicked; test data may remain on the volume".to_string(),
                ));
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
            report_temperature: settings.report_temperature,
            control: TaskControl::new(token.clone(), pause.clone()),
            view: TaskViewState::new(health),
            started: Instant::now(),
            cancel_requested: false,
        });
        self.screen = Screen::Run;
    }

    pub(super) fn target_volumes(&self) -> Vec<VolumeInfo> {
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

    pub(super) fn toggle_check(&mut self) {
        let Some(volume) = self.selected_volume() else {
            return;
        };
        let path = volume.mount_point.clone();
        if !self.checked.remove(&path) {
            self.checked.insert(path);
        }
    }

    pub(super) fn select_all(&mut self) {
        self.checked = self
            .volumes
            .iter()
            .map(|volume| volume.mount_point.clone())
            .collect();
    }

    pub(super) fn clear_checks(&mut self) {
        self.checked.clear();
    }

    pub(super) fn selected_is_system_volume(&self, volume: &VolumeInfo) -> bool {
        volume.mount_point.join("Windows").exists()
    }

    pub(super) fn needs_system_confirmation(&self) -> bool {
        let active =
            self.kind == TestKind::Quality && self.settings.values.mode != RunMode::VerifyExisting;
        active
            && self
                .target_volumes()
                .iter()
                .any(|volume| self.selected_is_system_volume(volume))
    }

    pub(super) fn launch_selected(&mut self, settings: Settings) {
        if self.checked.is_empty() {
            self.launch_run(settings);
        } else {
            self.launch_multi(settings);
        }
    }

    pub(super) fn launch_multi(&mut self, settings: Settings) {
        let targets = self.target_volumes();
        if targets.is_empty() {
            return;
        }
        cancel::reset();
        let (tx, rx) = mpsc::channel();
        let mut tasks = Vec::with_capacity(targets.len());
        for (index, volume) in targets.iter().enumerate() {
            let token = CancelToken::new();
            let pause = PauseToken::new();
            let target = volume.mount_point.clone();
            let thread_target = target.clone();
            let thread_token = token.clone();
            let thread_pause = pause.clone();
            let tx = tx.clone();
            let mode = settings.mode;
            let passes = settings.passes;
            let continue_on_error = settings.continue_on_error;
            let health = self.cached_health(&volume.mount_point);
            let panic_tx = tx.clone();
            thread::spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut reporter = TuiReporter::multi(index, tx);
                    let result = match mode {
                        RunMode::VerifyExisting => engine::verify_existing_with_control(
                            &thread_target,
                            Some(&thread_token),
                            Some(&thread_pause),
                            &mut reporter,
                        ),
                        RunMode::WriteOnly | RunMode::WriteVerify => {
                            let options = RunOptions {
                                target: &thread_target,
                                passes,
                                verify: mode == RunMode::WriteVerify,
                                continue_on_error,
                                reuse_existing: false,
                                cancel: Some(thread_token),
                                pause: Some(thread_pause),
                            };
                            engine::run_os(&options, &mut reporter)
                        }
                    };
                    if let Err(error) = result {
                        reporter.failed(format!("{error:#}"));
                    }
                }));
                if outcome.is_err() {
                    let _ = panic_tx.send((
                        index,
                        TuiEvent::Failed(
                            "worker thread panicked; test data may remain on the volume"
                                .to_string(),
                        ),
                    ));
                }
            });
            tasks.push(MultiTask {
                label: volume.display_name(),
                target_path: target.clone(),
                mode,
                control: TaskControl::new(token, pause),
                view: TaskViewState::new(health),
                status: TaskStatus::Queued,
                summary: None,
                suite: None,
                error: None,
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
            delete_after: settings.delete_after,
            exported: false,
            deleted: false,
            notified: false,
            results_scroll: 0,
        });
        self.screen = Screen::MultiRun;
    }

    pub(super) fn tick_multi(&mut self) {
        let mut finished_all = false;
        if let Some(multi) = self.multi.as_mut() {
            while let Ok((index, event)) = multi.rx.try_recv() {
                let Some(task) = multi.tasks.get_mut(index) else {
                    continue;
                };
                match event {
                    TuiEvent::PassStart { pass, passes } => {
                        task.status = TaskStatus::Running;
                        task.view.log.push_back(format!("pass {pass}/{passes}"));
                    }
                    TuiEvent::Info(message)
                    | TuiEvent::Warn(message)
                    | TuiEvent::Error(message) => {
                        task.view.log.push_back(message);
                    }
                    TuiEvent::PhaseStart { phase } => {
                        task.status = TaskStatus::Running;
                        task.view.phase = Some(phase);
                        task.view.speed_history.clear();
                        task.view
                            .log
                            .push_back(format!("{} started", phase.label()));
                    }
                    TuiEvent::Progress(progress) => {
                        task.status = TaskStatus::Running;
                        task.view.latest = Some(progress);
                        task.view.total = progress.total_bytes;
                        task.view
                            .speed_history
                            .push((progress.bytes, progress.current_mibps));
                    }
                    TuiEvent::PhaseDone { phase, stats } => {
                        task.view.log.push_back(format!(
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
                        match *event {
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
                                task.view.log.push_back(format!(
                                    "开始 {}",
                                    PRESETS.get(index).map(|item| item.name).unwrap_or("-")
                                ));
                            }
                            SuiteEvent::PhaseStarted { index, phase } => {
                                suite.current = index;
                                suite.phase = Some(phase);
                                suite.done_bytes = 0;
                                suite.mibps = 0.0;
                                task.view.log.push_back(format!(
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
                                        if let Some(curve) = suite.write_curves.get_mut(index) {
                                            curve.push((done_bytes, mibps));
                                        }
                                        if let Some(samples) = suite.write_samples.get_mut(index) {
                                            samples.push(mibps);
                                        }
                                    }
                                    SuitePhase::Verify => {
                                        suite.read_history.push((done_bytes, mibps));
                                        if let Some(curve) = suite.read_curves.get_mut(index) {
                                            curve.push((done_bytes, mibps));
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
                                task.view.log.push_back(format!(
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
                                suite.results.push(result.clone());
                                task.view.log.push_back(format!(
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
                        } else if result.stopped || result.error.is_some() {
                            TaskStatus::Failed
                        } else {
                            TaskStatus::Done
                        };
                        if let Some(error) = &result.error {
                            task.error = Some(error.clone());
                            task.view.log.push_back(error.clone());
                        }
                        if result.stopped {
                            self.error_flash_start = Some(Instant::now());
                            let offset = result
                                .stopped_at
                                .map(format_bytes)
                                .unwrap_or_else(|| "未知位置".to_string());
                            task.error = Some(format!(
                                "顺序项校验发现文件损坏（写入量 {offset} 处），速度测试已停止"
                            ));
                            task.view
                                .log
                                .push_back(task.error.clone().unwrap_or_default());
                        }
                    }
                    TuiEvent::Failed(error) => {
                        task.error = Some(error);
                        task.status = TaskStatus::Failed;
                    }
                }
                while task.view.log.len() > 30 {
                    task.view.log.pop_front();
                }
            }
            finished_all = multi.all_done();
        }
        if finished_all {
            if let Some(multi) = self.multi.as_mut()
                && !multi.notified
            {
                multi.notified = true;
                let success = multi
                    .tasks
                    .iter()
                    .all(|task| task.status == TaskStatus::Done && task.error.is_none());
                if success
                    && multi.delete_after
                    && multi.kind == TestKind::Quality
                    && !multi.deleted
                {
                    multi.deleted = true;
                    let targets: Vec<PathBuf> = multi
                        .tasks
                        .iter()
                        .map(|task| task.target_path.clone())
                        .collect();
                    let mut deleted = 0u32;
                    let mut errors = Vec::new();
                    for target in targets {
                        match format::delete_test_files(&target) {
                            Ok(count) => deleted += count,
                            Err(error) => errors.push(format!("{}: {error}", target.display())),
                        }
                    }
                    multi.export_message = format!("deleted {deleted} test file(s)");
                    if !errors.is_empty() {
                        multi
                            .export_message
                            .push_str(&format!("; delete failed: {}", errors.join("; ")));
                    }
                }
                completion_notification(if success {
                    "OpenURWTest - 多盘完成"
                } else {
                    "OpenURWTest - 多盘有错误"
                });
            }
            self.maybe_auto_export_multi();
            self.screen = Screen::MultiResults;
        }
    }

    pub(super) fn cancel_multi(&mut self, all: bool) {
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
                    task.control.cancel.cancel();
                }
            }
        }
        if needs_global && (all || active_is_verify) {
            cancel::request();
        }
    }

    pub(super) fn handle_multi_key(&mut self, key: KeyEvent) {
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
            KeyCode::Char('p') => {
                let active = multi.active.min(multi.tasks.len().saturating_sub(1));
                if let Some(task) = multi.tasks.get_mut(active) {
                    task.control.pause.toggle();
                }
            }
            KeyCode::Char('P') => {
                let pause = !multi
                    .tasks
                    .iter()
                    .all(|task| task.control.pause.is_paused());
                for task in &mut multi.tasks {
                    if pause {
                        task.control.pause.pause();
                    } else {
                        task.control.pause.resume();
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn selected_speed_presets(&self) -> Vec<crate::speed::suite::SpeedPreset> {
        PRESETS
            .iter()
            .enumerate()
            .filter(|(index, _)| self.speed.values.presets[*index])
            .map(|(_, preset)| *preset)
            .collect()
    }

    pub(super) fn cycle_speed_file_size(&mut self, forward: bool) {
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

    pub(super) fn handle_speed_settings(&mut self, key: KeyEvent) {
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

    pub(super) fn handle_speed_confirm(&mut self, key: KeyEvent) {
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

    pub(super) fn launch_speed_multi(&mut self) {
        let targets = self.target_volumes();
        if targets.is_empty() {
            return;
        }
        let presets = self.selected_speed_presets();
        if presets.is_empty() {
            self.message = "请至少选择一个测试项目".to_string();
            return;
        }
        let required = self.speed.values.file_size.saturating_add(64 * 1024 * 1024);
        for volume in &targets {
            if let Ok(free) = volume::free_space(&volume.mount_point)
                && free < required
            {
                self.message = format!(
                    "{} 可用空间不足：{} / {}",
                    volume.mount_point.display(),
                    format_bytes(free),
                    format_bytes(required)
                );
                return;
            }
        }
        cancel::reset();
        let settings = self.speed.values;
        let (tx, rx) = mpsc::channel();
        let mut tasks = Vec::with_capacity(targets.len());
        for (index, volume) in targets.iter().enumerate() {
            let token = CancelToken::new();
            let pause = PauseToken::new();
            let target = volume.mount_point.clone();
            let thread_target = target.clone();
            let tx = tx.clone();
            let token_for_thread = token.clone();
            let thread_pause = pause.clone();
            let presets = presets.clone();
            let health = self.cached_health(&volume.mount_point);
            let panic_tx = tx.clone();
            thread::spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let options = SuiteOptions {
                        file_size: settings.file_size,
                        ignore_sequential_errors: settings.ignore_errors,
                        seed: 0x5EED_1234_ABCD_0001,
                        pause: Some(thread_pause),
                    };
                    let tx_events = tx.clone();
                    let result = run_suite(
                        &thread_target,
                        &presets,
                        options,
                        token_for_thread.flag(),
                        &mut |event| {
                            let _ = tx_events.send((index, TuiEvent::Suite(Box::new(event))));
                        },
                    );
                    let _ = std::fs::remove_file(
                        thread_target.join(crate::speed::suite::bench_file_name()),
                    );
                    match result {
                        Ok(result) => {
                            let _ = tx.send((index, TuiEvent::SuiteFinished(Box::new(result))));
                        }
                        Err(error) => {
                            let _ = tx.send((index, TuiEvent::Failed(format!("{error:#}"))));
                        }
                    }
                }));
                if outcome.is_err() {
                    let _ = panic_tx.send((
                        index,
                        TuiEvent::Failed(
                            "speed worker panicked; benchmark file may remain".to_string(),
                        ),
                    ));
                }
            });
            tasks.push(MultiTask {
                label: volume.display_name(),
                target_path: target.clone(),
                mode: RunMode::WriteOnly,
                control: TaskControl::new(token, pause),
                view: TaskViewState::new(health),
                status: TaskStatus::Running,
                summary: None,
                suite: Some(SuiteView::default()),
                error: None,
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
            delete_after: false,
            exported: false,
            deleted: false,
            notified: false,
            results_scroll: 0,
        });
        self.screen = Screen::MultiRun;
    }

    pub(super) fn maybe_auto_export_multi(&mut self) {
        let should = self
            .multi
            .as_ref()
            .is_some_and(|multi| multi.auto_export && !multi.exported);
        if !should {
            return;
        }
        let Some((kind, rows)) = self.multi_report_rows() else {
            return;
        };
        let target = self
            .multi
            .as_ref()
            .and_then(|multi| multi.tasks.first())
            .map(|task| task.target_path.clone())
            .unwrap_or_default();
        let directory = export_directory_for(&target).join("multi");
        let png_inputs = self.speed_png_inputs();
        let quality_inputs = self.quality_png_inputs();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut count = 0usize;
            let mut error = None;
            match export::export_multi(&directory, kind, &rows) {
                Ok(files) => count += files.len(),
                Err(value) => error = Some(format!("report export failed: {value}")),
            }
            for (target, result) in &png_inputs {
                match png_speed::write_speed_pngs(&directory, target, result, false) {
                    Ok(files) => count += files.len(),
                    Err(value) => error = Some(format!("PNG export failed: {value}")),
                }
            }
            for (target, summary) in &quality_inputs {
                let params = AnalysisParams::default();
                let analysis = analysis::analyze(&summary.trace, &params);
                let path = directory.join(png::quality_file_name(target));
                match png::write_png(&path, target, &analysis, &summary.temperature_history) {
                    Ok(()) => count += 1,
                    Err(value) => error = Some(format!("PNG export failed: {value}")),
                }
            }
            let message = if let Some(error) = error {
                error
            } else {
                format!("exported {count} file(s) to {}", directory.display())
            };
            let _ = tx.send(message);
        });
        self.multi_export_rx = Some(rx);
        if let Some(multi) = self.multi.as_mut() {
            multi.exported = true;
            multi.export_message = "export started in background".to_string();
        }
    }
    pub(super) fn export_multi_report(&mut self) {
        let Some((kind, rows)) = self.multi_report_rows() else {
            return;
        };
        let target = self
            .multi
            .as_ref()
            .and_then(|multi| multi.tasks.first())
            .map(|task| task.target_path.clone())
            .unwrap_or_default();
        let directory = export_directory_for(&target).join("multi");
        let mut count = 0usize;
        let mut error = None;
        match export::export_multi(&directory, kind, &rows) {
            Ok(files) => count += files.len(),
            Err(value) => error = Some(format!("report export failed: {value}")),
        }
        for (target, result) in self.speed_png_inputs() {
            match png_speed::write_speed_pngs(&directory, &target, &result, false) {
                Ok(files) => count += files.len(),
                Err(value) => error = Some(format!("PNG export failed: {value}")),
            }
        }
        for (target, summary) in self.quality_png_inputs() {
            let params = AnalysisParams::default();
            let analysis = analysis::analyze(&summary.trace, &params);
            let path = directory.join(png::quality_file_name(&target));
            match png::write_png(&path, &target, &analysis, &summary.temperature_history) {
                Ok(()) => count += 1,
                Err(value) => error = Some(format!("PNG export failed: {value}")),
            }
        }
        let message = if let Some(error) = error {
            error
        } else {
            format!("exported {count} file(s) to {}", directory.display())
        };
        if let Some(multi) = self.multi.as_mut() {
            multi.export_message = message;
        }
    }

    fn quality_png_inputs(&self) -> Vec<(PathBuf, Summary)> {
        let Some(multi) = self.multi.as_ref() else {
            return Vec::new();
        };
        if multi.kind != TestKind::Quality {
            return Vec::new();
        }
        multi
            .tasks
            .iter()
            .filter_map(|task| {
                task.summary
                    .clone()
                    .map(|summary| (task.target_path.clone(), summary))
            })
            .collect()
    }

    fn speed_png_inputs(&self) -> Vec<(PathBuf, SuiteResult)> {
        let Some(multi) = self.multi.as_ref() else {
            return Vec::new();
        };
        if multi.kind != TestKind::Speed {
            return Vec::new();
        }
        multi
            .tasks
            .iter()
            .filter_map(|task| {
                let suite = task.suite.as_ref()?;
                if suite.results.is_empty() {
                    return None;
                }
                let result = SuiteResult {
                    file_size: suite.total_bytes.max(
                        suite
                            .results
                            .iter()
                            .flat_map(|item| item.write_curve.iter().chain(item.read_curve.iter()))
                            .map(|(bytes, _)| *bytes)
                            .max()
                            .unwrap_or(0),
                    ),
                    presets: suite.results.clone(),
                    ..SuiteResult::default()
                };
                Some((task.target_path.clone(), result))
            })
            .collect()
    }

    pub(super) fn multi_report_rows(&self) -> Option<(&'static str, Vec<MultiReportRow>)> {
        let multi = self.multi.as_ref()?;
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
                            temperature_c: task.view.health.temperature_c,
                            health: task.view.health.status.label().to_string(),
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
                        temperature_c: task.view.health.temperature_c,
                        health: task.view.health.status.label().to_string(),
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
        Some((kind, rows))
    }
    pub(super) fn render_speed_settings(&self, frame: &mut Frame) {
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
            "错误中断",
            format!("◀ {} ▶", yes_no(!values.ignore_errors)),
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
}
