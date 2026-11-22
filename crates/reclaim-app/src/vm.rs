//! View models: the whole UI state as plain Rust, fed by engine events and user intents,
//! read by the toolkit adapter (ARCHITECTURE §8.1). No toolkit types, no OS calls, so
//! every screen's behaviour is tested on any platform.

use std::collections::BTreeSet;

use reclaim_core::model::{Capabilities, Pressure, ProcessKey, SystemSnapshot};
use reclaim_core::outcome::{
    Completion, FailureKind, FollowUpPoint, ProcessOutcome, RunMode, RunRecord,
};
use reclaim_core::plan::{Plan, Selection};
use reclaim_core::report::Tone;
use reclaim_core::units::{Bytes, Millis};
use reclaim_engine::{Command, Event, Finished, Row, Settings, View};

use crate::format::{date_time, opt_bytes, percent, permille};
use crate::strings::{pressure, tr, trf};

/// Top-level screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    /// System memory and the main action.
    #[default]
    Overview,
    /// Process table with eligibility.
    Processes,
    /// Preferences.
    Settings,
    /// Version, license and attributions.
    About,
}

impl Screen {
    /// All screens in navigation order.
    pub const ALL: [Screen; 4] = [
        Screen::Overview,
        Screen::Processes,
        Screen::Settings,
        Screen::About,
    ];

    /// Navigation index.
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Screen at a navigation index.
    pub fn from_index(i: usize) -> Screen {
        Self::ALL.get(i).copied().unwrap_or_default()
    }

    fn view(self) -> View {
        match self {
            Screen::Processes => View::Processes,
            // Nothing on these screens shows live values: sample slowly.
            Screen::Settings | Screen::About => View::Background,
            Screen::Overview => View::Overview,
        }
    }
}

/// Which parts of the UI need refreshing after a change. The adapter pushes only these,
/// so an unchanged screen costs nothing (idle CPU, ACCEPTANCE §2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dirty {
    /// Overview values.
    pub overview: bool,
    /// Process table.
    pub processes: bool,
    /// Confirmation dialog.
    pub plan: bool,
    /// Result screen.
    pub result: bool,
    /// History list.
    pub history: bool,
    /// Settings form.
    pub settings: bool,
    /// Tray tooltip.
    pub tray: bool,
    /// Warning banner.
    pub warning: bool,
    /// Current screen.
    pub screen: bool,
}

impl Dirty {
    /// Everything (first render, window recreated).
    pub fn all() -> Self {
        Self {
            overview: true,
            processes: true,
            plan: true,
            result: true,
            history: true,
            settings: true,
            tray: true,
            warning: true,
            screen: true,
        }
    }

    /// Whether anything changed.
    pub fn any(&self) -> bool {
        *self != Self::default()
    }

    fn merge(&mut self, o: Dirty) {
        self.overview |= o.overview;
        self.processes |= o.processes;
        self.plan |= o.plan;
        self.result |= o.result;
        self.history |= o.history;
        self.settings |= o.settings;
        self.tray |= o.tray;
        self.warning |= o.warning;
        self.screen |= o.screen;
    }
}

/// What an event or intent asks of the adapter.
#[derive(Debug, Default)]
pub struct Reaction {
    /// Parts to refresh.
    pub dirty: Dirty,
    /// Commands to send to the engine.
    pub commands: Vec<Command>,
}

impl Reaction {
    fn dirty(d: Dirty) -> Self {
        Self {
            dirty: d,
            commands: Vec::new(),
        }
    }

    fn command(mut self, c: Command) -> Self {
        self.commands.push(c);
        self
    }
}

/// Process table columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Column {
    /// Image name.
    Name,
    /// PID.
    Pid,
    /// Working set.
    #[default]
    WorkingSet,
    /// Private bytes (commit).
    PrivateBytes,
    /// Eligibility.
    Status,
}

impl Column {
    /// Column from its table index.
    pub fn from_index(i: usize) -> Column {
        match i {
            0 => Column::Name,
            1 => Column::Pid,
            2 => Column::WorkingSet,
            3 => Column::PrivateBytes,
            _ => Column::Status,
        }
    }

    /// Table index of the column (inverse of [`Column::from_index`]).
    pub fn index(self) -> usize {
        match self {
            Column::Name => 0,
            Column::Pid => 1,
            Column::WorkingSet => 2,
            Column::PrivateBytes => 3,
            Column::Status => 4,
        }
    }
}

/// What the tray icon shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrayState {
    /// Normal or low memory pressure.
    #[default]
    Normal,
    /// The OS low-memory signal is set.
    High,
    /// A run or its follow-up measurement is in progress.
    Working,
    /// Trimming is not supported, or no memory sample yet.
    Unavailable,
}

/// Run progress as the UI sees it.
#[derive(Debug, Clone, Default)]
pub enum Stage {
    /// Nothing in progress.
    #[default]
    Idle,
    /// A plan was requested.
    Planning,
    /// Targets are being processed.
    Running {
        /// Done so far.
        done: usize,
        /// Total.
        total: usize,
    },
    /// Trimming done; follow-up samples pending.
    FollowUp {
        /// The run so far.
        finished: Box<Finished>,
        /// Follow-up points already taken.
        taken: Vec<FollowUpPoint>,
    },
    /// The run is final.
    Final(Box<Finished>),
}

/// Overview screen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverviewView {
    /// Physical memory in use (installed minus available).
    pub used: String,
    /// Installed physical memory.
    pub total: String,
    /// Share of physical memory in use.
    pub load: String,
    /// Commit charge of limit.
    pub committed: String,
    /// Share of installed memory in use, in per mille (0 when unknown).
    pub used_permille: u16,
    /// Commit charge share of the commit limit, in per mille (0 when unknown).
    pub commit_permille: u16,
    /// Pressure word.
    pub pressure: String,
    /// Whether memory pressure is high (shown in the warning colour).
    pub pressure_high: bool,
    /// Standby list (details only).
    pub standby: String,
    /// Modified list (details only).
    pub modified: String,
    /// Pages read from disk per second (details only).
    pub paging: String,
    /// Whether the details panel is on.
    pub details: bool,
    /// Status bar: run progress, or the summary of the last trim.
    pub status: String,
    /// Whether the run in progress can be cancelled from the status bar.
    pub can_cancel: bool,
    /// Whether a run can be started now.
    pub can_run: bool,
    /// Whether trimming is supported on this system.
    pub trim_supported: bool,
}

/// One process row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRowView {
    /// Identity.
    pub key: ProcessKey,
    /// Image name.
    pub name: String,
    /// PID.
    pub pid: String,
    /// Working set.
    pub working_set: String,
    /// Private bytes.
    pub private_bytes: String,
    /// Eligibility text.
    pub status: String,
    /// Eligible for trimming.
    pub eligible: bool,
    /// Selected for "trim selected".
    pub selected: bool,
}

/// Confirmation dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanView {
    /// Question.
    pub title: String,
    /// One line per skip reason with its count.
    pub skipped: Vec<String>,
    /// What trimming does and does not do.
    pub caveat: String,
    /// False when the plan has no targets.
    pub can_start: bool,
}

/// Result screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultView {
    /// Progress or completion line.
    pub status: String,
    /// Report statements with their tone.
    pub lines: Vec<(Tone, String)>,
    /// Per-process outcome: (name, text).
    pub processes: Vec<(String, String)>,
    /// Whether "Cancel" applies.
    pub can_cancel: bool,
    /// Progress 0..=1 while trimming.
    pub progress: f32,
}

/// Settings form, as text fields the user edits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SettingsForm {
    /// Size threshold in MiB.
    pub min_working_set_mib: String,
    /// Exclusions, one image name per line.
    pub exclusions: String,
    /// Caution images allowed, one per line.
    pub allowed_caution: String,
    /// Ask before every run.
    pub confirm_runs: bool,
    /// Keep history.
    pub keep_history: bool,
    /// Local error log.
    pub log_enabled: bool,
}

impl SettingsForm {
    /// The form showing `s`.
    pub fn of(s: &Settings) -> Self {
        Self {
            min_working_set_mib: s.min_working_set_mib.to_string(),
            exclusions: s.exclusions.join("\n"),
            allowed_caution: s.allowed_caution.join("\n"),
            confirm_runs: s.confirm_runs,
            keep_history: s.keep_history,
            log_enabled: s.log_enabled,
        }
    }

    /// Validated settings, keeping fields the form does not show from `base`.
    pub fn parse(&self, base: &Settings) -> Result<Settings, String> {
        let mib: u64 = self
            .min_working_set_mib
            .trim()
            .parse()
            .ok()
            .filter(|m| *m <= 65_536)
            .ok_or_else(|| tr("settings.invalid_threshold").to_owned())?;
        let list = |t: &str| -> Vec<String> {
            let mut v: Vec<String> = t
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            v.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
            v
        };
        Ok(Settings {
            min_working_set_mib: mib,
            exclusions: list(&self.exclusions),
            allowed_caution: list(&self.allowed_caution),
            confirm_runs: self.confirm_runs,
            keep_history: self.keep_history,
            log_enabled: self.log_enabled,
            ..base.clone()
        })
    }
}

/// The application state behind every screen.
#[derive(Debug, Default)]
pub struct AppState {
    utc_offset_min: i32,
    /// Current screen.
    pub screen: Screen,
    visible: bool,
    capabilities: Option<Capabilities>,
    settings: Settings,
    system: Option<SystemSnapshot>,
    table: Vec<Row>,
    sort: (Column, bool),
    eligible_only: bool,
    selected: BTreeSet<ProcessKey>,
    plan: Option<Plan>,
    pending_mode: Option<RunMode>,
    stage: Stage,
    history: Vec<RunRecord>,
    warning: Option<String>,
    details: bool,
}

impl AppState {
    /// A fresh state; `utc_offset_min` localises dates.
    pub fn new(utc_offset_min: i32) -> Self {
        Self {
            utc_offset_min,
            sort: (Column::WorkingSet, false),
            ..Self::default()
        }
    }

    /// Settings in effect.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Current run stage.
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// Whether the window is shown.
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// The pending warning, if any.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    fn view(&self) -> View {
        if self.visible {
            self.screen.view()
        } else {
            View::Hidden
        }
    }

    fn busy(&self) -> bool {
        matches!(self.stage, Stage::Planning | Stage::Running { .. })
    }

    // ---- events -------------------------------------------------------------------

    /// Applies an engine event.
    pub fn apply(&mut self, e: Event) -> Reaction {
        match e {
            Event::Capabilities(c) => {
                self.capabilities = Some(c);
                Reaction::dirty(Dirty {
                    overview: true,
                    ..Dirty::default()
                })
            }
            Event::Settings(s) => {
                self.settings = s;
                Reaction::dirty(Dirty {
                    settings: true,
                    ..Dirty::default()
                })
            }
            Event::SystemSample(s) => {
                let changed = self.system.as_ref() != Some(&s);
                self.system = Some(s);
                Reaction::dirty(Dirty {
                    overview: changed,
                    tray: changed,
                    ..Dirty::default()
                })
            }
            Event::ProcessTable(t) => {
                let changed = t != self.table;
                self.table = t;
                let live: BTreeSet<ProcessKey> = self.table.iter().map(|(p, _)| p.key).collect();
                self.selected.retain(|k| live.contains(k));
                Reaction::dirty(Dirty {
                    processes: changed,
                    ..Dirty::default()
                })
            }
            Event::PlanReady(plan) => self.plan_ready(plan),
            Event::RunProgress { done, total } => {
                self.stage = Stage::Running { done, total };
                Reaction::dirty(Dirty {
                    result: true,
                    overview: true,
                    ..Dirty::default()
                })
            }
            Event::RunCompleted(f) => {
                self.stage = Stage::FollowUp {
                    finished: f,
                    taken: Vec::new(),
                };
                Reaction::dirty(Dirty {
                    result: true,
                    overview: true,
                    ..Dirty::default()
                })
            }
            Event::FollowUpSample(p) => {
                if let Stage::FollowUp { taken, .. } = &mut self.stage {
                    taken.push(p);
                }
                Reaction::dirty(Dirty {
                    result: true,
                    ..Dirty::default()
                })
            }
            Event::RunFinalized(f) => {
                self.stage = Stage::Final(f);
                Reaction::dirty(Dirty {
                    result: true,
                    overview: true,
                    tray: true,
                    ..Dirty::default()
                })
            }
            Event::History(h) => {
                self.history = h;
                Reaction::dirty(Dirty {
                    history: true,
                    overview: true,
                    ..Dirty::default()
                })
            }
            Event::RunRejected(w) => {
                if matches!(self.stage, Stage::Planning | Stage::Running { .. }) {
                    self.stage = Stage::Idle;
                }
                self.warning = Some(w);
                Reaction::dirty(Dirty {
                    warning: true,
                    overview: true,
                    result: true,
                    ..Dirty::default()
                })
            }
            Event::Warning(w) => {
                if matches!(self.stage, Stage::Planning) {
                    // The plan request failed; let the user try again.
                    self.stage = Stage::Idle;
                }
                self.warning = Some(w);
                Reaction::dirty(Dirty {
                    warning: true,
                    overview: true,
                    ..Dirty::default()
                })
            }
            Event::Stopped => Reaction::default(),
        }
    }

    fn plan_ready(&mut self, plan: Plan) -> Reaction {
        let mode = self.pending_mode.unwrap_or(RunMode::Trim);
        if !self.settings.confirm_runs && !plan.targets.is_empty() {
            let id = plan.id;
            let total = plan.targets.len();
            self.plan = None;
            self.stage = Stage::Running { done: 0, total };
            return Reaction::dirty(Dirty {
                result: true,
                overview: true,
                ..Dirty::default()
            })
            .command(Command::StartRun { plan_id: id, mode })
            .command(Command::Subscribe(self.view()));
        }
        self.plan = Some(plan);
        self.stage = Stage::Idle;
        Reaction::dirty(Dirty {
            plan: true,
            overview: true,
            ..Dirty::default()
        })
    }

    // ---- intents ------------------------------------------------------------------

    /// Shows or hides the window.
    pub fn set_visible(&mut self, visible: bool) -> Reaction {
        self.visible = visible;
        let mut r = Reaction::default().command(Command::Subscribe(self.view()));
        if visible {
            r.dirty = Dirty::all();
            r.commands.push(Command::RefreshNow);
        }
        r
    }

    /// Navigates to a screen.
    pub fn show(&mut self, screen: Screen) -> Reaction {
        let view_changed = screen.view() != self.screen.view();
        self.screen = screen;
        let mut r = Reaction::dirty(Dirty {
            screen: true,
            ..Dirty::default()
        });
        if view_changed && self.visible {
            r.commands.push(Command::Subscribe(self.view()));
            r.commands.push(Command::RefreshNow);
        }
        r
    }

    /// Sorts by a column; the same column again flips the order.
    pub fn sort_by(&mut self, c: Column, ascending: bool) -> Dirty {
        self.sort = (c, ascending);
        Dirty {
            processes: true,
            ..Dirty::default()
        }
    }

    /// Shows only eligible processes.
    pub fn set_eligible_only(&mut self, on: bool) -> Dirty {
        self.eligible_only = on;
        Dirty {
            processes: true,
            ..Dirty::default()
        }
    }

    /// Toggles selection of an eligible process.
    pub fn toggle_selected(&mut self, key: ProcessKey) -> Dirty {
        if !self.selected.remove(&key)
            && self
                .table
                .iter()
                .any(|(p, e)| p.key == key && e.is_eligible())
        {
            self.selected.insert(key);
        }
        Dirty {
            processes: true,
            ..Dirty::default()
        }
    }

    /// Key of the first listed process with this image name.
    pub fn key_of(&self, image: &str) -> Option<ProcessKey> {
        self.table
            .iter()
            .find(|(p, _)| p.image.matches(image))
            .map(|(p, _)| p.key)
    }

    /// Current sort column and direction.
    pub fn sort(&self) -> (Column, bool) {
        self.sort
    }

    /// Whether only eligible processes are listed.
    pub fn eligible_only(&self) -> bool {
        self.eligible_only
    }

    /// Number of selected processes.
    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    /// Asks the engine for a plan (all eligible, or the selection).
    pub fn request_run(&mut self, mode: RunMode, selected_only: bool) -> Reaction {
        if self.busy() || !self.trim_supported() {
            return Reaction::default();
        }
        let sel = if selected_only && !self.selected.is_empty() {
            Selection::Only(self.selected.iter().copied().collect())
        } else {
            Selection::AllEligible
        };
        self.pending_mode = Some(mode);
        self.stage = Stage::Planning;
        Reaction::dirty(Dirty {
            overview: true,
            ..Dirty::default()
        })
        .command(Command::PlanRun(sel))
    }

    /// Confirms the shown plan.
    pub fn confirm(&mut self) -> Reaction {
        let Some(plan) = self.plan.take() else {
            return Reaction::default();
        };
        if plan.targets.is_empty() {
            return self.dismiss_plan_inner(Dirty {
                plan: true,
                ..Dirty::default()
            });
        }
        let mode = self.pending_mode.unwrap_or(RunMode::Trim);
        self.stage = Stage::Running {
            done: 0,
            total: plan.targets.len(),
        };
        let mut r = Reaction::dirty(Dirty {
            plan: true,
            result: true,
            overview: true,
            ..Dirty::default()
        })
        .command(Command::StartRun {
            plan_id: plan.id,
            mode,
        });
        if self.visible {
            r.commands.push(Command::Subscribe(self.view()));
        }
        r
    }

    /// Closes the dialog without running.
    pub fn dismiss_plan(&mut self) -> Reaction {
        self.plan = None;
        self.dismiss_plan_inner(Dirty {
            plan: true,
            overview: true,
            ..Dirty::default()
        })
    }

    fn dismiss_plan_inner(&mut self, d: Dirty) -> Reaction {
        self.stage = Stage::Idle;
        Reaction::dirty(d).command(Command::CancelRun)
    }

    /// Cancels the run or its follow-up.
    pub fn cancel_run(&mut self) -> Reaction {
        match self.stage {
            Stage::Running { .. } | Stage::FollowUp { .. } => {
                Reaction::default().command(Command::CancelRun)
            }
            _ => Reaction::default(),
        }
    }

    /// Validates and applies the form.
    pub fn save_settings(&mut self, form: &SettingsForm) -> Result<Reaction, String> {
        let s = form.parse(&self.settings)?;
        Ok(Reaction::default().command(Command::UpdateSettings(s)))
    }

    /// Turns the optional detailed counters on or off.
    pub fn set_details(&mut self, on: bool) -> Reaction {
        self.details = on;
        Reaction::dirty(Dirty {
            overview: true,
            ..Dirty::default()
        })
        .command(Command::SetDetails(on))
        .command(Command::RefreshNow)
    }

    /// Deletes history and logs.
    pub fn clear_history(&mut self) -> Reaction {
        Reaction::default().command(Command::ClearHistory)
    }

    /// Hides the warning banner.
    pub fn dismiss_warning(&mut self) -> Dirty {
        self.warning = None;
        Dirty {
            warning: true,
            ..Dirty::default()
        }
    }

    /// Merges several reactions (helper for adapters).
    pub fn merge(a: &mut Reaction, b: Reaction) {
        a.dirty.merge(b.dirty);
        a.commands.extend(b.commands);
    }

    // ---- views --------------------------------------------------------------------

    fn trim_supported(&self) -> bool {
        self.capabilities
            .as_ref()
            .is_none_or(|c| c.trim_working_set.is_available())
    }

    fn when(&self, t: Millis) -> String {
        date_time(t, self.utc_offset_min)
    }

    /// Overview screen.
    pub fn overview(&self) -> OverviewView {
        let unknown = || tr("value.unknown").to_owned();
        let mut v = OverviewView {
            details: self.details,
            can_run: !self.busy() && self.plan.is_none() && self.trim_supported(),
            trim_supported: self.trim_supported(),
            status: self.status_line(),
            can_cancel: matches!(self.stage, Stage::Running { .. }),
            ..OverviewView::default()
        };
        match &self.system {
            Some(s) => {
                let used = s.phys_total.saturating_sub(s.phys_available);
                v.used = used.to_string();
                v.total = s.phys_total.to_string();
                v.load = trf(
                    "overview.load",
                    &[("percent", &percent(used, s.phys_total))],
                );
                v.committed = trf(
                    "overview.committed",
                    &[("used", &s.commit_charge), ("limit", &s.commit_limit)],
                );
                v.used_permille = permille(used, s.phys_total);
                v.commit_permille = permille(s.commit_charge, s.commit_limit);
                v.pressure = pressure(s.pressure).to_owned();
                v.pressure_high = s.pressure == Pressure::High;
                v.standby = opt_bytes(s.standby);
                v.modified = opt_bytes(s.modified);
                v.paging = s
                    .pages_input_per_sec
                    .map_or_else(unknown, |p| format!("{p}/s"));
            }
            None => {
                for f in [
                    &mut v.used,
                    &mut v.total,
                    &mut v.load,
                    &mut v.committed,
                    &mut v.pressure,
                    &mut v.standby,
                    &mut v.modified,
                    &mut v.paging,
                ] {
                    *f = unknown();
                }
            }
        }
        v
    }

    /// Status bar text: progress while trimming, otherwise the totals of the current
    /// or the last stored trim (dry runs are not summarised).
    fn status_line(&self) -> String {
        let summary = |r: &RunRecord| {
            let t = r.totals();
            trf(
                "status.summary",
                &[("trimmed", &t.trimmed), ("amount", &t.working_set_reduced)],
            )
        };
        match &self.stage {
            Stage::Running { done, total } => {
                return trf("run.progress", &[("done", done), ("total", total)]);
            }
            Stage::FollowUp { finished: f, .. } | Stage::Final(f)
                if f.record.mode == RunMode::Trim =>
            {
                return summary(&f.record);
            }
            _ => {}
        }
        self.history
            .iter()
            .rev()
            .find(|r| r.mode == RunMode::Trim)
            .map_or_else(|| tr("status.never").to_owned(), summary)
    }

    /// Process table rows, filtered and sorted.
    pub fn processes(&self) -> Vec<ProcessRowView> {
        let mut rows: Vec<&Row> = self
            .table
            .iter()
            .filter(|(_, e)| !self.eligible_only || e.is_eligible())
            .collect();
        let ws = |r: &Row| r.0.memory.map_or(Bytes::ZERO, |m| m.working_set);
        let pb = |r: &Row| r.0.memory.map_or(Bytes::ZERO, |m| m.private_bytes);
        let (col, asc) = self.sort;
        rows.sort_by(|a, b| {
            let o = match col {
                Column::Name => {
                    a.0.image
                        .as_str()
                        .to_lowercase()
                        .cmp(&b.0.image.as_str().to_lowercase())
                }
                Column::Pid => a.0.key.pid.cmp(&b.0.key.pid),
                Column::WorkingSet => ws(a).cmp(&ws(b)),
                Column::PrivateBytes => pb(a).cmp(&pb(b)),
                Column::Status => tr(a.1.key()).cmp(tr(b.1.key())),
            };
            let o = if asc { o } else { o.reverse() };
            o.then(a.0.key.pid.cmp(&b.0.key.pid))
        });
        rows.into_iter()
            .map(|(p, e)| ProcessRowView {
                key: p.key,
                name: p.image.to_string(),
                pid: p.key.pid.to_string(),
                working_set: opt_bytes(p.memory.map(|m| m.working_set)),
                private_bytes: opt_bytes(p.memory.map(|m| m.private_bytes)),
                status: tr(e.key()).to_owned(),
                eligible: e.is_eligible(),
                selected: self.selected.contains(&p.key),
            })
            .collect()
    }

    /// The confirmation dialog, while a plan waits.
    pub fn plan(&self) -> Option<PlanView> {
        let p = self.plan.as_ref()?;
        let dry = self.pending_mode == Some(RunMode::DryRun);
        let title = if p.targets.is_empty() {
            tr("plan.empty").to_owned()
        } else {
            trf(
                if dry { "plan.title_dry" } else { "plan.title" },
                &[
                    ("count", &p.targets.len()),
                    ("amount", &p.targeted_working_set()),
                ],
            )
        };
        let skipped = p
            .skip_counts()
            .into_iter()
            .map(|(k, n)| trf("plan.skipped", &[("count", &n), ("reason", &tr(k))]))
            .collect();
        Some(PlanView {
            title,
            skipped,
            caveat: tr("plan.caveat").to_owned(),
            can_start: !p.targets.is_empty(),
        })
    }

    /// Result screen.
    pub fn result(&self) -> ResultView {
        match &self.stage {
            Stage::Idle | Stage::Planning => ResultView::default(),
            Stage::Running { done, total } => ResultView {
                status: trf("run.progress", &[("done", done), ("total", total)]),
                can_cancel: true,
                progress: if *total == 0 {
                    1.0
                } else {
                    *done as f32 / *total as f32
                },
                ..ResultView::default()
            },
            Stage::FollowUp { finished, taken } => {
                let next = [FollowUpPoint::Plus10s, FollowUpPoint::Plus60s]
                    .into_iter()
                    .find(|p| !taken.contains(p))
                    .unwrap_or(FollowUpPoint::Plus60s);
                let mut v = Self::finished_view(finished);
                v.status = trf("run.follow_up", &[("point", &point(next))]);
                v.can_cancel = true;
                v
            }
            Stage::Final(f) => {
                let mut v = Self::finished_view(f);
                v.status = match f.record.completion {
                    Completion::Complete => tr("run.follow_up_done"),
                    Completion::Cancelled => tr("run.cancelled"),
                    Completion::FollowUpTruncated => tr("run.truncated"),
                }
                .to_owned();
                v
            }
        }
    }

    fn finished_view(f: &Finished) -> ResultView {
        ResultView {
            lines: f.report.iter().map(|s| (s.tone, s.render_en())).collect(),
            processes: f
                .record
                .results
                .iter()
                .map(|r| (r.image.to_string(), outcome(&r.outcome)))
                .collect(),
            progress: 1.0,
            ..ResultView::default()
        }
    }

    /// History, newest first.
    pub fn history(&self) -> Vec<String> {
        self.history
            .iter()
            .rev()
            .map(|r| {
                let t = r.totals();
                trf(
                    "history.row",
                    &[
                        ("when", &self.when(r.started_at)),
                        (
                            "mode",
                            &tr(match r.mode {
                                RunMode::Trim => "mode.trim",
                                RunMode::DryRun => "mode.dry_run",
                            }),
                        ),
                        ("trimmed", &t.trimmed),
                        ("amount", &t.working_set_reduced),
                    ],
                )
            })
            .collect()
    }

    /// The settings form for the settings in effect.
    pub fn settings_form(&self) -> SettingsForm {
        SettingsForm::of(&self.settings)
    }

    /// Tray icon state.
    pub fn tray_state(&self) -> TrayState {
        if matches!(self.stage, Stage::Running { .. } | Stage::FollowUp { .. }) {
            return TrayState::Working;
        }
        match &self.system {
            _ if !self.trim_supported() => TrayState::Unavailable,
            None => TrayState::Unavailable,
            Some(s) if s.pressure == Pressure::High => TrayState::High,
            Some(_) => TrayState::Normal,
        }
    }

    /// Tray tooltip.
    pub fn tray_tooltip(&self) -> String {
        match &self.system {
            Some(s) => trf(
                "tray.tooltip_available",
                &[("available", &s.phys_available)],
            ),
            None => tr("tray.tooltip").to_owned(),
        }
    }
}

fn point(p: FollowUpPoint) -> &'static str {
    match p {
        FollowUpPoint::Plus10s => tr("point.10s"),
        FollowUpPoint::Plus60s => tr("point.60s"),
    }
}

fn outcome(o: &ProcessOutcome) -> String {
    match o {
        ProcessOutcome::Trimmed { before, after } => trf(
            "outcome.trimmed",
            &[
                ("before", &before.working_set),
                ("after", &after.working_set),
            ],
        ),
        ProcessOutcome::Skipped(e) => tr(e.key()).to_owned(),
        ProcessOutcome::Cancelled => tr("outcome.cancelled").to_owned(),
        ProcessOutcome::Failed(f) => match f {
            FailureKind::AccessDenied => tr("failure.access_denied").to_owned(),
            FailureKind::Exited => tr("failure.exited").to_owned(),
            FailureKind::IdentityChanged => tr("failure.identity_changed").to_owned(),
            FailureKind::Unsupported => tr("failure.unsupported").to_owned(),
            FailureKind::Os(c) => trf("failure.os", &[("code", c)]),
            FailureKind::Internal(_) => tr("failure.internal").to_owned(),
        },
    }
}
