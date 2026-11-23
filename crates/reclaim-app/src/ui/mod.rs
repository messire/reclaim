//! Slint adapter: binds [`AppState`] to the window and the tray (ARCHITECTURE §8).
//!
//! Lifecycle (owner decision Q5, PLAN M4.3–M4.5):
//! - The tray icon lives for the whole process. Explorer restarts are handled by Slint's
//!   tray, which re-adds the icon on `TaskbarCreated`.
//! - Close (X) hides to the tray and destroys the window so its renderer memory is freed.
//!   Only "Quit" in the tray menu exits.
//! - `--tray` starts without a window. A second launch shows the running instance.
//! - DPI: winit makes the process Per-Monitor V2 aware before the first window.

mod autostart;
mod icons;
mod storage;
mod view;
mod win;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, VecModel};

use reclaim_app::vm::{AppState, Column, Dirty, Reaction, Screen, SettingsForm, TrayState};
use reclaim_core::outcome::RunMode;
use reclaim_engine::{Command, EngineHandle, Event, StdClock, Store, spawn};
use reclaim_platform_windows::WindowsPlatform;

use view::{MainWindow, ProcRow, Tray};

struct App {
    state: AppState,
    engine: Option<EngineHandle>,
    window: Option<MainWindow>,
    tray: Tray,
    tray_state: Option<TrayState>,
    settings_message: String,
    ready_event: Option<String>,
    /// Storage screens, created on their first visit.
    storage: Option<reclaim_app::storage_vm::Storage>,
    /// Change watchers and their poll timer while "Watch for changes" is on.
    watcher: Option<(Vec<reclaim_storage_windows::watch::Watcher>, slint::Timer)>,
    watch_on: bool,
    storage_message: String,
    /// Frees the disk index after the window has been hidden for a while.
    release_timer: Option<slint::Timer>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app unless it is already borrowed (a re-entrant toolkit callback,
/// which is dropped: the state it would change is re-pushed by the outer call).
fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Applies an intent and its consequences.
fn act(f: impl FnOnce(&mut AppState) -> Reaction) {
    with(|a| {
        let r = f(&mut a.state);
        a.react(r);
    });
}

fn act_dirty(f: impl FnOnce(&mut AppState) -> Dirty) {
    with(|a| {
        let d = f(&mut a.state);
        a.refresh(d);
    });
}

impl App {
    fn react(&mut self, r: Reaction) {
        if let Some(e) = &self.engine {
            for c in r.commands {
                e.send(c);
            }
        }
        self.refresh(r.dirty);
    }

    /// Pushes the changed parts of the state to the toolkit.
    fn refresh(&mut self, d: Dirty) {
        let s = &self.state;
        if d.tray {
            self.tray.set_tip(s.tray_tooltip().into());
        }
        if d.overview {
            self.tray.set_can_run(s.overview().can_run);
        }
        if d.tray || d.overview || d.result {
            let state = s.tray_state();
            // Decode and hand Windows a new icon only when the state really changes.
            if self.tray_state != Some(state)
                && let Some(icon) = icons::tray(state)
            {
                self.tray.set_tray_icon(icon);
                self.tray_state = Some(state);
            }
        }
        let Some(w) = &self.window else {
            return;
        };
        if d.screen {
            w.set_page(s.screen.index() as i32);
        }
        if d.warning {
            w.set_warning(s.warning().unwrap_or_default().into());
        }
        if d.overview {
            let o = s.overview();
            w.set_used(o.used.into());
            w.set_total(o.total.into());
            w.set_load(o.load.into());
            w.set_committed(o.committed.into());
            w.set_used_permille(i32::from(o.used_permille));
            w.set_commit_permille(i32::from(o.commit_permille));
            w.set_pressure_high(o.pressure_high);
            w.set_pressure(o.pressure.into());
            w.set_standby(o.standby.into());
            w.set_modified(o.modified.into());
            w.set_paging(o.paging.into());
            w.set_status(o.status.into());
            w.set_can_cancel(o.can_cancel);
            w.set_can_run(o.can_run);
            w.set_trim_supported(o.trim_supported);
            w.set_details(o.details);
        }
        if d.processes {
            let rows = s.processes();
            w.set_selected_count(s.selected_count() as i32);
            // A recreated window starts unchecked: restore the filter it shows.
            w.set_eligible_only(s.eligible_only());
            let (col, asc) = s.sort();
            w.set_sort_column(col.index() as i32 + 1);
            w.set_sort_ascending(asc);
            let rows: Vec<ProcRow> = rows
                .into_iter()
                .map(|r| ProcRow {
                    selected: r.selected,
                    name: r.name.into(),
                    pid: r.pid.into(),
                    working_set: r.working_set.into(),
                    private_bytes: r.private_bytes.into(),
                    status: r.status.into(),
                    eligible: r.eligible,
                })
                .collect();
            w.set_rows(ModelRc::new(VecModel::from(rows)));
        }
        if d.plan {
            match s.plan() {
                Some(p) => {
                    w.set_plan_title(p.title.into());
                    w.set_plan_skipped(p.skipped.join("\n").into());
                    w.set_plan_caveat(p.caveat.into());
                    w.set_plan_can_start(p.can_start);
                    w.set_plan_open(true);
                }
                None => w.set_plan_open(false),
            }
        }
        if d.settings {
            let f = s.settings_form();
            w.set_s_threshold(f.min_working_set_mib.into());
            w.set_s_exclusions(f.exclusions.into());
            w.set_s_caution(f.allowed_caution.into());
            w.set_s_confirm(f.confirm_runs);
            w.set_s_history(f.keep_history);
            w.set_s_log(f.log_enabled);
            // Read from the registry each time: the installer or Task Manager may change it.
            w.set_s_autostart(autostart::enabled());
            w.set_settings_message(self.settings_message.as_str().into());
        }
    }

    fn form(w: &MainWindow) -> SettingsForm {
        SettingsForm {
            min_working_set_mib: w.get_s_threshold().into(),
            exclusions: w.get_s_exclusions().into(),
            allowed_caution: w.get_s_caution().into(),
            confirm_runs: w.get_s_confirm(),
            keep_history: w.get_s_history(),
            log_enabled: w.get_s_log(),
        }
    }
}

/// Creates the window and wires its callbacks to intents.
fn build() -> Option<MainWindow> {
    let w = MainWindow::new().ok()?;
    w.set_version(reclaim_core::VERSION.into());
    if let Some(icon) = icons::app() {
        w.set_app_icon(icon);
    }
    w.on_navigate(|i| {
        let screen = Screen::from_index(i.max(0) as usize);
        act(|s| s.show(screen));
        if screen.is_storage() || screen == Screen::Settings {
            with(App::enter_storage);
        }
    });
    storage::wire(&w);
    w.on_dismiss_warning(|| act_dirty(AppState::dismiss_warning));
    w.on_run(|| act(|s| s.request_run(RunMode::Trim, false)));
    w.on_run_selected(|| act(|s| s.request_run(RunMode::Trim, true)));
    w.on_toggle_details(|on| act(|s| s.set_details(on)));
    // Table column 0 is the selection mark; data columns start at 1.
    w.on_sort(|c, asc| {
        if c >= 1 {
            act_dirty(|s| s.sort_by(Column::from_index(c as usize - 1), asc));
        }
    });
    w.on_set_eligible_only(|on| act_dirty(|s| s.set_eligible_only(on)));
    w.on_toggle_row(|i| {
        act_dirty(|s| match s.processes().get(i.max(0) as usize) {
            Some(r) => s.toggle_selected(r.key),
            None => Dirty::default(),
        })
    });
    w.on_confirm(|| act(AppState::confirm));
    w.on_dismiss(|| act(AppState::dismiss_plan));
    w.on_cancel_run(|| act(AppState::cancel_run));
    w.on_clear_history(|| act(AppState::clear_history));
    let weak = w.as_weak();
    w.on_save_settings(move || {
        let Some(w) = weak.upgrade() else { return };
        let form = App::form(&w);
        let start_with_windows = w.get_s_autostart();
        with(|a| {
            // Validate first: an invalid form changes nothing, autostart included.
            match a.state.save_settings(&form) {
                Ok(r) => {
                    let autostart = if start_with_windows == autostart::enabled() {
                        Ok(())
                    } else {
                        autostart::set(start_with_windows)
                    };
                    a.settings_message = match autostart {
                        Ok(()) => reclaim_app::strings::tr("settings.saved").to_owned(),
                        Err(code) => reclaim_app::strings::tr("settings.autostart_failed")
                            .replace("{code}", &code.to_string()),
                    };
                    a.react(r);
                }
                Err(msg) => a.settings_message = msg,
            }
            if let Some(w) = &a.window {
                w.set_settings_message(a.settings_message.as_str().into());
            }
        });
    });
    w.on_revert_settings(|| {
        with(|a| {
            a.settings_message.clear();
            a.refresh(Dirty {
                settings: true,
                ..Dirty::default()
            });
        });
    });
    // Close (X) goes to the tray (owner decision Q5).
    w.window().on_close_requested(|| {
        hide_window();
        slint::CloseRequestResponse::HideWindow
    });
    Some(w)
}

/// Asks DWM for a dark caption that matches the window background. The native window is
/// created lazily by the event loop, so `show()` alone does not give a handle yet: retry on
/// later loop turns (up to ~1 s). Cosmetic: failures leave the default caption.
fn dark_title_bar(attempt: u32) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let applied = with(|a| {
        let w = a.window.as_ref()?;
        let handle = w.window().window_handle();
        let h = handle.window_handle().ok()?;
        match h.as_raw() {
            RawWindowHandle::Win32(h) => {
                win::dark_caption(h.hwnd.get());
                win::set_icons(h.hwnd.get());
                Some(())
            }
            _ => None,
        }
    })
    .flatten()
    .is_some();
    if !applied && attempt < 40 {
        slint::Timer::single_shot(Duration::from_millis(25), move || {
            dark_title_bar(attempt + 1);
        });
    }
}

/// Shows the window, creating it if it was destroyed on hide.
fn open_window() {
    let created = with(|a| {
        let mut created = false;
        if a.window.is_none() {
            a.window = build();
            created = true;
        }
        let r = a.state.set_visible(true);
        a.react(r);
        a.release_timer = None;
        if let Some(w) = &a.window {
            let _ = w.show();
            w.window().set_minimized(false);
        }
        created
    });
    if created == Some(true) {
        slint::Timer::single_shot(Duration::ZERO, || dark_title_bar(0));
        arm_ready_marker();
    }
}

/// Hides the window and drops it on the next turn of the event loop (not inside its own
/// callback), releasing its renderer resources (ARCHITECTURE §8.2, benchmark B3).
fn hide_window() {
    with(|a| {
        if let Some(w) = &a.window {
            let _ = w.hide();
        }
        let r = a.state.set_visible(false);
        a.react(r);
    });
    arm_index_release(None);
    slint::Timer::single_shot(Duration::ZERO, || {
        let w = with(|a| {
            if a.state.visible() {
                None
            } else {
                a.window.take()
            }
        });
        drop(w);
    });
}

/// While the window stays hidden, frees the disk index after the configured minutes
/// (`delay` overrides it for a retry). A running scan or removal postpones it by a minute.
fn arm_index_release(delay: Option<Duration>) {
    let delay = delay.or_else(|| {
        with(|a| {
            a.storage
                .as_ref()
                .map(|s| s.release_after_min())
                .filter(|m| *m > 0)
                .map(|m| Duration::from_secs(u64::from(m) * 60))
        })
        .flatten()
    });
    let Some(delay) = delay else { return };
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::SingleShot, delay, || {
        let retry = with(|a| {
            a.release_timer = None;
            if a.state.visible() {
                return false;
            }
            let Some(s) = &a.storage else {
                return false;
            };
            if !s.has_index() {
                return false;
            }
            if !s.can_release_index() {
                // Busy: keep the index and its change watchers, try again in a minute.
                return true;
            }
            a.stop_watch();
            if let Some(s) = &mut a.storage {
                s.release_index();
            }
            false
        });
        if retry == Some(true) {
            arm_index_release(Some(Duration::from_secs(60)));
        }
    });
    with(|a| a.release_timer = Some(timer));
}

/// Signals `--ready-event` after the first frame of the first window (startup marker for
/// benchmark B1), or on the first event-loop turn if rendering notifiers are unavailable.
fn arm_ready_marker() {
    let Some(name) = with(|a| a.ready_event.take()).flatten() else {
        return;
    };
    let once = Rc::new(RefCell::new(Some(name)));
    let fired = once.clone();
    let ok = with(|a| {
        a.window.as_ref().map(|w| {
            w.window()
                .set_rendering_notifier(move |state, _| {
                    if matches!(state, slint::RenderingState::AfterRendering)
                        && let Some(n) = fired.borrow_mut().take()
                    {
                        win::signal(&n);
                    }
                })
                .is_ok()
        })
    })
    .flatten()
    .unwrap_or(false);
    if !ok {
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(n) = once.borrow_mut().take() {
                win::signal(&n);
            }
        });
    }
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

/// Entry point of the desktop app.
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let Some(mut instance) = win::claim() else {
        return 0; // The running instance was asked to show itself.
    };
    let start_in_tray = args.iter().any(|a| a == "--tray");

    let tray = match Tray::new() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("reclaim: cannot create the user interface: {e}");
            return 1;
        }
    };
    // Replaced by the state icon on the first refresh; the drawn icon is the fallback
    // if a bundled PNG ever fails to decode.
    tray.set_tray_icon(icons::tray(TrayState::Unavailable).unwrap_or_else(tray_icon));
    tray.set_tip(reclaim_app::strings::tr("tray.tooltip").into());
    tray.on_open(open_window);
    tray.on_run(|| {
        open_window();
        act(|s| {
            let mut r = s.show(Screen::Overview);
            AppState::merge(&mut r, s.request_run(RunMode::Trim, false));
            r
        });
    });
    tray.on_quit(|| {
        let engine = with(|a| {
            a.window.take();
            a.engine.take()
        })
        .flatten();
        if let Some(e) = engine {
            e.shutdown(); // Stores a partial record if a run is in flight.
        }
        let _ = slint::quit_event_loop();
    });

    let engine = spawn(
        WindowsPlatform::new(),
        Box::new(StdClock::default()),
        Some(Store::new(Store::default_dir())),
        |e: Event| {
            if matches!(e, Event::Stopped) {
                return;
            }
            let _ = slint::invoke_from_event_loop(move || {
                with(|a| {
                    let r = a.state.apply(e);
                    a.react(r);
                });
            });
        },
    );
    engine.send(Command::Subscribe(reclaim_engine::View::Hidden));

    APP.with(|cell| {
        *cell.borrow_mut() = Some(App {
            state: AppState::new(win::utc_offset_minutes()),
            engine: Some(engine),
            window: None,
            tray,
            tray_state: Some(TrayState::Unavailable),
            settings_message: String::new(),
            ready_event: arg_value(&args, "--ready-event"),
            storage: None,
            watcher: None,
            watch_on: false,
            storage_message: String::new(),
            release_timer: None,
        });
    });

    instance.watch_activation(|| {
        let _ = slint::invoke_from_event_loop(open_window);
    });
    if let Some(name) = arg_value(&args, "--hide-event") {
        // Benchmark hook (B3): behaves like the user pressing Close (X).
        win::on_event(&name, || {
            let _ = slint::invoke_from_event_loop(hide_window);
        });
    }

    if !start_in_tray {
        open_window();
    }
    let _ = with(|a| a.tray.show());
    let code = match slint::run_event_loop_until_quit() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("reclaim: event loop failed: {e}");
            1
        }
    };
    let leftover = APP.with(|cell| cell.borrow_mut().take());
    if let Some(mut a) = leftover
        && let Some(e) = a.engine.take()
    {
        e.shutdown();
    }
    drop(instance);
    code
}

/// 32×32 RGBA tray icon drawn in code: a cyan ring with a downward bar, in the window's
/// accent colour (no asset, no decoder).
fn tray_icon() -> slint::Image {
    const N: u32 = 32;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(N, N);
    for (i, px) in buf.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = ((i as u32 % N) as i32 - 16, (i as u32 / N) as i32 - 16);
        let r2 = x * x + y * y;
        let ring = (11 * 11..=15 * 15).contains(&r2);
        let bar = x.abs() <= 2 && (-7..=7).contains(&y);
        *px = if ring || bar {
            slint::Rgba8Pixel {
                r: 0x3f,
                g: 0xe3,
                b: 0xf2,
                a: 0xff,
            }
        } else {
            slint::Rgba8Pixel {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            }
        };
    }
    slint::Image::from_rgba8(buf)
}
