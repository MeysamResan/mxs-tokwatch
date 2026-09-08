use crate::{
    claude,
    codex::{AccountStatus, Session},
    model::{self, UsageSnapshot},
    settings::{self, Provider, Settings},
    updater,
};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        System::{
            LibraryLoader::GetModuleHandleW,
            Registry::*,
            SystemServices::{SS_ENDELLIPSIS, SS_NOPREFIX, SS_OWNERDRAW},
            Threading::*,
            Time::*,
        },
        UI::{
            Controls::{DRAWITEMSTRUCT, Dialogs::*, WM_MOUSELEAVE},
            HiDpi::*,
            Input::KeyboardAndMouse::*,
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::{BOOL, PCWSTR, w},
};
#[path = "ui_render.rs"]
mod ui_render;
#[path = "ui_style.rs"]
mod ui_style;
#[path = "ui_tray.rs"]
mod ui_tray;
const PANEL_WIDTH: i32 = 320;
const PANEL_HEIGHT: i32 = 292;
const FONT_SPECS: [(i32, i32); 5] = [(12, 600), (12, 400), (28, 600), (18, 600), (11, 400)];
const LABEL_SPECS: [(i32, i32, i32, i32, usize); 14] = [
    (24, 16, 190, 18, 0),
    (222, 16, 74, 18, 4),
    (24, 36, 80, 38, 2),
    (112, 46, 184, 18, 1),
    (24, 87, 272, 16, 4),
    (24, 124, 118, 16, 4),
    (24, 141, 118, 24, 3),
    (24, 169, 118, 16, 4),
    (178, 124, 118, 16, 4),
    (178, 141, 118, 24, 3),
    (178, 169, 118, 16, 4),
    (16, 196, 136, 20, 1),
    (164, 196, 140, 20, 0),
    (28, 227, 276, 16, 4),
];
const NIN_KEYSELECT: u32 = 0x401;
const TRAY: u32 = WM_APP + 1;
const RESULT: u32 = WM_APP + 2;
const SHOW_PANEL: u32 = WM_APP + 4;
const TICK: usize = 1;
const PANEL_TICK: usize = 2;
const MOTION_TICK: usize = 3;
const APPEARANCE_CHANGED: u32 = WM_APP + 5;
const CONTROL_CHANGED: u32 = WM_APP + 6;
#[path = "ui_motion.rs"]
mod ui_motion;
#[path = "ui_theme.rs"]
mod ui_theme;
const PROVIDER_CODEX: u16 = 110;
const PROVIDER_CLAUDE: u16 = 111;
const CHECK_UPDATES: u16 = 112;
const AUTO_UPDATES: u16 = 113;
const INSTALL_UPDATE: u16 = 114;
const UPDATE_STATUS: u16 = 115;
const CONNECTION_STATUS: u16 = 116;
const REFRESH: u16 = 101;
const CONNECT: u16 = 102;
const SETTINGS: u16 = 103;
const STARTUP: u16 = 104;
const PICK_CODEX: u16 = 105;
const EXIT: u16 = 106;
const ABOUT: u16 = 107;
const OPEN_FOLDER: u16 = 108;
const AUTO_CODEX: u16 = 109;
const WINDOW_BASE: u16 = 1000;
const INTERVAL_BASE: u16 = 2000;
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn color(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}
#[derive(Clone, Copy)]
enum PanelAnchor {
    Tray(RECT),
    Window(RECT),
}

// Constrain the flyout's own scale without changing the tray icon's monitor DPI.
fn panel_geometry(dpi: u32, work: RECT, anchor: PanelAnchor) -> (u32, RECT) {
    let work_width = (work.right - work.left).max(1);
    let work_height = (work.bottom - work.top).max(1);
    let inset_x = 8.min((work_width - 1) / 2);
    let inset_y = 8.min((work_height - 1) / 2);
    let available_width = work_width - inset_x * 2;
    let available_height = work_height - inset_y * 2;
    let panel_dpi = dpi
        .max(1)
        .min((available_width as u32 * 96 / PANEL_WIDTH as u32).max(1))
        .min((available_height as u32 * 96 / PANEL_HEIGHT as u32).max(1));
    let width = (PANEL_WIDTH * panel_dpi as i32 / 96)
        .max(1)
        .min(available_width);
    let height = (PANEL_HEIGHT * panel_dpi as i32 / 96)
        .max(1)
        .min(available_height);
    let min_x = work.left + inset_x;
    let min_y = work.top + inset_y;
    let preferred = match anchor {
        PanelAnchor::Tray(rect) => POINT {
            x: rect.right - width,
            y: if rect.top - height - 8 >= min_y {
                rect.top - height - 8
            } else {
                rect.bottom + 8
            },
        },
        PanelAnchor::Window(rect) => POINT {
            x: rect.left,
            y: rect.top,
        },
    };
    let x = preferred.x.clamp(min_x, work.right - inset_x - width);
    let y = preferred.y.clamp(min_y, work.bottom - inset_y - height);
    (
        panel_dpi,
        RECT {
            left: x,
            top: y,
            right: x + width,
            bottom: y + height,
        },
    )
}

unsafe fn panel_monitor(anchor: &RECT, fallback_dpi: u32) -> (RECT, u32) {
    unsafe {
        let monitor = MonitorFromRect(anchor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcWork
        } else {
            let mut fallback = RECT {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN).max(1),
                bottom: GetSystemMetrics(SM_CYSCREEN).max(1),
            };
            let _ = SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut fallback as *mut _ as *mut _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            fallback
        };
        let mut dpi_x = fallback_dpi;
        let mut dpi_y = fallback_dpi;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        (work, dpi_x.max(96))
    }
}

enum Command {
    Refresh(Provider, Option<PathBuf>),
    Connect(Provider, Option<PathBuf>),
    CheckLogin,
    Stop,
}
enum Update {
    Snapshot(UsageSnapshot),
    SignedOut(bool),
    Error(String),
    OpenLogin(String),
    ClaudeConnected,
    Pending,
}
struct Worker {
    tx: Sender<Command>,
    rx: Receiver<Update>,
    thread: Option<thread::JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
}
impl Worker {
    fn stop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.tx.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            // Let the worker close its helper before process exit closes every
            // handle, including the helper's kill-on-close job.
            let _ = thread.join();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}
fn worker(hwnd: HWND) -> Worker {
    let (tx, commands) = mpsc::channel();
    let (results, rx) = mpsc::channel();
    let handle = hwnd.0 as isize;
    let stopping = Arc::new(AtomicBool::new(false));
    let worker_stopping = Arc::clone(&stopping);
    let thread = thread::spawn(move || {
        let mut session = Session::new(Arc::clone(&worker_stopping));
        let mut login_started: Option<Instant> = None;
        while let Ok(command) = commands.recv() {
            if matches!(command, Command::Stop) || worker_stopping.load(Ordering::Acquire) {
                break;
            }
            let result: Result<Update, String> = (|| match command {
                Command::Stop => Ok(Update::Pending),
                Command::Refresh(provider, path) => {
                    login_started = None;
                    if provider == Provider::Claude {
                        session.clear();
                        return claude::read_limits().map(Update::Snapshot);
                    }
                    let client = session.connect(path)?;
                    match client.read_account()? {
                        AccountStatus::SignedIn { .. } => {
                            Ok(Update::Snapshot(client.read_limits()?))
                        }
                        AccountStatus::SignedOut => Ok(Update::SignedOut(false)),
                        AccountStatus::ApiKey => Ok(Update::SignedOut(true)),
                    }
                }
                Command::Connect(provider, path) => {
                    session.clear();
                    login_started = None;
                    if provider == Provider::Claude {
                        claude::connect()?;
                        return Ok(Update::ClaudeConnected);
                    }
                    let client = session.connect(path)?;
                    let url = client.start_login()?;
                    login_started = Some(Instant::now());
                    Ok(Update::OpenLogin(url))
                }
                Command::CheckLogin => {
                    if login_started.is_some_and(|t| t.elapsed() > Duration::from_secs(300)) {
                        session.clear();
                        login_started = None;
                        return Err("Sign-in timed out. Choose Connect to try again.".into());
                    }
                    if login_started.is_none() {
                        return Ok(Update::Pending);
                    }
                    let Some(client) = session.client() else {
                        return Ok(Update::Pending);
                    };
                    if client.poll_login()? {
                        let snapshot = client.read_limits()?;
                        login_started = None;
                        Ok(Update::Snapshot(snapshot))
                    } else {
                        Ok(Update::Pending)
                    }
                }
            })();
            let update = result.unwrap_or_else(|e| {
                session.clear();
                login_started = None;
                Update::Error(e)
            });
            if worker_stopping.load(Ordering::Acquire) || results.send(update).is_err() {
                break;
            }
            unsafe {
                let _ = PostMessageW(Some(HWND(handle as *mut _)), RESULT, WPARAM(0), LPARAM(0));
            }
        }
    });
    Worker {
        tx,
        rx,
        thread: Some(thread),
        stopping,
    }
}
struct App {
    hwnd: HWND,
    worker: Worker,
    settings: Settings,
    snapshot: Option<UsageSnapshot>,
    verified: bool,
    status: String,
    error: Option<String>,
    busy: bool,
    login_pending: bool,
    next_refresh: Instant,
    failures: u32,
    pinned: bool,
    visible: bool,
    demo: bool,
    dark: bool,
    appearance: ui_theme::Appearance,
    glass: bool,
    panel_bounds: RECT,
    panel_offset: i32,
    panel_value: f32,
    panel_motion: Option<ui_motion::Tween>,
    meter_value: f32,
    meter_motion: Option<ui_motion::Tween>,
    hover: [f32; 2],
    hover_motion: [Option<ui_motion::Tween>; 2],
    dpi: u32,
    panel_dpi: u32,
    brush: HBRUSH,
    surface_brush: HBRUSH,
    signed_out: bool,
    fonts: Vec<HFONT>,
    labels: Vec<HWND>,
    buttons: Vec<HWND>,
    icon: HICON,
    icon_key: String,
    tray_added: bool,
    taskbar_created: u32,
    menu_keys: Vec<String>,
    label_texts: RefCell<Vec<String>>,
    last_render_second: u64,
    claude_connected: bool,
    update_rx: Option<Receiver<Result<Option<updater::PreparedUpdate>, String>>>,
    update_ready: Option<updater::PreparedUpdate>,
    installer_rx: Option<Receiver<Result<(), String>>>,
    update_status: String,
    next_update_check: Instant,
}
impl App {
    fn px(&self, n: i32) -> i32 {
        n * self.panel_dpi as i32 / 96
    }
    fn palette(&self) -> ui_style::Palette {
        ui_style::palette_for(
            self.dark,
            self.appearance.accent,
            self.appearance.high_contrast,
        )
    }
    fn bg(&self) -> COLORREF {
        self.palette().bg
    }
    fn fg(&self) -> COLORREF {
        self.palette().text
    }
    fn muted(&self) -> COLORREF {
        self.palette().muted
    }
    fn selected(&self) -> Option<(&model::UsagePool, &model::UsageWindow)> {
        self.snapshot
            .as_ref()?
            .selected_window(self.settings.selected_window.as_deref())
    }
    fn stale(&self) -> bool {
        !self.verified
            || self.error.is_some()
            || self
                .snapshot
                .as_ref()
                .is_none_or(|s| s.is_stale(model::unix_now(), self.settings.poll_seconds()))
    }
    fn primary_connect(&self) -> bool {
        if self.demo {
            return false;
        }
        match self.settings.provider {
            Provider::Codex => self.signed_out,
            Provider::Claude => !self.claude_connected,
        }
    }
    fn can_connect(&self) -> bool {
        !self.busy
            && !self.login_pending
            && !self.demo
            && match self.settings.provider {
                Provider::Claude => !self.claude_connected,
                Provider::Codex => !self.verified || self.error.is_some(),
            }
    }
    fn send(&mut self, command: Command) {
        if self.busy || self.demo {
            return;
        }
        self.busy = true;
        if self.worker.tx.send(command).is_err() {
            self.busy = false;
            self.error = Some("The connection worker stopped. Restart TokWatch.".into());
        }
        unsafe {
            self.render();
        }
    }
    fn refresh(&mut self) {
        if self.login_pending {
            self.send(Command::CheckLogin);
        } else {
            self.send(Command::Refresh(
                self.settings.provider,
                self.settings.codex_path.clone(),
            ));
        }
    }
    fn refresh_seconds(&self) -> u64 {
        if self.settings.provider == Provider::Claude {
            15
        } else {
            self.settings.poll_seconds()
        }
    }
    fn check_updates(&mut self) {
        if self.demo || self.update_rx.is_some() || self.update_ready.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        self.update_status = "Checking GitHub for updates…".into();
        self.next_update_check = Instant::now() + updater::CHECK_INTERVAL;
        let handle = self.hwnd.0 as isize;
        thread::spawn(move || {
            let _ = tx.send(updater::check_and_download());
            unsafe {
                let _ = PostMessageW(Some(HWND(handle as *mut _)), RESULT, WPARAM(0), LPARAM(0));
            }
        });
    }
    fn switch_provider(&mut self, provider: Provider) {
        if self.busy || self.login_pending || self.settings.provider == provider {
            return;
        }
        self.settings.provider = provider;
        self.settings.selected_window = None;
        self.claude_connected = !self.demo && claude::is_configured();
        self.snapshot = if self.demo {
            Some(demo_snapshot_for(provider))
        } else {
            match provider {
                Provider::Codex => settings::load_cache().ok().flatten(),
                Provider::Claude => claude::read_limits().ok(),
            }
        };
        self.verified = self.demo;
        self.signed_out = provider == Provider::Claude && !self.claude_connected;
        self.error = None;
        self.failures = 0;
        self.status = format!("Connecting to {}…", provider.name());
        self.icon_key.clear();
        self.save_settings();
        self.next_refresh = Instant::now();
        self.refresh();
    }
    unsafe fn create_controls(&mut self) {
        unsafe {
            self.fonts = FONT_SPECS
                .iter()
                .map(|(size, weight)| self.font(*size, *weight))
                .collect();
            if let Some(host) =
                (GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) as *const Host).as_ref()
            {
                host.theme.set(PaintTheme::from_app(self));
            }
            for (i, (x, y, width, height, font)) in LABEL_SPECS.iter().enumerate() {
                let style = WS_CHILD
                    | WS_VISIBLE
                    | WINDOW_STYLE(SS_OWNERDRAW.0 | SS_ENDELLIPSIS.0 | SS_NOPREFIX.0);
                let child = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!(""),
                    style,
                    self.px(*x),
                    self.px(*y),
                    self.px(*width),
                    self.px(*height),
                    Some(self.hwnd),
                    Some(HMENU((300 + i) as *mut _)),
                    None,
                    None,
                )
                .unwrap_or_default();
                SendMessageW(
                    child,
                    WM_SETFONT,
                    Some(WPARAM(self.fonts[*font].0 as usize)),
                    Some(LPARAM(1)),
                );
                self.labels.push(child);
                self.label_texts.borrow_mut().push(String::new());
            }
            for (id, name, x) in [(REFRESH, "Refresh usage", 12), (SETTINGS, "Settings", 166)] {
                let text = wide(name);
                let child = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    PCWSTR(text.as_ptr()),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                    self.px(x),
                    self.px(250),
                    self.px(142),
                    self.px(30),
                    Some(self.hwnd),
                    Some(HMENU(id as usize as *mut _)),
                    None,
                    None,
                )
                .unwrap_or_default();
                SendMessageW(
                    child,
                    WM_SETFONT,
                    Some(WPARAM(self.fonts[1].0 as usize)),
                    Some(LPARAM(1)),
                );
                let _ = SetWindowSubclass(child, Some(button_proc), 1, 0);
                self.buttons.push(child);
            }
        }
    }
    unsafe fn font(&self, size: i32, weight: i32) -> HFONT {
        unsafe {
            CreateFontW(
                -self.px(size),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                DEFAULT_PITCH.0 as u32,
                w!("Segoe UI"),
            )
        }
    }
    unsafe fn set_label(&self, index: usize, value: &str) {
        {
            let mut text = self.label_texts.borrow_mut();
            if text[index] == value {
                return;
            }
            text[index] = value.to_owned();
        }
        unsafe {
            let _ = SetWindowTextW(self.labels[index], PCWSTR(wide(value).as_ptr()));
        }
    }
    unsafe fn render(&mut self) {
        unsafe {
            let now = model::unix_now();
            self.last_render_second = now;
            let selected = self.selected();
            let pool_name = selected
                .map(|(pool, _)| pool.name.as_str())
                .unwrap_or(self.settings.provider.name());
            let heading = self
                .snapshot
                .as_ref()
                .and_then(|s| s.plan())
                .filter(|plan| !plan.trim().is_empty())
                .map_or_else(
                    || pool_name.to_owned(),
                    |plan| format!("{pool_name} · {}", title_case(plan)),
                );
            self.set_label(0, &heading);
            self.set_label(
                1,
                &selected
                    .map(|(_, w)| w.duration_label())
                    .unwrap_or_else(|| "Usage".into()),
            );
            self.set_label(
                2,
                &selected
                    .and_then(|(_, w)| w.remaining_percent())
                    .map(|n| format!("{n}%"))
                    .unwrap_or_else(|| "Usage unavailable".into()),
            );
            self.set_label(
                3,
                if self.stale() && self.snapshot.is_some() {
                    "last known"
                } else {
                    "remaining"
                },
            );
            let hero_note = if self.stale() && self.snapshot.is_some() {
                "Cached reading".into()
            } else if self.settings.provider == Provider::Claude {
                "Via Claude Code".into()
            } else if self.snapshot.is_some() {
                format!("Every {} min", self.settings.poll_seconds() / 60)
            } else {
                "Usage unavailable".into()
            };
            self.set_label(4, &hero_note);
            self.set_label(5, "Next reset");
            self.set_label(
                6,
                &selected
                    .and_then(|(_, w)| w.resets_at.map(|_| w.reset_countdown(now)))
                    .unwrap_or_else(|| "—".into()),
            );
            self.set_label(
                7,
                &selected
                    .and_then(|(_, w)| w.resets_at)
                    .map(short_local_date)
                    .unwrap_or_else(|| "Time unavailable".into()),
            );
            let credits = self
                .snapshot
                .as_ref()
                .and_then(|s| s.reset_credits.as_ref());
            self.set_label(8, "Full resets");
            self.set_label(
                9,
                &credits
                    .and_then(|c| c.available_count)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
            );
            self.set_label(
                10,
                &credits
                    .and_then(|c| c.next_expiry())
                    .map(|t| {
                        if t <= now {
                            "Expiry due".into()
                        } else {
                            format!("Expires in {}", model::countdown(Some(t), now))
                        }
                    })
                    .unwrap_or_else(|| {
                        if credits.and_then(|c| c.available_count) == Some(0) {
                            "None available".into()
                        } else {
                            "Expiry unavailable".into()
                        }
                    }),
            );
            if self.settings.provider == Provider::Claude {
                let other = self
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.pools.first())
                    .and_then(|pool| {
                        pool.windows.iter().find(|window| {
                            Some(window.key.as_str()) != selected.map(|(_, w)| w.key.as_str())
                        })
                    });
                self.set_label(
                    8,
                    &other
                        .map(|w| w.duration_label())
                        .unwrap_or_else(|| "Other window".into()),
                );
                self.set_label(
                    9,
                    &other
                        .and_then(|w| w.remaining_percent())
                        .map(|n| format!("{n}%"))
                        .unwrap_or_else(|| "—".into()),
                );
                self.set_label(
                    10,
                    &other
                        .filter(|w| w.resets_at.is_some())
                        .map(|w| format!("Resets in {}", w.reset_countdown(now)))
                        .unwrap_or_else(|| "Reset not reported".into()),
                );
            }
            self.set_label(
                11,
                if self.settings.provider == Provider::Claude {
                    "Source"
                } else {
                    "Extra credits"
                },
            );
            let balance = self
                .snapshot
                .as_ref()
                .and_then(|s| s.extra_credits())
                .map(|c| {
                    if c.unlimited == Some(true) {
                        "Unlimited".into()
                    } else {
                        c.balance
                            .as_deref()
                            .map(display_balance)
                            .unwrap_or_else(|| "—".into())
                    }
                })
                .unwrap_or_else(|| "—".into());
            self.set_label(
                12,
                if self.settings.provider == Provider::Claude {
                    "Claude Code"
                } else {
                    &balance
                },
            );
            let status = if let Some(error) = &self.error {
                error.clone()
            } else if self.busy {
                if self.login_pending {
                    "Checking your sign-in…".into()
                } else {
                    "Updating your allowance…".into()
                }
            } else if self.login_pending {
                "Finish signing in in your browser.".into()
            } else if self.installer_rx.is_some() {
                "Preparing to restart for update…".into()
            } else if let Some(update) = &self.update_ready {
                format!("{} ready · Settings to restart", update.version)
            } else if self.settings.provider == Provider::Claude
                && self
                    .selected()
                    .is_none_or(|(_, w)| w.remaining_percent().is_none())
            {
                "Waiting for Claude Code usage".into()
            } else if let Some(s) = &self.snapshot {
                let age = now.saturating_sub(s.fetched_at);
                format!(
                    "{}{}",
                    if self.demo {
                        "Preview · "
                    } else if self.stale() {
                        "Cached · "
                    } else {
                        ""
                    },
                    if age < 10 {
                        "Updated just now".into()
                    } else if age < 60 {
                        format!("Updated {age}s ago")
                    } else {
                        format!("Updated {}m ago", age / 60)
                    }
                )
            } else {
                self.status.clone()
            };
            self.set_label(13, &status);
            let button = if self.login_pending {
                "Signing in…"
            } else if self.primary_connect() {
                if self.settings.provider == Provider::Claude {
                    "Connect Claude"
                } else {
                    "Connect Codex"
                }
            } else {
                "Refresh usage"
            };
            let _ = SetWindowTextW(self.buttons[0], PCWSTR(wide(button).as_ptr()));
            let _ = EnableWindow(
                self.buttons[0],
                !self.busy && !self.login_pending && (!self.demo || self.snapshot.is_some()),
            );
            self.retarget_meter();
            self.repaint_motion();
            self.update_tray();
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            for button in &self.buttons {
                let _ = InvalidateRect(Some(*button), None, false);
            }
        }
    }
    fn nid(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            ..Default::default()
        }
    }
    unsafe fn update_tray(&mut self) {
        unsafe {
            let amount = self.selected().and_then(|(_, w)| w.remaining_percent());
            let unknown = self.stale();
            let text = if unknown {
                "?".into()
            } else {
                amount
                    .map(|n| format!("{n}%"))
                    .unwrap_or_else(|| "?".into())
            };
            let icon_dpi = tray_icon_dpi(self.hwnd, self.dpi);
            let key = format!(
                "{text}-{}-{}-{}-{}-{}",
                self.dark,
                icon_dpi,
                self.settings.provider.name(),
                self.palette().accent.0,
                self.appearance.high_contrast
            );
            if self.icon_key == key && self.tray_added {
                return;
            }
            if !self.icon.is_invalid() {
                let _ = DestroyIcon(self.icon);
            }
            self.icon = ui_tray::make_icon(
                &text,
                self.dark,
                icon_dpi,
                self.appearance.accent,
                self.appearance.high_contrast,
            );
            self.icon_key = key;
            let mut nid = self.nid();
            nid.uFlags = NIF_MESSAGE | NIF_TIP | NIF_ICON | NIF_SHOWTIP;
            nid.uCallbackMessage = TRAY;
            nid.hIcon = self.icon;
            let tip = wide(&format!(
                "TokWatch · {}\n{}{}",
                self.settings.provider.name(),
                if unknown { "Last known: " } else { "" },
                amount
                    .map(|n| format!("{n}% remaining"))
                    .unwrap_or_else(|| "Usage unavailable".into())
            ));
            let length = tip.len().min(nid.szTip.len() - 1);
            nid.szTip[..length].copy_from_slice(&tip[..length]);
            if self.tray_added {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            } else if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                self.tray_added = true;
                nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
                // The icon rectangle becomes available only after registration.
                if tray_icon_dpi(self.hwnd, self.dpi) != icon_dpi {
                    self.update_tray();
                }
            }
        }
    }
    unsafe fn add_tray(&mut self) {
        self.tray_added = false;
        self.icon_key.clear();
        unsafe {
            self.update_tray();
        }
    }
    unsafe fn resize_panel(&mut self, panel_dpi: u32) {
        if self.panel_dpi == panel_dpi {
            return;
        }
        unsafe {
            self.panel_dpi = panel_dpi;
            for child in self.labels.drain(..).chain(self.buttons.drain(..)) {
                let _ = DestroyWindow(child);
            }
            self.label_texts.borrow_mut().clear();
            for font in self.fonts.drain(..) {
                let _ = DeleteObject(font.into());
            }
            self.create_controls();
        }
    }
    fn motion_enabled(&self) -> bool {
        self.appearance.animations
    }
    unsafe fn repaint_motion(&self) {
        unsafe {
            if let Some(host) =
                (GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) as *const Host).as_ref()
            {
                host.theme.set(PaintTheme::from_app(self));
            }
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            for button in &self.buttons {
                let _ = InvalidateRect(Some(*button), None, false);
            }
        }
    }
    unsafe fn retarget_meter(&mut self) {
        let target = self
            .selected()
            .and_then(|(_, w)| w.remaining_percent())
            .unwrap_or(0) as f32;
        if self.meter_motion.is_some_and(|t| t.target() == target) {
            return;
        }
        if (target - self.meter_value).abs() < 0.01 {
            self.meter_motion = None;
            return;
        }
        if self.visible && self.motion_enabled() {
            self.meter_motion = Some(ui_motion::Tween::new(
                self.meter_value,
                target,
                Instant::now(),
                240,
            ));
            unsafe {
                SetTimer(Some(self.hwnd), MOTION_TICK, 16, None);
            }
        } else {
            self.meter_value = target;
            self.meter_motion = None;
        }
    }
    unsafe fn hover_changed(&mut self) {
        unsafe {
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            for (index, button) in self.buttons.iter().enumerate() {
                let mut rect = RECT::default();
                let _ = GetWindowRect(*button, &mut rect);
                let hovered = self.visible
                    && point.x >= rect.left
                    && point.x < rect.right
                    && point.y >= rect.top
                    && point.y < rect.bottom;
                let target = if hovered { 1.0 } else { 0.0 };
                if self.hover_motion[index].is_some_and(|t| t.target() == target) {
                    continue;
                }
                if self.motion_enabled() && (target - self.hover[index]).abs() > 0.01 {
                    self.hover_motion[index] = Some(ui_motion::Tween::new(
                        self.hover[index],
                        target,
                        Instant::now(),
                        100,
                    ));
                    SetTimer(Some(self.hwnd), MOTION_TICK, 16, None);
                } else {
                    self.hover[index] = target;
                    self.hover_motion[index] = None;
                }
            }
            self.repaint_motion();
        }
    }
    unsafe fn finish_motion(&mut self) {
        unsafe {
            if self.panel_motion.is_some_and(|t| t.target() == 0.0) {
                self.finish_hide();
            } else {
                self.panel_motion = None;
                self.panel_value = if self.visible { 1.0 } else { 0.0 };
                if self.visible {
                    let _ = SetWindowPos(
                        self.hwnd,
                        None,
                        self.panel_bounds.left,
                        self.panel_bounds.top,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
                    );
                }
            }
            if let Some(tween) = self.meter_motion.take() {
                self.meter_value = tween.target();
            }
            for index in 0..2 {
                if let Some(tween) = self.hover_motion[index].take() {
                    self.hover[index] = tween.target();
                }
            }
            let _ = KillTimer(Some(self.hwnd), MOTION_TICK);
            self.repaint_motion();
        }
    }
    unsafe fn animate(&mut self) {
        unsafe {
            let now = Instant::now();
            if let Some(tween) = self.panel_motion {
                let (value, done) = tween.sample(now);
                self.panel_value = value;
                let rect = self.panel_bounds;
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    rect.left,
                    rect.top + ((1.0 - value) * self.panel_offset as f32).round() as i32,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
                );
                if done {
                    self.panel_motion = None;
                    if tween.target() == 0.0 {
                        self.finish_hide();
                        return;
                    }
                }
            }
            if let Some(tween) = self.meter_motion {
                let (value, done) = tween.sample(now);
                self.meter_value = value;
                if done {
                    self.meter_motion = None;
                }
            }
            for index in 0..2 {
                if let Some(tween) = self.hover_motion[index] {
                    let (value, done) = tween.sample(now);
                    self.hover[index] = value;
                    if done {
                        self.hover_motion[index] = None;
                    }
                }
            }
            self.repaint_motion();
            if self.panel_motion.is_none()
                && self.meter_motion.is_none()
                && self.hover_motion.iter().all(Option::is_none)
            {
                let _ = KillTimer(Some(self.hwnd), MOTION_TICK);
            }
        }
    }
    unsafe fn show(&mut self, pinned: bool) {
        unsafe {
            let was_visible = self.visible;
            self.pinned |= pinned;
            self.visible = true;
            let rect = tray_rect(self.hwnd).unwrap_or_else(|| {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                RECT {
                    left: p.x,
                    top: p.y,
                    right: p.x + 1,
                    bottom: p.y + 1,
                }
            });
            let (work, dpi) = panel_monitor(&rect, self.dpi);
            self.dpi = dpi;
            let (panel_dpi, bounds) = panel_geometry(dpi, work, PanelAnchor::Tray(rect));
            self.resize_panel(panel_dpi);
            self.panel_bounds = bounds;
            self.panel_offset = if bounds.bottom <= rect.top {
                self.px(12).min(work.bottom - bounds.bottom)
            } else {
                -self.px(12).min(bounds.top - work.top)
            };
            if self.motion_enabled() {
                if !was_visible {
                    self.panel_value = 0.0;
                    self.meter_value = 0.0;
                    self.meter_motion = None;
                }
                self.panel_motion = Some(ui_motion::Tween::new(
                    self.panel_value,
                    1.0,
                    Instant::now(),
                    180,
                ));
                SetTimer(Some(self.hwnd), MOTION_TICK, 16, None);
            } else {
                self.panel_value = 1.0;
                self.panel_motion = None;
            }
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                bounds.left,
                bounds.top + ((1.0 - self.panel_value) * self.panel_offset as f32).round() as i32,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_NOACTIVATE,
            );
            let _ = ShowWindow(self.hwnd, if pinned { SW_SHOW } else { SW_SHOWNOACTIVATE });
            if pinned {
                let _ = SetForegroundWindow(self.hwnd);
            }
            SetTimer(Some(self.hwnd), PANEL_TICK, 1000, None);
            self.render();
        }
    }
    unsafe fn finish_hide(&mut self) {
        unsafe {
            self.visible = false;
            self.pinned = false;
            self.panel_value = 0.0;
            self.panel_motion = None;
            self.meter_motion = None;
            self.hover_motion = [None; 2];
            self.hover = [0.0; 2];
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            let _ = KillTimer(Some(self.hwnd), PANEL_TICK);
            let _ = KillTimer(Some(self.hwnd), MOTION_TICK);
            ui_render::release();
        }
    }
    unsafe fn hide(&mut self) {
        unsafe {
            self.pinned = false;
            if self.visible && self.motion_enabled() {
                if !self.panel_motion.is_some_and(|t| t.target() == 0.0) {
                    self.panel_motion = Some(ui_motion::Tween::new(
                        self.panel_value,
                        0.0,
                        Instant::now(),
                        100,
                    ));
                    SetTimer(Some(self.hwnd), MOTION_TICK, 16, None);
                }
            } else {
                self.finish_hide();
            }
        }
    }
    unsafe fn menu(&mut self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            for (id, provider) in [
                (PROVIDER_CODEX, Provider::Codex),
                (PROVIDER_CLAUDE, Provider::Claude),
            ] {
                let flags = MF_STRING
                    | if self.settings.provider == provider {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    }
                    | if self.busy || self.login_pending {
                        MF_GRAYED
                    } else {
                        MF_ENABLED
                    };
                let name = wide(match provider {
                    Provider::Codex => "Monitor Codex",
                    Provider::Claude => "Monitor Claude Code",
                });
                let _ = AppendMenuW(menu, flags, id as usize, PCWSTR(name.as_ptr()));
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, REFRESH as usize, w!("Refresh now"));
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.can_connect() {
                        MF_ENABLED
                    } else {
                        MF_GRAYED
                    },
                CONNECT as usize,
                PCWSTR(
                    wide(if self.settings.provider == Provider::Claude {
                        "Connect Claude Code…"
                    } else {
                        "Connect to Codex…"
                    })
                    .as_ptr(),
                ),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CONNECTION_STATUS as usize,
                w!("Connection details…"),
            );
            self.menu_keys.clear();
            if let Some(s) = &self.snapshot {
                let selected = self.selected().map(|(_, w)| w.key.clone());
                for pool in &s.pools {
                    for window in &pool.windows {
                        if self.menu_keys.len() >= 500 {
                            break;
                        }
                        let name = wide(&format!("{} / {}", pool.name, window.duration_label()));
                        let flags = MF_STRING
                            | if selected.as_deref() == Some(&window.key) {
                                MF_CHECKED
                            } else {
                                MF_UNCHECKED
                            };
                        let _ = AppendMenuW(
                            menu,
                            flags,
                            WINDOW_BASE as usize + self.menu_keys.len(),
                            PCWSTR(name.as_ptr()),
                        );
                        self.menu_keys.push(window.key.clone());
                    }
                }
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            for (i, seconds) in [60u64, 120, 300, 600]
                .iter()
                .enumerate()
                .filter(|_| self.settings.provider == Provider::Codex)
            {
                let name = wide(&format!("Refresh every {} min", seconds / 60));
                let flags = MF_STRING
                    | if self.settings.poll_seconds() == *seconds {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    };
                let _ = AppendMenuW(
                    menu,
                    flags,
                    INTERVAL_BASE as usize + i,
                    PCWSTR(name.as_ptr()),
                );
            }
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if startup_enabled() {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    },
                STARTUP as usize,
                w!("Start with Windows"),
            );
            let path_flags = MF_STRING
                | if self.busy || self.login_pending || self.settings.provider != Provider::Codex {
                    MF_GRAYED
                } else {
                    MF_ENABLED
                };
            let _ = AppendMenuW(
                menu,
                path_flags,
                PICK_CODEX as usize,
                w!("Choose Codex executable…"),
            );
            let _ = AppendMenuW(
                menu,
                path_flags,
                AUTO_CODEX as usize,
                w!("Detect Codex automatically"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                OPEN_FOLDER as usize,
                w!("Open settings folder"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let update_text = if self.update_rx.is_some() {
                "Checking for updates…"
            } else {
                "Check for updates"
            };
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.update_rx.is_some() || self.demo {
                        MF_GRAYED
                    } else {
                        MF_ENABLED
                    },
                CHECK_UPDATES as usize,
                PCWSTR(wide(update_text).as_ptr()),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.settings.automatic_updates {
                        MF_CHECKED
                    } else {
                        MF_UNCHECKED
                    },
                AUTO_UPDATES as usize,
                w!("Automatic updates"),
            );
            if let Some(update) = &self.update_ready {
                let name = wide(&format!("Restart to update to {}", update.version));
                let _ = AppendMenuW(
                    menu,
                    MF_STRING,
                    INSTALL_UPDATE as usize,
                    PCWSTR(name.as_ptr()),
                );
            }
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                UPDATE_STATUS as usize,
                w!("Update status…"),
            );
            let _ = AppendMenuW(menu, MF_STRING, ABOUT as usize, w!("About TokWatch"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, EXIT as usize, w!("Exit"));
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let _ = SetForegroundWindow(self.hwnd);
            let chosen = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                p.x,
                p.y,
                None,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            if chosen.0 != 0 {
                self.command(chosen.0 as u16);
            }
        }
    }
    unsafe fn command(&mut self, id: u16) {
        unsafe {
            match id {
                REFRESH => {
                    if self.primary_connect() {
                        self.command(CONNECT);
                    } else {
                        self.refresh();
                    }
                }
                CONNECT => {
                    if self.can_connect() {
                        self.verified = false;
                        self.snapshot = None;
                        self.error = None;
                        if self.settings.provider == Provider::Codex {
                            let _ = settings::clear_cache();
                        }
                        self.send(Command::Connect(
                            self.settings.provider,
                            self.settings.codex_path.clone(),
                        ));
                    }
                }
                SETTINGS => self.menu(),
                PROVIDER_CODEX => self.switch_provider(Provider::Codex),
                PROVIDER_CLAUDE => self.switch_provider(Provider::Claude),
                CONNECTION_STATUS => {
                    let details = if let Some(error) = &self.error {
                        error.clone()
                    } else if self.settings.provider == Provider::Claude {
                        "Claude subscription allowance comes from Claude Code's local status line. Connect Claude Code, then use a Pro or Max session. Readings update during Claude Code sessions; TokWatch checks the local feed every 15 seconds. Existing custom status lines are preserved.".into()
                    } else {
                        self.status.clone()
                    };
                    let _ = MessageBoxW(
                        Some(self.hwnd),
                        PCWSTR(wide(&details).as_ptr()),
                        w!("TokWatch connection"),
                        MB_OK,
                    );
                }
                CHECK_UPDATES => self.check_updates(),
                AUTO_UPDATES => {
                    self.settings.automatic_updates = !self.settings.automatic_updates;
                    self.save_settings();
                    if self.settings.automatic_updates {
                        self.check_updates();
                    }
                }
                UPDATE_STATUS => {
                    let details = wide(&self.update_status);
                    let _ = MessageBoxW(
                        Some(self.hwnd),
                        PCWSTR(details.as_ptr()),
                        w!("TokWatch updates"),
                        MB_OK,
                    );
                }
                INSTALL_UPDATE => {
                    if !self.demo && self.installer_rx.is_none() {
                        if let Some(update) = self.update_ready.clone() {
                            let (tx, rx) = mpsc::channel();
                            self.installer_rx = Some(rx);
                            let handle = self.hwnd.0 as isize;
                            thread::spawn(move || {
                                let _ = tx.send(updater::launch_installer(&update));
                                let _ = PostMessageW(
                                    Some(HWND(handle as *mut _)),
                                    RESULT,
                                    WPARAM(0),
                                    LPARAM(0),
                                );
                            });
                        }
                    }
                }
                STARTUP => {
                    if let Err(e) = set_startup(!startup_enabled()) {
                        self.error = Some(e);
                    }
                }
                OPEN_FOLDER => {
                    if let Ok(path) = settings::data_dir() {
                        open(&path.to_string_lossy());
                    }
                }
                PICK_CODEX => {
                    if self.busy || self.login_pending {
                        return;
                    }
                    let mut path = [0u16; 32768];
                    let filter = wide("Codex executable\0*.exe\0\0");
                    let mut dialog = OPENFILENAMEW {
                        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
                        hwndOwner: self.hwnd,
                        lpstrFilter: PCWSTR(filter.as_ptr()),
                        lpstrFile: windows::core::PWSTR(path.as_mut_ptr()),
                        nMaxFile: path.len() as u32,
                        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
                        ..Default::default()
                    };
                    if GetOpenFileNameW(&mut dialog).as_bool() {
                        self.settings.codex_path = Some(PathBuf::from(String::from_utf16_lossy(
                            &path[..path.iter().position(|&c| c == 0).unwrap_or(path.len())],
                        )));
                        self.verified = false;
                        self.save_settings();
                        self.refresh();
                    }
                }
                AUTO_CODEX => {
                    if self.busy || self.login_pending {
                        return;
                    }
                    self.settings.codex_path = None;
                    self.verified = false;
                    self.save_settings();
                    self.refresh();
                }
                ABOUT => {
                    let _ = MessageBoxW(
                        Some(self.hwnd),
                        w!(
                            "TokWatch 0.3.0\n\nNative Windows monitor for Codex and Claude allowance.\nRust + Win32. Account readings stay local.\n\nCodex uses its official helper. Claude uses its local Claude Code status line.\n\nClick for details. Escape to dismiss.\nGitHub updates are verified before installation."
                        ),
                        w!("About TokWatch"),
                        MB_OK,
                    );
                }
                EXIT => {
                    let _ = DestroyWindow(self.hwnd);
                    return;
                }
                n if (WINDOW_BASE..WINDOW_BASE + 500).contains(&n) => {
                    if let Some(key) = self.menu_keys.get((n - WINDOW_BASE) as usize) {
                        self.settings.selected_window = Some(key.clone());
                        self.save_settings();
                    }
                }
                n if (INTERVAL_BASE..INTERVAL_BASE + 4).contains(&n) => {
                    self.settings.poll_seconds = [60, 120, 300, 600][(n - INTERVAL_BASE) as usize];
                    self.next_refresh =
                        Instant::now() + Duration::from_secs(self.refresh_seconds());
                    self.save_settings();
                }
                _ => {}
            }
            self.render();
        }
    }
    fn save_settings(&mut self) {
        if self.demo {
            return;
        }
        if let Err(e) = self.settings.save() {
            self.error = Some(e);
        }
    }
    unsafe fn receive(&mut self) {
        unsafe {
            while let Ok(update) = self.worker.rx.try_recv() {
                self.busy = false;
                match update {
                    Update::Snapshot(snapshot) => {
                        self.login_pending = false;
                        self.signed_out = false;
                        self.verified = true;
                        self.error = None;
                        self.failures = 0;
                        if self.settings.provider == Provider::Codex {
                            if let Err(e) = settings::save_cache(&snapshot) {
                                self.error = Some(e);
                            }
                        }
                        self.snapshot = Some(snapshot);
                        self.next_refresh =
                            Instant::now() + Duration::from_secs(self.refresh_seconds());
                    }
                    Update::ClaudeConnected => {
                        self.claude_connected = true;
                        self.signed_out = false;
                        self.error = None;
                        self.status = "Use Claude Code to load allowance.".into();
                        self.next_refresh = Instant::now() + Duration::from_secs(15);
                    }
                    Update::SignedOut(api) => {
                        self.signed_out = true;
                        self.snapshot = None;
                        self.verified = false;
                        self.error = None;
                        self.login_pending = false;
                        let _ = settings::clear_cache();
                        self.status = if api {
                            "Codex uses an API key. Connect with ChatGPT to see subscription usage."
                        } else {
                            "Choose Connect to sign in with your ChatGPT account."
                        }
                        .into();
                        self.next_refresh =
                            Instant::now() + Duration::from_secs(self.refresh_seconds());
                    }
                    Update::Error(e) => {
                        self.error = Some(e);
                        self.login_pending = false;
                        self.failures = self.failures.saturating_add(1);
                        self.next_refresh = Instant::now()
                            + Duration::from_secs(if self.settings.provider == Provider::Claude {
                                15
                            } else {
                                (self.settings.poll_seconds()
                                    * 2u64.saturating_pow(self.failures.min(4)))
                                .min(1800)
                            });
                    }
                    Update::OpenLogin(url) => {
                        self.error = None;
                        self.login_pending = true;
                        if !open_auth(&url) {
                            self.error=Some("Could not open Codex sign-in. Check your default browser and try again.".into());
                        }
                        self.next_refresh = Instant::now() + Duration::from_secs(5);
                    }
                    Update::Pending => {
                        self.next_refresh = Instant::now() + Duration::from_secs(5);
                    }
                }
            }
            let install = self
                .installer_rx
                .as_ref()
                .and_then(|rx| match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                        "The installer could not start. Try Restart to update again.".into(),
                    )),
                    Err(mpsc::TryRecvError::Empty) => None,
                });
            if let Some(result) = install {
                self.installer_rx = None;
                match result {
                    Ok(()) => {
                        let _ = DestroyWindow(self.hwnd);
                        return;
                    }
                    Err(error) => {
                        self.update_ready = None;
                        self.update_status = error;
                        self.command(UPDATE_STATUS);
                    }
                }
            }
            let update = self.update_rx.as_ref().and_then(|rx| match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                    "The update worker stopped. Check for updates again.".into(),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            });
            if let Some(result) = update {
                self.update_rx = None;
                match result {
                    Ok(Some(ready)) => {
                        self.update_status = format!(
                            "{} has been downloaded and verified. Choose Restart to update in Settings.",
                            ready.version
                        );
                        self.update_ready = Some(ready);
                    }
                    Ok(None) => {
                        self.update_status =
                            format!("TokWatch {} is up to date.", env!("CARGO_PKG_VERSION"))
                    }
                    Err(error) => self.update_status = error,
                }
            }
            self.render();
        }
    }
}
#[derive(Clone, Copy)]
struct PaintTheme {
    background: COLORREF,
    foreground: COLORREF,
    muted: COLORREF,
    brush: HBRUSH,
    surface_brush: HBRUSH,
    surface: COLORREF,
    dpi: u32,
    button_font: HFONT,
    colors: ui_style::Palette,
    glass: bool,
    hover: [f32; 2],
}
impl PaintTheme {
    fn from_app(app: &App) -> Self {
        Self {
            background: app.bg(),
            foreground: app.fg(),
            muted: app.muted(),
            brush: app.brush,
            surface_brush: app.surface_brush,
            surface: app.palette().surface,
            dpi: app.panel_dpi,
            button_font: app.fonts.get(1).copied().unwrap_or_default(),
            colors: app.palette(),
            glass: app.glass,
            hover: app.hover,
        }
    }
}
#[derive(Clone, Copy)]
struct DeferredMessage {
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    dpi_rect: Option<RECT>,
}
/// The window procedure never creates a mutable reference through GWLP_USERDATA.
/// Native calls may synchronously enter it again while App is already borrowed.
struct Host {
    app: RefCell<App>,
    theme: Cell<PaintTheme>,
    deferred: RefCell<Vec<DeferredMessage>>,
    repaint: Cell<bool>,
    drain_posted: Cell<bool>,
    destroyed: Cell<bool>,
    taskbar_created: u32,
}
const DRAIN: u32 = WM_APP + 3;
impl Host {
    fn defer(&self, event: DeferredMessage) {
        let mut queue = self.deferred.borrow_mut();
        queue.retain(|old| {
            if event.message == WM_TIMER {
                old.message != WM_TIMER || old.wparam.0 != event.wparam.0
            } else if matches!(
                event.message,
                RESULT | WM_ACTIVATE | WM_DPICHANGED | WM_DISPLAYCHANGE | CONTROL_CHANGED
            ) || event.message == self.taskbar_created
            {
                old.message != event.message
            } else if matches!(
                event.message,
                WM_SETTINGCHANGE
                    | WM_THEMECHANGED
                    | WM_DWMCOLORIZATIONCOLORCHANGED
                    | WM_DWMCOMPOSITIONCHANGED
                    | APPEARANCE_CHANGED
            ) {
                !matches!(
                    old.message,
                    WM_SETTINGCHANGE
                        | WM_THEMECHANGED
                        | WM_DWMCOLORIZATIONCOLORCHANGED
                        | WM_DWMCOMPOSITIONCHANGED
                        | APPEARANCE_CHANGED
                )
            } else {
                true
            }
        });
        if queue.len() >= 64 {
            let discard = queue
                .iter()
                .position(|old| matches!(old.message, WM_TIMER | RESULT))
                .unwrap_or(0);
            queue.remove(discard);
        }
        queue.push(event);
    }
    /// Publish work only after the outer callback releases its App borrow.
    /// A single posted drain preserves owned DPI rectangles and prevents a
    /// nested modal loop from continuously reposting deferred timer messages.
    unsafe fn flush(&self, hwnd: HWND) {
        unsafe {
            if self.destroyed.get() {
                self.deferred.borrow_mut().clear();
                self.repaint.set(false);
                return;
            }
            if self.app.try_borrow().is_err() {
                return;
            }
            if self.repaint.replace(false) {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            if !self.deferred.borrow().is_empty()
                && !self.drain_posted.replace(true)
                && PostMessageW(Some(hwnd), DRAIN, WPARAM(0), LPARAM(0)).is_err()
            {
                self.drain_posted.set(false);
            }
        }
    }
}
unsafe fn new_host(
    hwnd: HWND,
    settings: Settings,
    snapshot: Option<UsageSnapshot>,
    demo: bool,
    setting_error: Option<String>,
) -> Box<Host> {
    unsafe {
        let dark = system_dark();
        let appearance = ui_theme::Appearance::read();
        let dpi = GetDpiForWindow(hwnd).max(96);
        let app = App {
            hwnd,
            worker: worker(hwnd),
            settings,
            snapshot,
            verified: demo,
            status: "Waiting for an allowance reading…".into(),
            error: setting_error,
            busy: false,
            login_pending: false,
            next_refresh: Instant::now(),
            failures: 0,
            pinned: false,
            visible: false,
            demo,
            dark,
            appearance,
            glass: false,
            panel_bounds: RECT::default(),
            panel_offset: 0,
            panel_value: 0.0,
            panel_motion: None,
            meter_value: 0.0,
            meter_motion: None,
            hover: [0.0; 2],
            hover_motion: [None; 2],
            dpi,
            panel_dpi: dpi,
            brush: CreateSolidBrush(ui_style::palette(dark).bg),
            surface_brush: CreateSolidBrush(ui_style::palette(dark).surface),
            signed_out: false,
            fonts: Vec::new(),
            labels: Vec::new(),
            buttons: Vec::new(),
            icon: HICON::default(),
            icon_key: String::new(),
            tray_added: false,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            menu_keys: Vec::new(),
            label_texts: RefCell::new(Vec::new()),
            last_render_second: 0,
            claude_connected: !demo && claude::is_configured(),
            update_rx: None,
            update_ready: None,
            installer_rx: None,
            update_status: if demo {
                "Updates are disabled in demo mode.".into()
            } else {
                updater::read_install_error()
                    .unwrap_or_else(|| "Updates have not been checked yet.".into())
            },
            next_update_check: Instant::now(),
        };
        let theme = PaintTheme::from_app(&app);
        let taskbar_created = app.taskbar_created;
        Box::new(Host {
            app: RefCell::new(app),
            theme: Cell::new(theme),
            deferred: RefCell::new(Vec::new()),
            repaint: Cell::new(false),
            drain_posted: Cell::new(false),
            destroyed: Cell::new(false),
            taskbar_created,
        })
    }
}
pub fn run() {
    let _runtime = ui_theme::Runtime::new();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex =
            CreateMutexW(None, false, w!("Local\\TokWatch.Native.Tray.v1")).unwrap_or_default();
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(existing) = FindWindowW(w!("TokWatch.Native.Window"), None) {
                let _ = PostMessageW(Some(existing), SHOW_PANEL, WPARAM(0), LPARAM(0));
            }
            if !mutex.is_invalid() {
                let _ = CloseHandle(mutex);
            }
            return;
        }
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("TokWatch.Native.Window"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = match CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("TokWatch.Native.Window"),
            w!("TokWatch — usage"),
            WS_POPUP | WS_BORDER | WS_CLIPCHILDREN,
            0,
            0,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        ) {
            Ok(h) => h,
            Err(_) => {
                let _ = MessageBoxW(
                    None,
                    w!("TokWatch could not create its tray window."),
                    w!("TokWatch"),
                    MB_ICONERROR,
                );
                if !mutex.is_invalid() {
                    let _ = CloseHandle(mutex);
                }
                return;
            }
        };
        let (mut settings, setting_error) = match Settings::load() {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(e)),
        };
        let demo_claude = std::env::args().any(|s| s == "--demo-claude");
        let demo = demo_claude || std::env::args().any(|s| s == "--demo");
        if demo_claude {
            settings.provider = Provider::Claude;
        }
        let snapshot = if demo {
            Some(demo_snapshot_for(settings.provider))
        } else {
            match settings.provider {
                Provider::Codex => settings::load_cache().ok().flatten(),
                Provider::Claude => claude::read_limits().ok(),
            }
        };
        let host = new_host(hwnd, settings, snapshot, demo, setting_error);
        let _appearance_observer = ui_theme::Observer::new(hwnd);
        {
            let mut app = host.app.borrow_mut();
            app.glass = apply_window_style(hwnd, app.dark, app.appearance);
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&*host as *const Host) as isize);
        {
            let mut app = host.app.borrow_mut();
            app.create_controls();
            app.add_tray();
            app.render();
            SetTimer(Some(hwnd), TICK, 5000, None);
            if demo {
                app.show(true);
            } else {
                app.refresh();
            }
            if std::env::args().any(|s| s == "--show") {
                app.show(true);
            }
        }
        host.flush(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if msg.message == WM_KEYDOWN && msg.wParam.0 == 0x1b {
                {
                    host.app.borrow_mut().hide();
                }
                host.flush(hwnd);
                continue;
            }
            if msg.message == WM_KEYDOWN && msg.wParam.0 == 0x0d {
                let focused = GetFocus();
                let id = GetDlgCtrlID(focused);
                if matches!(id, 101 | 103) && IsChild(hwnd, focused).as_bool() {
                    let _ = PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(id as usize), LPARAM(0));
                    continue;
                }
            }
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        // Disconnect native callbacks before dropping the stable Host allocation.
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
        let mut app = host.app.borrow_mut();
        // Finish helper cleanup before releasing the singleton mutex, which
        // the installer uses to decide that replacement can proceed.
        app.worker.stop();
        let _ = Shell_NotifyIconW(NIM_DELETE, &app.nid());
        if !app.icon.is_invalid() {
            let _ = DestroyIcon(app.icon);
        }
        for font in &app.fonts {
            let _ = DeleteObject((*font).into());
        }
        let _ = DeleteObject(app.brush.into());
        let _ = DeleteObject(app.surface_brush.into());
        if !mutex.is_invalid() {
            let _ = CloseHandle(mutex);
        }
    }
}
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_ERASEBKGND {
            return LRESULT(1);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Host;
        if message == WM_DESTROY {
            if let Some(host) = pointer.as_ref() {
                host.destroyed.set(true);
            }
            PostQuitMessage(0);
            return LRESULT(0);
        }
        let Some(host) = pointer.as_ref() else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        if message == WM_NCDESTROY {
            host.destroyed.set(true);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        if message == WM_DRAWITEM && lparam.0 != 0 {
            let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
            let theme = host.theme.get();
            let mut text = [0u16; 2048];
            let length = GetWindowTextW(item.hwndItem, &mut text);
            let text = String::from_utf16_lossy(&text[..length as usize]);
            if (300..300 + LABEL_SPECS.len() as u32).contains(&item.CtlID) {
                ui_style::paint_label(theme, item, (item.CtlID - 300) as usize, &text);
                return LRESULT(1);
            }
            ui_style::paint_button(
                theme,
                item,
                &text,
                item.CtlID == REFRESH as u32,
                theme.hover[usize::from(item.CtlID == SETTINGS as u32)],
            );
            return LRESULT(1);
        }
        if message == WM_CTLCOLORSTATIC {
            let theme = host.theme.get();
            let dc = HDC(wparam.0 as *mut _);
            let id = GetDlgCtrlID(HWND(lparam.0 as *mut _)) - 300;
            let card = (0..=10).contains(&id);
            let _ = SetBkColor(
                dc,
                if card {
                    theme.surface
                } else {
                    theme.background
                },
            );
            let ink = if matches!(id, 0 | 2 | 6 | 9 | 12) {
                theme.foreground
            } else {
                theme.muted
            };
            let _ = SetTextColor(dc, ink);
            return LRESULT(if card {
                theme.surface_brush.0
            } else {
                theme.brush.0
            } as isize);
        }
        let actionable = matches!(
            message,
            TRAY | RESULT
                | SHOW_PANEL
                | DRAIN
                | WM_COMMAND
                | WM_TIMER
                | WM_PAINT
                | WM_PRINTCLIENT
                | WM_CLOSE
                | WM_ACTIVATE
                | WM_SETTINGCHANGE
                | WM_THEMECHANGED
                | WM_DWMCOLORIZATIONCOLORCHANGED
                | WM_DWMCOMPOSITIONCHANGED
                | APPEARANCE_CHANGED
                | CONTROL_CHANGED
                | WM_DPICHANGED
                | WM_DISPLAYCHANGE
                | WM_POWERBROADCAST
        ) || (host.taskbar_created != 0 && message == host.taskbar_created);
        if !actionable {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        // Windows owns the suggested rectangle only for this invocation.
        let dpi_rect = if message == WM_DPICHANGED && lparam.0 != 0 {
            Some(*(lparam.0 as *const RECT))
        } else {
            None
        };
        let event = DeferredMessage {
            message,
            wparam,
            lparam: if message == TRAY { lparam } else { LPARAM(0) },
            dpi_rect,
        };
        let result = match host.app.try_borrow_mut() {
            Ok(mut app) => {
                if message == DRAIN {
                    host.drain_posted.set(false);
                    let deferred = std::mem::take(&mut *host.deferred.borrow_mut());
                    for event in deferred {
                        if host.destroyed.get() {
                            break;
                        }
                        dispatch(&mut app, host, hwnd, event);
                    }
                    LRESULT(0)
                } else {
                    dispatch(&mut app, host, hwnd, event)
                }
            }
            Err(_) => {
                if message == WM_PAINT {
                    // Validate the region now; repaint after the outer borrow
                    // ends, rather than spin in a nested native modal loop.
                    let mut ps = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut ps);
                    FillRect(dc, &ps.rcPaint, host.theme.get().brush);
                    let _ = EndPaint(hwnd, &ps);
                    host.repaint.set(true);
                } else if message == WM_PRINTCLIENT {
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    FillRect(HDC(wparam.0 as *mut _), &rect, host.theme.get().brush);
                } else if message == DRAIN {
                    host.drain_posted.set(false);
                } else {
                    host.defer(event);
                }
                return LRESULT(if message == WM_POWERBROADCAST { 1 } else { 0 });
            }
        };
        // The RefMut temporary above is gone before this can post new work.
        host.flush(hwnd);
        result
    }
}
unsafe fn paint_panel(app: &App, dc: HDC, region: &RECT) {
    unsafe {
        ui_style::paint_panel(app, dc, region);
    }
}
unsafe fn dispatch(app: &mut App, host: &Host, hwnd: HWND, event: DeferredMessage) -> LRESULT {
    unsafe {
        let DeferredMessage {
            message,
            wparam,
            lparam,
            dpi_rect,
        } = event;
        if message == app.taskbar_created {
            app.add_tray();
            return LRESULT(0);
        }
        match message {
            TRAY => {
                match lparam.0 as u32 & 0xffff {
                    NIN_POPUPOPEN | NIN_POPUPCLOSE => {}
                    NIN_SELECT | NIN_KEYSELECT => {
                        if app.visible && app.pinned {
                            app.hide();
                        } else {
                            app.show(true);
                        }
                    }
                    WM_CONTEXTMENU => app.menu(),
                    _ => {}
                }
                LRESULT(0)
            }
            RESULT => {
                app.receive();
                LRESULT(0)
            }
            WM_COMMAND => {
                app.command((wparam.0 & 0xffff) as u16);
                LRESULT(0)
            }
            CONTROL_CHANGED => {
                app.hover_changed();
                LRESULT(0)
            }
            WM_TIMER => {
                if wparam.0 == MOTION_TICK {
                    app.animate();
                } else if wparam.0 == TICK {
                    app.update_tray();
                    if !app.demo
                        && app.settings.automatic_updates
                        && Instant::now() >= app.next_update_check
                    {
                        app.check_updates();
                    }
                    if !app.demo && Instant::now() >= app.next_refresh {
                        app.refresh();
                    }
                } else if wparam.0 == PANEL_TICK
                    && app.visible
                    && app.last_render_second != model::unix_now()
                {
                    app.render();
                }
                LRESULT(0)
            }
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                paint_panel(app, dc, &ps.rcPaint);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_PRINTCLIENT => {
                let mut rect = RECT::default();
                let _ = GetClientRect(hwnd, &mut rect);
                paint_panel(app, HDC(wparam.0 as *mut _), &rect);
                LRESULT(0)
            }
            WM_CLOSE => {
                app.hide();
                LRESULT(0)
            }
            WM_ACTIVATE => {
                if wparam.0 & 0xffff == WA_INACTIVE as usize && app.pinned {
                    app.hide();
                }
                LRESULT(0)
            }
            WM_SETTINGCHANGE
            | WM_THEMECHANGED
            | WM_DWMCOLORIZATIONCOLORCHANGED
            | WM_DWMCOMPOSITIONCHANGED
            | APPEARANCE_CHANGED => {
                app.dark = system_dark();
                app.appearance = ui_theme::Appearance::read();
                app.icon_key.clear();
                let replacement = CreateSolidBrush(app.bg());
                let surface = CreateSolidBrush(app.palette().surface);
                if !replacement.is_invalid() && !surface.is_invalid() {
                    let previous = app.brush;
                    let previous_surface = app.surface_brush;
                    app.brush = replacement;
                    app.surface_brush = surface;
                    host.theme.set(PaintTheme::from_app(app));
                    let _ = DeleteObject(previous.into());
                    let _ = DeleteObject(previous_surface.into());
                } else {
                    if !replacement.is_invalid() {
                        let _ = DeleteObject(replacement.into());
                    }
                    if !surface.is_invalid() {
                        let _ = DeleteObject(surface.into());
                    }
                }
                app.glass = apply_window_style(hwnd, app.dark, app.appearance);
                if !app.motion_enabled() {
                    app.finish_motion();
                }
                host.theme.set(PaintTheme::from_app(app));
                app.render();
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                app.update_tray();
                LRESULT(0)
            }
            WM_DPICHANGED => {
                app.dpi = ((wparam.0 & 0xffff) as u32).max(96);
                let suggested = dpi_rect.unwrap_or_else(|| {
                    let mut rect = RECT::default();
                    let _ = GetWindowRect(hwnd, &mut rect);
                    rect
                });
                let (work, _) = panel_monitor(&suggested, app.dpi);
                let (panel_dpi, bounds) =
                    panel_geometry(app.dpi, work, PanelAnchor::Window(suggested));
                app.resize_panel(panel_dpi);
                app.panel_bounds = bounds;
                app.panel_motion = None;
                app.panel_value = 1.0;
                app.icon_key.clear();
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    bounds.left,
                    bounds.top,
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
                app.render();
                LRESULT(0)
            }
            WM_POWERBROADCAST if wparam.0 == 18 || wparam.0 == 7 => {
                app.next_refresh = Instant::now();
                if !app.demo {
                    app.refresh();
                }
                LRESULT(1)
            }
            _ => LRESULT(0),
        }
    }
}
unsafe fn tray_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        Shell_NotifyIconGetRect(&NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        })
        .ok()
    }
}
unsafe fn tray_icon_dpi(hwnd: HWND, fallback_dpi: u32) -> u32 {
    unsafe {
        // The flyout may be on another monitor, or have its scale constrained
        // to fit the work area. Render for the notification area's monitor.
        let anchor = tray_rect(hwnd).or_else(|| {
            let taskbar = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
            let mut rect = RECT::default();
            GetWindowRect(taskbar, &mut rect).ok()?;
            Some(rect)
        });
        let Some(anchor) = anchor else {
            return fallback_dpi.max(96);
        };
        let monitor = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
        let mut dpi_x = fallback_dpi;
        let mut dpi_y = fallback_dpi;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        dpi_x.max(96)
    }
}
unsafe extern "system" fn button_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    unsafe {
        if message == WM_MOUSEMOVE {
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut track);
            let _ = InvalidateRect(Some(hwnd), None, false);
        } else if message == WM_MOUSELEAVE {
            let _ = InvalidateRect(Some(hwnd), None, false);
        } else if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(button_proc), 1);
        }
        let result = DefSubclassProc(hwnd, message, wparam, lparam);
        if matches!(
            message,
            WM_MOUSEMOVE
                | WM_MOUSELEAVE
                | WM_SETFOCUS
                | WM_KILLFOCUS
                | WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | BM_SETSTATE
                | WM_ENABLE
        ) {
            if let Ok(parent) = GetParent(hwnd) {
                let _ = PostMessageW(Some(parent), CONTROL_CHANGED, WPARAM(0), LPARAM(0));
            }
        }
        result
    }
}
fn apply_window_style(hwnd: HWND, dark: bool, appearance: ui_theme::Appearance) -> bool {
    unsafe {
        let round = if appearance.high_contrast {
            DWMWCP_DONOTROUND
        } else {
            DWMWCP_ROUND
        };
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&round as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
            std::mem::size_of_val(&round) as u32,
        );
        let mode = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&mode as *const BOOL).cast(),
            std::mem::size_of_val(&mode) as u32,
        );
        let border = 0xffffffffu32; // Let Windows draw the current system border.
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, (&border as *const u32).cast(), 4);
        let material = if appearance.transparency {
            DWMSBT_TRANSIENTWINDOW
        } else {
            DWMSBT_NONE
        };
        let applied = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&material as *const DWM_SYSTEMBACKDROP_TYPE).cast(),
            std::mem::size_of_val(&material) as u32,
        )
        .is_ok();
        let glass = appearance.transparency && applied;
        let margins = if glass {
            windows::Win32::UI::Controls::MARGINS {
                cxLeftWidth: -1,
                cxRightWidth: -1,
                cyTopHeight: -1,
                cyBottomHeight: -1,
            }
        } else {
            Default::default()
        };
        let extended = DwmExtendFrameIntoClientArea(hwnd, &margins).is_ok();
        glass && extended
    }
}
fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}
fn display_balance(value: &str) -> String {
    match value.parse::<f64>() {
        Ok(number) if number.is_finite() && number.abs() < 1_000_000_000.0 => {
            format!("{number:.2}")
        }
        _ => value.into(),
    }
}
fn short_local_date(timestamp: u64) -> String {
    let full = local_date(timestamp);
    if full.len() < 19 || !full.is_ascii() || &full[4..5] != "-" {
        return "Time unavailable".into();
    }
    let month = full[5..7].parse::<usize>().unwrap_or(0);
    let names = [
        "", "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if month == 0 || month > 12 {
        return "Time unavailable".into();
    }
    format!(
        "{} {} · {}",
        names[month],
        full[8..10].trim_start_matches('0'),
        &full[14..19]
    )
}
fn system_dark() -> bool {
    unsafe {
        let mut value = 1u32;
        let mut size = 4u32;
        let _ = RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut _ as *mut _),
            Some(&mut size),
        );
        value == 0
    }
}
fn startup_enabled() -> bool {
    unsafe {
        let mut size = 0;
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("TokWatch"),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .is_ok()
    }
}
fn set_startup(enabled: bool) -> Result<(), String> {
    unsafe {
        let value = if enabled {
            let exe = std::env::current_exe()
                .map_err(|_| "Could not find TokWatch executable.".to_string())?;
            Some(wide(&format!("\"{}\"", exe.display())))
        } else {
            None
        };
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()
        .map_err(|_| "Could not update Windows startup.".to_string())?;
        let result = if let Some(value) = value {
            RegSetValueExW(
                key,
                w!("TokWatch"),
                None,
                REG_SZ,
                Some(std::slice::from_raw_parts(
                    value.as_ptr() as *const u8,
                    value.len() * 2,
                )),
            )
            .ok()
        } else {
            RegDeleteValueW(key, w!("TokWatch")).ok()
        };
        let _ = RegCloseKey(key);
        result.map_err(|_| "Could not update Windows startup.".into())
    }
}
unsafe fn open(target: &str) {
    unsafe {
        let _ = ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide(target).as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}
fn open_auth(url: &str) -> bool {
    if !url.starts_with("https://auth.openai.com/")
        && !url.starts_with("https://auth0.openai.com/")
        && !url.starts_with("https://chatgpt.com/")
    {
        return false;
    }
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide(url).as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
        .0 as isize
            > 32
    }
}
fn local_date(timestamp: u64) -> String {
    unsafe {
        let Some(ticks) = timestamp
            .checked_mul(10_000_000)
            .and_then(|t| t.checked_add(116_444_736_000_000_000))
        else {
            return "Reset date unavailable".into();
        };
        let file = FILETIME {
            dwLowDateTime: ticks as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        let mut utc = SYSTEMTIME::default();
        let mut time = SYSTEMTIME::default();
        if FileTimeToSystemTime(&file, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut time).is_err()
        {
            return "Reset date unavailable".into();
        }
        format!(
            "{:04}-{:02}-{:02} at {:02}:{:02} (local)",
            time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute
        )
    }
}
fn demo_snapshot_for(provider: Provider) -> UsageSnapshot {
    if provider == Provider::Codex {
        return demo_snapshot();
    }
    let now = model::unix_now();
    UsageSnapshot {
        fetched_at: now,
        pools: vec![model::UsagePool {
            id: "claude".into(),
            name: "Claude".into(),
            plan: None,
            credits: None,
            windows: vec![
                model::UsageWindow {
                    key: "claude:five_hour".into(),
                    kind: "five_hour".into(),
                    used_percent: Some(28.0),
                    duration_mins: Some(300),
                    resets_at: Some(now + 9000),
                },
                model::UsageWindow {
                    key: "claude:seven_day".into(),
                    kind: "seven_day".into(),
                    used_percent: Some(14.0),
                    duration_mins: Some(10080),
                    resets_at: Some(now + 345600),
                },
            ],
        }],
        reset_credits: None,
    }
}
fn demo_snapshot() -> UsageSnapshot {
    let now = model::unix_now();
    UsageSnapshot::from_response(serde_json::json!({
        "rateLimitsByLimitId":{"codex":{"limitId":"codex","limitName":"Codex","primary":{"usedPercent":74,"windowDurationMins":10080,"resetsAt":now+374400},"planType":"pro","credits":{"balance":"35.28"}}},
        "rateLimitResetCredits":{"availableCount":2,"credits":[{"status":"available","expiresAt":now+1987200}]}
    })).expect("valid bundled demo")
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
