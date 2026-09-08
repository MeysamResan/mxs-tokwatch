// Native UI regression checks create only their own test windows. No account calls.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Fixture {
    Normal,
    Missing,
    Long,
    Claude,
}

impl Fixture {
    fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Missing => "missing",
            Self::Long => "long",
            Self::Claude => "claude",
        }
    }

    fn snapshot(self) -> UsageSnapshot {
        match self {
            Self::Normal => demo_snapshot(),
            Self::Claude => demo_snapshot_for(Provider::Claude),
            Self::Missing => UsageSnapshot::from_response(serde_json::json!({
                "rateLimits": {"primary": {"windowDurationMins": 10080}}
            }))
            .unwrap(),
            Self::Long => {
                let mut snapshot = demo_snapshot();
                for window in &mut snapshot.pools[0].windows {
                    window.used_percent = Some(0.0);
                }
                snapshot.pools[0].name =
                    "Codex usage with a very long descriptive account allowance name ".repeat(2);
                snapshot.pools[0].plan =
                    Some("A long enterprise subscription plan description".into());
                snapshot.pools[0].credits.as_mut().unwrap().balance =
                    Some("1234567890123456789012345678901234567890.12345678901234567890".into());
                snapshot
            }
        }
    }
}

#[test]
fn native_controls_fit_in_both_themes_at_common_dpi_scales() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).unwrap();
        let name = w!("TokWatch.Native.LayoutTest");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: name,
            ..Default::default()
        });
        for dpi in [96u32, 120, 144, 168, 192] {
            for dark in [false, true] {
                for fixture in [
                    Fixture::Normal,
                    Fixture::Missing,
                    Fixture::Long,
                    Fixture::Claude,
                ] {
                    let hwnd = CreateWindowExW(
                        WS_EX_TOOLWINDOW,
                        name,
                        w!("TokWatch layout test"),
                        WS_POPUP | WS_CLIPCHILDREN,
                        160,
                        120,
                        PANEL_WIDTH * dpi as i32 / 96,
                        PANEL_HEIGHT * dpi as i32 / 96,
                        None,
                        None,
                        Some(instance.into()),
                        None,
                    )
                    .unwrap();
                    let host = new_host(
                        hwnd,
                        Settings {
                            provider: if matches!(fixture, Fixture::Claude) {
                                Provider::Claude
                            } else {
                                Provider::Codex
                            },
                            ..Settings::default()
                        },
                        Some(fixture.snapshot()),
                        true,
                        None,
                    );
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&*host as *const Host) as isize);
                    {
                        let mut borrowed = host.app.borrow_mut();
                        let app = &mut *borrowed;
                        app.dpi = dpi;
                        app.panel_dpi = dpi;
                        app.dark = dark;
                        let _ = DeleteObject(app.brush.into());
                        let _ = DeleteObject(app.surface_brush.into());
                        app.brush = CreateSolidBrush(app.bg());
                        app.surface_brush = CreateSolidBrush(ui_style::palette(dark).surface);
                        if matches!(fixture, Fixture::Long) {
                            app.error = Some(
                                "The local Codex connection is unavailable. Check the configured application path and sign in again through Codex before refreshing your account usage. Additional connection details should remain available without expanding or clipping this compact panel."
                                    .into(),
                            );
                        }
                        app.create_controls();
                        host.theme.set(PaintTheme::from_app(app));
                        app.render();
                        assert_eq!(app.labels.len(), 14);
                        assert_eq!(app.buttons.len(), 2);
                        verify_control_bounds(app, dpi, dark, fixture);
                        verify_text_layout(app, dpi, dark, fixture);
                        verify_control_semantics(app);
                        match fixture {
                            Fixture::Normal => {
                                assert_eq!(label_text(app.labels[0]), "Codex · Pro");
                                assert_eq!(label_text(app.labels[2]), "26%");
                                assert_eq!(label_text(app.labels[9]), "2");
                            }
                            Fixture::Missing => {
                                assert_eq!(label_text(app.labels[2]), "Usage unavailable");
                                assert_eq!(label_text(app.labels[9]), "—");
                            }
                            Fixture::Long => {
                                assert_eq!(label_text(app.labels[2]), "100%");
                            }
                            Fixture::Claude => {
                                assert_eq!(label_text(app.labels[2]), "72%");
                                assert_eq!(label_text(app.labels[9]), "86%");
                                assert_eq!(label_text(app.labels[12]), "Claude Code");
                            }
                        }
                    }
                    host.flush(hwnd);
                    // Capture both native and fractional-scale renderings. All 40
                    // configurations use the renderer's actual DirectWrite metrics.
                    if matches!(dpi, 96 | 144) {
                        capture_test_panel(hwnd, &host, dark, fixture);
                        if matches!(fixture, Fixture::Normal) {
                            // Check the actual alpha bytes delivered by parent and
                            // native child DCs; a solid fallback must not mask glass.
                            {
                                let mut app = host.app.borrow_mut();
                                app.glass = true;
                                host.theme.set(PaintTheme::from_app(&app));
                            }
                            capture_test_panel(hwnd, &host, dark, fixture);
                        }
                    }
                    destroy_test_panel(hwnd, &host);
                }
            }
        }
        let _ = UnregisterClassW(name, Some(instance.into()));
    }
}

unsafe fn verify_control_bounds(app: &App, dpi: u32, dark: bool, fixture: Fixture) {
    unsafe {
        let mut client = RECT::default();
        GetClientRect(app.hwnd, &mut client).unwrap();
        let mut positions = Vec::new();
        for control in app.labels.iter().chain(&app.buttons) {
            let mut screen = RECT::default();
            GetWindowRect(*control, &mut screen).unwrap();
            let mut top_left = POINT {
                x: screen.left,
                y: screen.top,
            };
            let mut bottom_right = POINT {
                x: screen.right,
                y: screen.bottom,
            };
            assert!(ScreenToClient(app.hwnd, &mut top_left).as_bool());
            assert!(ScreenToClient(app.hwnd, &mut bottom_right).as_bool());
            assert!(
                top_left.x >= 0
                    && top_left.y >= 0
                    && bottom_right.x <= client.right
                    && bottom_right.y <= client.bottom,
                "control outside {dpi}-DPI panel (dark={dark}, {fixture:?}): {screen:?}"
            );
            positions.push((top_left, bottom_right));
        }
        // Native children should not cover one another even after DPI rounding.
        for (index, (start, end)) in positions.iter().enumerate() {
            for (other_index, (other_start, other_end)) in
                positions.iter().enumerate().skip(index + 1)
            {
                assert!(
                    end.x <= other_start.x
                        || other_end.x <= start.x
                        || end.y <= other_start.y
                        || other_end.y <= start.y,
                    "controls {index} and {other_index} overlap at {dpi} DPI"
                );
            }
        }
    }
}

unsafe fn verify_text_layout(app: &App, dpi: u32, dark: bool, fixture: Fixture) {
    unsafe {
        // Measure with the same fractional physical font size used for rendering.
        // Allow only one physical pixel for integer native window bounds.
        let scale = dpi as f32 / 96.0;
        let tolerance = 1.0;
        for (index, hwnd) in app.labels.iter().enumerate() {
            let accessible_text = label_text(*hwnd);
            let text = ui_style::label_caption(index, &accessible_text);
            let (size, weight) = FONT_SPECS[LABEL_SPECS[index].4];
            let extent = ui_render::measure_text(text, size as f32 * scale, weight as u16)
                .expect("DirectWrite text measurement must be available");
            let mut rect = RECT::default();
            GetClientRect(*hwnd, &mut rect).unwrap();
            let width = rect.right as f32;
            let height = rect.bottom as f32;
            assert!(
                extent.height <= height + tolerance,
                "label {index} vertically clipped at {dpi} DPI (dark={dark}, {fixture:?}): '{text}' height {} vs {height}",
                extent.height
            );
            let external_metadata = matches!(index, 0 | 1 | 4 | 7 | 10 | 12 | 13);
            assert!(
                extent.width <= width + tolerance || external_metadata,
                "label {index} horizontally clipped at {dpi} DPI (dark={dark}, {fixture:?}): '{text}' width {} vs {width}",
                extent.width
            );
        }
        for hwnd in &app.buttons {
            let text = label_text(*hwnd);
            let extent = ui_render::measure_text(&text, 14.0 * scale, 400)
                .expect("DirectWrite button text measurement must be available");
            let mut rect = RECT::default();
            GetClientRect(*hwnd, &mut rect).unwrap();
            assert!(
                extent.width + 16.0 * scale <= rect.right as f32 + tolerance
                    && extent.height <= rect.bottom as f32 + tolerance,
                "button text does not fit at {dpi} DPI: '{text}'"
            );
        }
    }
}

unsafe fn verify_control_semantics(app: &App) {
    unsafe {
        for (index, label) in app.labels.iter().enumerate() {
            let style = GetWindowLongPtrW(*label, GWL_STYLE) as u32;
            assert_eq!(
                style & 0x1f,
                13,
                "label {index} must use DirectWrite owner drawing"
            );
            assert!(
                !label_text(*label).is_empty(),
                "label {index} is missing its native accessible name"
            );
        }
        for (button, expected_id) in app.buttons.iter().zip([REFRESH, SETTINGS]) {
            assert_eq!(GetDlgCtrlID(*button), expected_id as i32);
            let style = GetWindowLongPtrW(*button, GWL_STYLE) as u32;
            assert_ne!(
                style & WS_TABSTOP.0,
                0,
                "button must remain keyboard reachable"
            );
            assert_eq!(style & 0xf, BS_OWNERDRAW as u32);
            assert!(
                !label_text(*button).is_empty(),
                "native accessible button name is missing"
            );
        }
    }
}

unsafe fn label_text(hwnd: HWND) -> String {
    unsafe {
        let count = GetWindowTextLengthW(hwnd);
        let mut text = vec![0u16; count as usize + 1];
        let length = GetWindowTextW(hwnd, &mut text);
        String::from_utf16_lossy(&text[..length as usize])
    }
}

unsafe fn destroy_test_panel(hwnd: HWND, host: &Host) {
    unsafe {
        {
            let mut borrowed = host.app.borrow_mut();
            let app = &mut *borrowed;
            let _ = app.worker.tx.send(Command::Stop);
            for child in app.labels.drain(..).chain(app.buttons.drain(..)) {
                let _ = DestroyWindow(child);
            }
            for font in app.fonts.drain(..) {
                let _ = DeleteObject(font.into());
            }
            let _ = DeleteObject(app.brush.into());
            let _ = DeleteObject(app.surface_brush.into());
            if !app.icon.is_invalid() {
                let _ = DestroyIcon(app.icon);
            }
        }
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let _ = DestroyWindow(hwnd);
    }
}

unsafe fn capture_test_panel(hwnd: HWND, host: &Host, dark: bool, fixture: Fixture) {
    unsafe {
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect).unwrap();
        let width = rect.right;
        let height = rect.bottom;
        let dc = CreateCompatibleDC(None);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        let previous = SelectObject(dc, bitmap.into());
        {
            let app = host.app.borrow();
            FillRect(dc, &rect, app.brush);
        }
        SendMessageW(
            hwnd,
            WM_PRINT,
            Some(WPARAM(dc.0 as usize)),
            Some(LPARAM(
                (PRF_CLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
            )),
        );
        let _ = GdiFlush();
        let data = std::slice::from_raw_parts(bits as *const u8, (width * height * 4) as usize);
        let app = host.app.borrow();
        verify_allowance_bar(data, width, &app);
        if app.glass {
            verify_glass_alpha(data, width, app.panel_dpi, dark);
        } else {
            verify_smoothed_card_corner(data, width, app.panel_dpi, dark);
        }
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&(54 + data.len() as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&width.to_le_bytes());
        bmp.extend_from_slice(&(-height).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bmp.extend_from_slice(&[0; 16]);
        bmp.extend_from_slice(data);
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".verification");
        std::fs::create_dir_all(&directory).unwrap();
        let theme = format!(
            "{}{}",
            if dark { "dark" } else { "light" },
            if app.glass { "-glass" } else { "" }
        );
        std::fs::write(
            directory.join(if host.app.borrow().panel_dpi == 96 {
                format!("native-panel-{theme}-{}.bmp", fixture.name())
            } else {
                format!(
                    "native-panel-{theme}-{}-{}pct.bmp",
                    fixture.name(),
                    host.app.borrow().panel_dpi * 100 / 96
                )
            }),
            bmp,
        )
        .unwrap();
        SelectObject(dc, previous);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
    }
}

fn verify_allowance_bar(pixels: &[u8], width: i32, app: &App) {
    let colors = app.palette();
    let reading = app
        .selected()
        .and_then(|(_, window)| window.remaining_percent());
    for offset in [16, 136, 256] {
        let x = app.px(24 + offset);
        let y = app.px(80);
        let pixel = &pixels[((y * width + x) * 4) as usize..][..4];
        let filled = reading.is_some() && offset as f32 / 272.0 < app.meter_value / 100.0;
        let expected = if !filled {
            colors.track
        } else if app.stale() {
            colors.muted
        } else {
            colors.accent
        };
        assert_eq!(
            pixel,
            &[
                ((expected.0 >> 16) & 255) as u8,
                ((expected.0 >> 8) & 255) as u8,
                (expected.0 & 255) as u8,
                255,
            ],
            "the horizontal bar must show the remaining amount from left to right"
        );
    }
}
fn verify_glass_alpha(pixels: &[u8], width: i32, dpi: u32, dark: bool) {
    let pixel = |x: i32, y: i32| {
        let (x, y) = (x * dpi as i32 / 96, y * dpi as i32 / 96);
        let offset = ((y * width + x) * 4) as usize;
        &pixels[offset..offset + 4]
    };
    let root = pixel(10, 10);
    assert_eq!(
        root[3], 170,
        "the theme veil must retain one-third of the OS backdrop"
    );
    assert_eq!(
        pixel(150, 199),
        root,
        "empty label pixels must match the root tint"
    );
    assert_eq!(
        pixel(12, 250),
        root,
        "empty button corners must match the root tint"
    );
    let card = pixel(20, 50);
    assert!(
        (218..=221).contains(&card[3]),
        "card alpha must include the shared base: {card:?}"
    );
    assert_eq!(
        pixel(290, 20),
        card,
        "child label backgrounds must match the parent card"
    );
    assert_eq!(
        pixel(18, 258)[3],
        255,
        "the primary button must remain opaque"
    );
    for pixel in pixels.chunks_exact(4) {
        assert!(
            pixel[..3].iter().all(|c| *c <= pixel[3]),
            "color channels must remain premultiplied for DWM"
        );
    }
    if dark {
        // Regression reference: the user's captured DWM root was #545454;
        // the nearby taskbar's median was #1C1D1E. This is an offline
        // compositing check, not a claim that Explorer exposes its brush.
        let taskbar_bgr = [30i32, 29, 28];
        for channel in 0..3 {
            let composed = root[channel] as i32 + 84 * (255 - root[3] as i32) / 255;
            assert!(
                (composed - taskbar_bgr[channel]).abs() <= 2,
                "the dark flyout must stay within two levels of the taskbar reference"
            );
        }
    }
}

fn verify_smoothed_card_corner(pixels: &[u8], width: i32, dpi: u32, dark: bool) {
    let colors = ui_style::palette(dark);
    let solid_colors = [colors.bg.0, colors.surface.0, colors.border.0];
    let scale = |value: i32| value * dpi as i32 / 96;
    let mut blended_pixels = 0;
    // This part of the hero's rounded corner contains no native child controls.
    // Aliased GDI geometry can produce only the three solid palette colors;
    // the Direct2D edge must contain intermediate coverage values.
    for y in scale(10)..scale(18) {
        for x in scale(12)..scale(20) {
            let offset = ((y * width + x) * 4) as usize;
            let color = pixels[offset + 2] as u32
                | ((pixels[offset + 1] as u32) << 8)
                | ((pixels[offset] as u32) << 16);
            if !solid_colors.contains(&color) {
                blended_pixels += 1;
            }
        }
    }
    assert!(
        blended_pixels >= 4,
        "the rounded panel edge was not antialiased at {dpi} DPI (dark={dark})"
    );
}

#[test]
fn reset_dates_and_auth_urls_reject_invalid_input() {
    assert_eq!(local_date(u64::MAX), "Reset date unavailable");
    assert!(!open_auth("https://example.com/signin"));
    assert!(!open_auth("https://auth.openai.com.evil.example/signin"));
    assert!(!open_auth("file:///C:/Windows/System32/cmd.exe"));
}

#[test]
fn nested_callbacks_are_deferred_without_borrow_panics() {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap();
        let name = w!("TokWatch.Native.ReentrancyTest");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: name,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            name,
            w!("TokWatch callback test"),
            WS_POPUP,
            0,
            0,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .unwrap();
        let host = new_host(hwnd, Settings::default(), Some(demo_snapshot()), true, None);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&*host as *const Host) as isize);
        {
            let mut app = host.app.borrow_mut();
            app.create_controls();
            for _ in 0..1000 {
                SendMessageW(hwnd, WM_TIMER, Some(WPARAM(TICK)), Some(LPARAM(0)));
                SendMessageW(hwnd, RESULT, Some(WPARAM(0)), Some(LPARAM(0)));
            }
            assert!(
                host.deferred.borrow().len() <= 64,
                "nested timer queue grew without bounds"
            );
        }
        destroy_test_panel(hwnd, &host);
        let _ = UnregisterClassW(name, Some(instance.into()));
    }
}

#[test]
fn flyout_geometry_stays_inside_scaled_and_offset_work_areas() {
    let cases = [
        (
            "1080p at 200 percent",
            192,
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1032,
            },
            PanelAnchor::Tray(RECT {
                left: 1820,
                top: 1040,
                right: 1844,
                bottom: 1072,
            }),
        ),
        (
            "monitor with negative origin",
            192,
            RECT {
                left: -1920,
                top: -1080,
                right: 0,
                bottom: -48,
            },
            PanelAnchor::Tray(RECT {
                left: -80,
                top: -40,
                right: -56,
                bottom: -8,
            }),
        ),
        (
            "top-edge tray",
            144,
            RECT {
                left: 0,
                top: 48,
                right: 1920,
                bottom: 1080,
            },
            PanelAnchor::Tray(RECT {
                left: 4,
                top: 8,
                right: 28,
                bottom: 40,
            }),
        ),
        (
            "bottom-edge tray at native scale",
            96,
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1032,
            },
            PanelAnchor::Tray(RECT {
                left: 1900,
                top: 1040,
                right: 1924,
                bottom: 1072,
            }),
        ),
        (
            "small work area",
            288,
            RECT {
                left: 0,
                top: 0,
                right: 640,
                bottom: 480,
            },
            PanelAnchor::Tray(RECT {
                left: 610,
                top: 470,
                right: 634,
                bottom: 494,
            }),
        ),
        (
            "DPI suggested rectangle outside display",
            192,
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1032,
            },
            PanelAnchor::Window(RECT {
                left: 1800,
                top: 900,
                right: 2640,
                bottom: 2020,
            }),
        ),
    ];
    for (name, monitor_dpi, work, anchor) in cases {
        let (panel_dpi, bounds) = panel_geometry(monitor_dpi, work, anchor);
        assert!(
            panel_dpi > 0 && panel_dpi <= monitor_dpi,
            "invalid scale: {name}"
        );
        assert!(
            bounds.left >= work.left + 8
                && bounds.top >= work.top + 8
                && bounds.right <= work.right - 8
                && bounds.bottom <= work.bottom - 8,
            "flyout is not fully accessible: {name}, {bounds:?} in {work:?}"
        );
        assert_eq!(
            bounds.right - bounds.left,
            PANEL_WIDTH * panel_dpi as i32 / 96
        );
        assert_eq!(
            bounds.bottom - bounds.top,
            PANEL_HEIGHT * panel_dpi as i32 / 96
        );
        if monitor_dpi == 96 {
            assert_eq!(
                panel_dpi, 96,
                "ordinary displays should retain requested scale"
            );
        }
        if name == "top-edge tray" {
            assert!(bounds.top >= 48, "top tray should open into the work area");
        }
    }
}

#[test]
fn tray_opens_only_on_selection_and_switches_provider_without_leaking_usage() {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap();
        let name = w!("TokWatch.Native.ClickProviderTest");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: name,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            name,
            w!("TokWatch click test"),
            WS_POPUP,
            0,
            0,
            PANEL_WIDTH,
            PANEL_HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .unwrap();
        let host = new_host(hwnd, Settings::default(), Some(demo_snapshot()), true, None);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, (&*host as *const Host) as isize);
        // Selection semantics are also required when Windows disables animation.
        host.app.borrow_mut().appearance.animations = false;
        host.app.borrow_mut().create_controls();
        SendMessageW(
            hwnd,
            TRAY,
            Some(WPARAM(0)),
            Some(LPARAM(NIN_POPUPOPEN as isize)),
        );
        assert!(!host.app.borrow().visible, "hover must not open the panel");
        SendMessageW(
            hwnd,
            TRAY,
            Some(WPARAM(0)),
            Some(LPARAM(NIN_SELECT as isize)),
        );
        assert!(host.app.borrow().visible, "click opens the panel");
        SendMessageW(
            hwnd,
            TRAY,
            Some(WPARAM(0)),
            Some(LPARAM(NIN_POPUPCLOSE as isize)),
        );
        assert!(
            host.app.borrow().visible,
            "ending hover must not dismiss a clicked panel"
        );
        SendMessageW(
            hwnd,
            TRAY,
            Some(WPARAM(0)),
            Some(LPARAM(NIN_SELECT as isize)),
        );
        assert!(!host.app.borrow().visible, "second click closes the panel");
        SendMessageW(
            hwnd,
            TRAY,
            Some(WPARAM(0)),
            Some(LPARAM(NIN_KEYSELECT as isize)),
        );
        assert!(
            host.app.borrow().visible,
            "keyboard selection opens the panel"
        );
        {
            let mut app = host.app.borrow_mut();
            app.switch_provider(Provider::Claude);
            app.render();
            assert_eq!(app.settings.provider, Provider::Claude);
            assert_eq!(app.selected().unwrap().0.id, "claude");
            assert_eq!(label_text(app.labels[2]), "72%");
            assert!(app.snapshot.as_ref().unwrap().reset_credits.is_none());
            app.switch_provider(Provider::Codex);
            assert_eq!(app.selected().unwrap().0.id, "codex");
            assert!(app.update_rx.is_none(), "demo must not check GitHub");
            verify_motion_cleanup(&mut app);
            verify_native_backdrop(&app);
        }
        destroy_test_panel(hwnd, &host);
        let _ = UnregisterClassW(name, Some(instance.into()));
    }
}

// Exercise the production transition paths on this test's own native window.
unsafe fn verify_motion_cleanup(app: &mut App) {
    unsafe {
        app.appearance.animations = true;
        app.show(false);
        assert!(app.panel_motion.is_some());
        app.hide();
        assert!(
            app.visible,
            "closing retains the window until the transition finishes"
        );
        assert_eq!(app.panel_motion.unwrap().target(), 0.0);
        app.show(false);
        assert_eq!(
            app.panel_motion.unwrap().target(),
            1.0,
            "reopening reverses a close"
        );
        app.appearance.animations = false;
        app.finish_motion();
        assert!(app.visible);
        assert_eq!(app.panel_value, 1.0);
        assert!(app.panel_motion.is_none());
        let mut rect = RECT::default();
        GetWindowRect(app.hwnd, &mut rect).unwrap();
        assert_eq!(
            rect.top, app.panel_bounds.top,
            "reduced motion snaps to the resting position"
        );
        assert_eq!(rect.left, app.panel_bounds.left);
        app.appearance.animations = true;
        app.hide();
        app.panel_motion = Some(ui_motion::Tween::new(
            1.0,
            0.0,
            Instant::now() - Duration::from_secs(1),
            100,
        ));
        app.animate();
        assert!(!app.visible);
        assert!(!IsWindowVisible(app.hwnd).as_bool());
        assert!(app.panel_motion.is_none() && app.meter_motion.is_none());
        assert!(app.hover_motion.iter().all(Option::is_none));
    }
}

unsafe fn verify_native_backdrop(app: &App) {
    unsafe {
        let mut appearance = app.appearance;
        appearance.high_contrast = false;
        appearance.transparency = true;
        let applied = apply_window_style(app.hwnd, app.dark, appearance);
        let mut material = DWMSBT_AUTO;
        // Older Windows 11 builds have no public system backdrop attribute.
        if DwmGetWindowAttribute(
            app.hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            (&mut material as *mut DWM_SYSTEMBACKDROP_TYPE).cast(),
            std::mem::size_of_val(&material) as u32,
        )
        .is_ok()
        {
            assert!(
                applied,
                "supported systems must accept the Acrylic backdrop"
            );
            assert_eq!(material, DWMSBT_TRANSIENTWINDOW);
            appearance.transparency = false;
            assert!(!apply_window_style(app.hwnd, app.dark, appearance));
            DwmGetWindowAttribute(
                app.hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                (&mut material as *mut DWM_SYSTEMBACKDROP_TYPE).cast(),
                std::mem::size_of_val(&material) as u32,
            )
            .unwrap();
            assert_eq!(
                material, DWMSBT_NONE,
                "disabling transparency removes the backdrop"
            );
        }
    }
}
