use super::*;
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED,
};

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub(super) bg: COLORREF,
    pub(super) surface: COLORREF,
    pub(super) border: COLORREF,
    pub(super) text: COLORREF,
    pub(super) muted: COLORREF,
    pub(super) accent: COLORREF,
    pub(super) accent_bg: COLORREF,
    pub(super) track: COLORREF,
    pub(super) warning: COLORREF,
    pub(super) danger: COLORREF,
    pub(super) button_hover: COLORREF,
}

pub(super) fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: color(18, 21, 27),
            surface: color(27, 32, 40),
            border: color(44, 51, 63),
            text: color(241, 245, 249),
            muted: color(155, 168, 185),
            accent: color(111, 231, 194),
            accent_bg: color(31, 61, 54),
            track: color(48, 58, 70),
            warning: color(246, 193, 104),
            danger: color(255, 137, 148),
            button_hover: color(39, 47, 58),
        }
    } else {
        Palette {
            bg: color(245, 247, 249),
            surface: color(255, 255, 255),
            border: color(220, 227, 233),
            text: color(25, 36, 47),
            muted: color(98, 114, 129),
            accent: color(21, 129, 100),
            accent_bg: color(221, 245, 234),
            track: color(228, 235, 239),
            warning: color(153, 99, 16),
            danger: color(186, 55, 73),
            button_hover: color(233, 239, 243),
        }
    }
}

fn mix(a: COLORREF, b: COLORREF, amount: u32) -> COLORREF {
    let channel = |shift: u32| {
        ((((a.0 >> shift) & 255) * (100 - amount) + ((b.0 >> shift) & 255) * amount) / 100) as u8
    };
    color(channel(0), channel(8), channel(16))
}

pub(super) unsafe fn paint_panel(app: &App, dc: HDC, region: &RECT) {
    unsafe {
        let colors = palette(app.dark);
        let mut client = RECT::default();
        let _ = GetClientRect(app.hwnd, &mut client);
        let px = |value| app.px(value);
        let rect = |x, y, width, height| RECT {
            left: px(x),
            top: px(y),
            right: px(x + width),
            bottom: px(y + height),
        };
        let scale = app.panel_dpi as f32 / 96.0;
        if !ui_render::paint(dc, client, |canvas| {
            canvas.fill(client, colors.bg);
            canvas.rounded(rect(16, 15, 22, 22), 6.0 * scale, colors.accent_bg, None);
            for (x, y, height) in [(21, 27, 5), (27, 23, 9), (33, 19, 13)] {
                canvas.rounded(rect(x, y, 2, height), scale, colors.accent, None);
            }
            canvas.rounded(
                rect(244, 14, 60, 26),
                8.0 * scale,
                colors.surface,
                Some(colors.border),
            );
            canvas.rounded(
                rect(12, 50, 296, 98),
                12.0 * scale,
                colors.surface,
                Some(colors.border),
            );
            canvas.rounded(rect(24, 137, 272, 4), 2.0 * scale, colors.track, None);
            if let Some(percent) = app
                .selected()
                .and_then(|(_, window)| window.remaining_percent())
            {
                let ink = if app.stale() {
                    colors.muted
                } else if percent <= 20 {
                    colors.danger
                } else if percent <= 50 {
                    colors.warning
                } else {
                    colors.accent
                };
                if percent > 0 {
                    let mut meter = rect(24, 137, 272, 4);
                    meter.right =
                        meter.left + ((meter.right - meter.left) * percent as i32 / 100).max(1);
                    canvas.rounded(
                        meter,
                        (2.0 * scale).min((meter.right - meter.left) as f32 / 2.0),
                        ink,
                        None,
                    );
                }
            }
            canvas.line(
                (160.0 * scale, 162.0 * scale),
                (160.0 * scale, 220.0 * scale),
                1.0,
                colors.border,
            );
            canvas.line(
                (16.0 * scale, 252.0 * scale),
                (304.0 * scale, 252.0 * scale),
                1.0,
                colors.border,
            );
            let status = if app.error.is_some() {
                colors.danger
            } else if app.stale() || app.busy || app.login_pending {
                colors.warning
            } else {
                colors.accent
            };
            canvas.ellipse(rect(16, 262, 5, 5), status, None);
        }) {
            FillRect(dc, region, app.brush);
        }
    }
}

// Keep native controls and their accessible names; only their pixels use DirectWrite.
pub(super) unsafe fn paint_label(
    dpi: u32,
    dark: bool,
    item: &DRAWITEMSTRUCT,
    index: usize,
    text: &str,
) {
    unsafe {
        let colors = palette(dark);
        let background = if (1..=6).contains(&index) {
            colors.surface
        } else {
            colors.bg
        };
        let ink = if index == 2 {
            colors.accent
        } else if matches!(index, 0 | 1 | 4 | 8 | 11 | 14) {
            colors.text
        } else {
            colors.muted
        };
        let (size, weight) = FONT_SPECS[LABEL_SPECS[index].4];
        let align = if index == 2 {
            ui_render::Align::Center
        } else if matches!(index, 3 | 14) {
            ui_render::Align::Right
        } else {
            ui_render::Align::Left
        };
        if !ui_render::paint(item.hDC, item.rcItem, |canvas| {
            canvas.fill(item.rcItem, background);
            canvas.text(
                text,
                item.rcItem,
                ui_render::TextStyle {
                    size: size as f32 * dpi as f32 / 96.0,
                    weight: weight as u16,
                    color: ink,
                    align,
                    center_y: true,
                    ellipsis: true,
                },
            );
        }) {
            let font = HFONT(SendMessageW(item.hwndItem, WM_GETFONT, None, None).0 as *mut _);
            let align = if index == 2 {
                DT_CENTER
            } else if matches!(index, 3 | 14) {
                DT_RIGHT
            } else {
                DT_LEFT
            };
            fallback_text(
                item.hDC,
                item.rcItem,
                text,
                font,
                (ink, background),
                align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
            );
        }
    }
}

pub(super) unsafe fn paint_button(
    dpi: u32,
    dark: bool,
    font: HFONT,
    item: &DRAWITEMSTRUCT,
    text: &str,
    primary: bool,
    hovered: bool,
) {
    unsafe {
        let colors = palette(dark);
        let scale = dpi as f32 / 96.0;
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let focused =
            item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_NOFOCUSRECT.0 == 0;
        let (fill, border, foreground) = if disabled {
            (colors.surface, colors.border, colors.muted)
        } else if primary {
            let base = if dark {
                colors.accent
            } else {
                color(18, 112, 87)
            };
            let fill = if pressed {
                mix(base, color(0, 0, 0), 13)
            } else if hovered {
                mix(base, color(255, 255, 255), 12)
            } else {
                base
            };
            (
                fill,
                fill,
                if dark {
                    color(13, 45, 35)
                } else {
                    color(255, 255, 255)
                },
            )
        } else {
            let fill = if pressed {
                colors.track
            } else if hovered {
                colors.button_hover
            } else {
                colors.surface
            };
            (fill, colors.border, colors.text)
        };
        if !ui_render::paint(item.hDC, item.rcItem, |canvas| {
            canvas.fill(item.rcItem, colors.bg);
            canvas.rounded(item.rcItem, 8.0 * scale, fill, Some(border));
            if focused && !disabled {
                let inset = (3.0 * scale).round() as i32;
                let focus = RECT {
                    left: item.rcItem.left + inset,
                    top: item.rcItem.top + inset,
                    right: item.rcItem.right - inset,
                    bottom: item.rcItem.bottom - inset,
                };
                canvas.rounded(
                    focus,
                    5.0 * scale,
                    fill,
                    Some(if primary { foreground } else { colors.accent }),
                );
            }
            let mut bounds = item.rcItem;
            if pressed {
                bounds.top += scale.round() as i32;
            }
            canvas.text(
                text,
                bounds,
                ui_render::TextStyle {
                    size: 12.0 * scale,
                    weight: 400,
                    color: foreground,
                    align: ui_render::Align::Center,
                    center_y: true,
                    ellipsis: false,
                },
            );
        }) {
            fallback_text(
                item.hDC,
                item.rcItem,
                text,
                font,
                (foreground, fill),
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
    }
}

unsafe fn fallback_text(
    dc: HDC,
    mut bounds: RECT,
    text: &str,
    font: HFONT,
    colors: (COLORREF, COLORREF),
    flags: DRAW_TEXT_FORMAT,
) {
    unsafe {
        let brush = CreateSolidBrush(colors.1);
        FillRect(dc, &bounds, brush);
        let _ = DeleteObject(brush.into());
        let previous_font = SelectObject(dc, font.into());
        let previous_mode = SetBkMode(dc, TRANSPARENT);
        let previous_color = SetTextColor(dc, colors.0);
        let mut caption: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(dc, &mut caption, &mut bounds, flags);
        let _ = SetTextColor(dc, previous_color);
        let _ = SetBkMode(dc, BACKGROUND_MODE(previous_mode as u32));
        SelectObject(dc, previous_font);
    }
}
