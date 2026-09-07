use crate::{
    codex::{self, AccountStatus, Client},
    model::{self, UsageSnapshot},
    settings::{self, Settings},
};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
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
const PANEL_HEIGHT: i32 = 320;
const FONT_SPECS: [(i32, i32); 5] = [(16, 600), (12, 400), (40, 600), (20, 600), (11, 500)];
const LABEL_SPECS: [(i32, i32, i32, i32, usize); 16] = [
    (46, 14, 180, 22, 0),
    (24, 59, 181, 17, 1),
    (250, 19, 48, 15, 4),
    (214, 59, 80, 17, 4),
    (24, 78, 130, 55, 2),
    (169, 94, 125, 18, 1),
    (169, 115, 125, 16, 4),
    (16, 159, 139, 17, 4),
    (16, 178, 139, 27, 3),
    (16, 207, 139, 16, 4),
    (171, 159, 133, 17, 4),
    (171, 178, 133, 27, 3),
    (171, 207, 133, 16, 4),
    (16, 232, 140, 18, 1),
    (175, 229, 129, 22, 0),
    (28, 256, 276, 16, 4),
];
const NIN_KEYSELECT: u32 = 0x401;
const TRAY: u32 = WM_APP + 1;
const RESULT: u32 = WM_APP + 2;
const SHOW_PANEL: u32 = WM_APP + 4;
const TICK: usize = 1;
const HOVER: usize = 2;
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
    Refresh(Option<PathBuf>),
    Connect(Option<PathBuf>),
    CheckLogin,
    Stop,
}
enum Update {
    Snapshot(UsageSnapshot),
    SignedOut(bool),
    Error(String),
    OpenLogin(String),
    Pending,
}
struct Worker {
    tx: Sender<Command>,
    rx: Receiver<Update>,
}
fn worker(hwnd: HWND) -> Worker {
    let (tx, commands) = mpsc::channel();
    let (results, rx) = mpsc::channel();
    let handle = hwnd.0 as isize;
    thread::spawn(move || {
        let mut login: Option<Client> = None;
        let mut login_started: Option<Instant> = None;
        let connect = |path: Option<PathBuf>| -> Result<Client, String> {
            Client::spawn(&match path {
                Some(p) => p,
                None => codex::discover_codex()?,
            })
        };
        while let Ok(command) = commands.recv() {
            if matches!(command, Command::Stop) {
                break;
            }
            let result: Result<Update, String> = (|| match command {
                Command::Stop => Ok(Update::Pending),
                Command::Refresh(path) => {
                    login = None;
                    login_started = None;
                    let mut client = connect(path)?;
                    match client.read_account()? {
                        AccountStatus::SignedIn { .. } => {
                            Ok(Update::Snapshot(client.read_limits()?))
                        }
                        AccountStatus::SignedOut => Ok(Update::SignedOut(false)),
                        AccountStatus::ApiKey => Ok(Update::SignedOut(true)),
                    }
                }
                Command::Connect(path) => {
                    let mut client = connect(path)?;
                    let url = client.start_login()?;
                    login = Some(client);
                    login_started = Some(Instant::now());
                    Ok(Update::OpenLogin(url))
                }
                Command::CheckLogin => {
                    if login_started.is_some_and(|t| t.elapsed() > Duration::from_secs(300)) {
                        login = None;
                        login_started = None;
                        return Err("Sign-in timed out. Choose Connect to try again.".into());
                    }
                    let Some(client) = login.as_mut() else {
                        return Ok(Update::Pending);
                    };
                    if client.poll_login()? {
                        let snapshot = client.read_limits()?;
                        login = None;
                        login_started = None;
                        Ok(Update::Snapshot(snapshot))
                    } else {
                        Ok(Update::Pending)
                    }
                }
            })();
            let update = result.unwrap_or_else(|e| {
                login = None;
                login_started = None;
                Update::Error(e)
            });
            if results.send(update).is_err() {
                break;
            }
            unsafe {
                let _ = PostMessageW(Some(HWND(handle as *mut _)), RESULT, WPARAM(0), LPARAM(0));
            }
        }
    });
    Worker { tx, rx }
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
    taskbar_created: u32,
    menu_keys: Vec<String>,
    label_texts: RefCell<Vec<String>>,
    last_render_second: u64,
}
impl App {
    fn px(&self, n: i32) -> i32 {
        n * self.panel_dpi as i32 / 96
    }
    fn palette(&self) -> ui_style::Palette {
        ui_style::palette(self.dark)
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
    fn can_connect(&self) -> bool {
        !self.busy && !self.login_pending && !self.demo && (!self.verified || self.error.is_some())
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
            self.send(Command::Refresh(self.settings.codex_path.clone()));
        }
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
            for (id, name, x) in [(REFRESH, "Refresh usage", 16), (SETTINGS, "Settings", 164)] {
                let text = wide(name);
                let child = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("BUTTON"),
                    PCWSTR(text.as_ptr()),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                    self.px(x),
                    self.px(278),
                    self.px(140),
                    self.px(28),
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
            self.set_label(0, "TokWatch");
            self.set_label(
                2,
                &self
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.plan())
                    .map(title_case)
                    .unwrap_or_else(|| "Codex".into()),
            );
            let selected = self.selected();
            self.set_label(
                1,
                selected
                    .map(|(pool, _)| pool.name.as_str())
                    .unwrap_or("Codex"),
            );
            self.set_label(
                3,
                &selected
                    .map(|(_, w)| w.duration_label())
                    .unwrap_or_else(|| "Usage".into()),
            );
            self.set_label(
                4,
                &selected
                    .and_then(|(_, w)| w.remaining_percent())
                    .map(|n| format!("{n}%"))
                    .unwrap_or_else(|| "—".into()),
            );
            self.set_label(
                5,
                if self.stale() && self.snapshot.is_some() {
                    "last known"
                } else {
                    "remaining"
                },
            );
            let hero_note = if self.stale() && self.snapshot.is_some() {
                "Cached reading".into()
            } else if self.snapshot.is_some() {
                format!("Every {} min", self.settings.poll_seconds() / 60)
            } else {
                "Usage unavailable".into()
            };
            self.set_label(6, &hero_note);
            self.set_label(7, "Next reset");
            self.set_label(
                8,
                &selected
                    .and_then(|(_, w)| w.resets_at.map(|_| w.reset_countdown(now)))
                    .unwrap_or_else(|| "—".into()),
            );
            self.set_label(
                9,
                &selected
                    .and_then(|(_, w)| w.resets_at)
                    .map(short_local_date)
                    .unwrap_or_else(|| "Time unavailable".into()),
            );
            let credits = self
                .snapshot
                .as_ref()
                .and_then(|s| s.reset_credits.as_ref());
            self.set_label(10, "Full resets");
            self.set_label(
                11,
                &credits
                    .and_then(|c| c.available_count)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
            );
            self.set_label(
                12,
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
            self.set_label(13, "Extra credits");
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
            self.set_label(14, &balance);
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
            self.set_label(15, &status);
            let button = if self.login_pending {
                "Signing in…"
            } else if self.signed_out {
                "Connect Codex"
            } else {
                "Refresh usage"
            };
            let _ = SetWindowTextW(self.buttons[0], PCWSTR(wide(button).as_ptr()));
            let _ = EnableWindow(
                self.buttons[0],
                !self.busy && !self.login_pending && (!self.demo || self.snapshot.is_some()),
            );
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
            let key = format!("{text}-{}-{}", self.dark, self.dpi);
            if self.icon_key == key {
                return;
            }
            if !self.icon.is_invalid() {
                let _ = DestroyIcon(self.icon);
            }
            self.icon = ui_tray::make_icon(&text, self.dark, self.dpi);
            self.icon_key = key;
            let mut nid = self.nid();
            nid.uFlags = NIF_TIP | NIF_ICON;
            nid.hIcon = self.icon;
            let tip = wide(&format!(
                "TokWatch\n{}{}",
                if unknown { "Last known: " } else { "" },
                amount
                    .map(|n| format!("{n}% remaining"))
                    .unwrap_or_else(|| "Usage unavailable".into())
            ));
            let length = tip.len().min(nid.szTip.len() - 1);
            nid.szTip[..length].copy_from_slice(&tip[..length]);
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
    unsafe fn add_tray(&mut self) {
        unsafe {
            if self.icon.is_invalid() {
                self.icon = ui_tray::make_icon("?", self.dark, self.dpi);
            }
            let mut nid = self.nid();
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.uCallbackMessage = TRAY;
            nid.hIcon = self.icon;
            nid.szTip[..9].copy_from_slice(&wide("TokWatch")[..9]);
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
            self.icon_key.clear();
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
    unsafe fn show(&mut self, pinned: bool) {
        unsafe {
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
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                bounds.left,
                bounds.top,
                bounds.right - bounds.left,
                bounds.bottom - bounds.top,
                SWP_NOACTIVATE,
            );
            let _ = ShowWindow(self.hwnd, if pinned { SW_SHOW } else { SW_SHOWNOACTIVATE });
            if pinned {
                let _ = SetForegroundWindow(self.hwnd);
            }
            SetTimer(Some(self.hwnd), HOVER, 250, None);
            self.render();
        }
    }
    unsafe fn hide(&mut self) {
        unsafe {
            self.pinned = false;
            self.visible = false;
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            let _ = KillTimer(Some(self.hwnd), HOVER);
            ui_render::release();
        }
    }
    unsafe fn menu(&mut self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
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
                w!("Connect to Codex…"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
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
            for (i, seconds) in [60u64, 120, 300, 600].iter().enumerate() {
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
                | if self.busy || self.login_pending {
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
                    if self.signed_out {
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
                        let _ = settings::clear_cache();
                        self.send(Command::Connect(self.settings.codex_path.clone()));
                    }
                }
                SETTINGS => self.menu(),
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
                            "TokWatch 0.1.0\n\nNative Windows monitor for Codex usage.\nRust + Win32. Account data stays local.\n\nUsage is a percentage of your allowance, not an exact token balance.\nRequires a compatible signed-in Codex installation.\n\nHover for details. Click to keep open. Escape to dismiss."
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
                        Instant::now() + Duration::from_secs(self.settings.poll_seconds());
                    self.save_settings();
                }
                _ => {}
            }
            self.render();
        }
    }
    fn save_settings(&mut self) {
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
                        if let Err(e) = settings::save_cache(&snapshot) {
                            self.error = Some(e);
                        }
                        self.snapshot = Some(snapshot);
                        self.next_refresh =
                            Instant::now() + Duration::from_secs(self.settings.poll_seconds());
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
                            Instant::now() + Duration::from_secs(self.settings.poll_seconds());
                    }
                    Update::Error(e) => {
                        self.error = Some(e);
                        self.login_pending = false;
                        self.failures = self.failures.saturating_add(1);
                        self.next_refresh = Instant::now()
                            + Duration::from_secs(
                                (self.settings.poll_seconds()
                                    * 2u64.saturating_pow(self.failures.min(4)))
                                .min(1800),
                            );
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
    accent: COLORREF,
    dpi: u32,
    dark: bool,
    button_font: HFONT,
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
            accent: app.palette().accent,
            dpi: app.panel_dpi,
            dark: app.dark,
            button_font: app.fonts.get(1).copied().unwrap_or_default(),
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
            } else if matches!(event.message, RESULT | WM_ACTIVATE | WM_DPICHANGED)
                || event.message == self.taskbar_created
            {
                old.message != event.message
            } else if matches!(event.message, WM_SETTINGCHANGE | WM_THEMECHANGED) {
                !matches!(old.message, WM_SETTINGCHANGE | WM_THEMECHANGED)
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
        let dpi = GetDpiForWindow(hwnd).max(96);
        let app = App {
            hwnd,
            worker: worker(hwnd),
            settings,
            snapshot,
            verified: demo,
            status: "Connecting to Codex…".into(),
            error: setting_error,
            busy: false,
            login_pending: false,
            next_refresh: Instant::now(),
            failures: 0,
            pinned: false,
            visible: false,
            demo,
            dark,
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
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            menu_keys: Vec::new(),
            label_texts: RefCell::new(Vec::new()),
            last_render_second: 0,
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
            w!("TokWatch — Codex usage"),
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
        let (settings, setting_error) = match Settings::load() {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(e)),
        };
        let demo = std::env::args().any(|s| s == "--demo");
        let snapshot = if demo {
            Some(demo_snapshot())
        } else {
            settings::load_cache().ok().flatten()
        };
        let host = new_host(hwnd, settings, snapshot, demo, setting_error);
        apply_window_style(hwnd, host.app.borrow().dark);
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
        let app = host.app.borrow_mut();
        let _ = app.worker.tx.send(Command::Stop);
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
            if (300..316).contains(&item.CtlID) {
                ui_style::paint_label(
                    theme.dpi,
                    theme.dark,
                    item,
                    (item.CtlID - 300) as usize,
                    &text,
                );
                return LRESULT(1);
            }
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let mut rect = RECT::default();
            let _ = GetWindowRect(item.hwndItem, &mut rect);
            let hovered = IsWindowVisible(hwnd).as_bool()
                && point.x >= rect.left
                && point.x < rect.right
                && point.y >= rect.top
                && point.y < rect.bottom;
            ui_style::paint_button(
                theme.dpi,
                theme.dark,
                theme.button_font,
                item,
                &text,
                item.CtlID == REFRESH as u32,
                hovered,
            );
            return LRESULT(1);
        }
        if message == WM_CTLCOLORSTATIC {
            let theme = host.theme.get();
            let dc = HDC(wparam.0 as *mut _);
            let id = GetDlgCtrlID(HWND(lparam.0 as *mut _)) - 300;
            let card = (1..=6).contains(&id);
            let _ = SetBkColor(
                dc,
                if card {
                    theme.surface
                } else {
                    theme.background
                },
            );
            let ink = if id == 2 {
                theme.accent
            } else if matches!(id, 0 | 4 | 8 | 11 | 14) {
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
                | WM_DPICHANGED
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
            SHOW_PANEL => {
                app.show(true);
                LRESULT(0)
            }
            TRAY => {
                match lparam.0 as u32 & 0xffff {
                    NIN_POPUPOPEN => app.show(false),
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
            WM_TIMER => {
                if wparam.0 == TICK {
                    if !app.demo && Instant::now() >= app.next_refresh {
                        app.refresh();
                    }
                    if !app.visible {
                        app.update_tray();
                    }
                } else if wparam.0 == HOVER {
                    if !app.pinned {
                        let mut p = POINT::default();
                        let _ = GetCursorPos(&mut p);
                        let mut rect = RECT::default();
                        let _ = GetWindowRect(hwnd, &mut rect);
                        let inside = |r: RECT| {
                            p.x >= r.left && p.x <= r.right && p.y >= r.top && p.y <= r.bottom
                        };
                        if !inside(rect) && !tray_rect(hwnd).is_some_and(inside) {
                            app.hide();
                        }
                    }
                    if app.visible && app.last_render_second != model::unix_now() {
                        app.render();
                    }
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
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
                app.dark = system_dark();
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
                apply_window_style(hwnd, app.dark);
                app.render();
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
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}
fn apply_window_style(hwnd: HWND, dark: bool) {
    unsafe {
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &round as *const _ as *const _,
            std::mem::size_of_val(&round) as u32,
        );
        let mode = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &mode as *const _ as *const _,
            std::mem::size_of_val(&mode) as u32,
        );
        let border = ui_style::palette(dark).border;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &border as *const _ as *const _,
            std::mem::size_of_val(&border) as u32,
        );
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
