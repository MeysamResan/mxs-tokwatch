use super::*;
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_NOFOCUSRECT, ODS_SELECTED,
};

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub(super) bg: COLORREF,
    pub(super) backdrop_tint: COLORREF,
    pub(super) surface: COLORREF,
    pub(super) border: COLORREF,
    pub(super) text: COLORREF,
    pub(super) muted: COLORREF,
    pub(super) accent: COLORREF,
    pub(super) accent_bg: COLORREF,
    pub(super) accent_text: COLORREF,
    pub(super) track: COLORREF,
    pub(super) warning: COLORREF,
    pub(super) danger: COLORREF,
    pub(super) button_hover: COLORREF,
}

pub(super) fn palette(dark: bool) -> Palette {
    palette_for(dark, ui_theme::Appearance::read().accent, false)
}

pub(super) fn palette_for(dark: bool, accent: COLORREF, high_contrast: bool) -> Palette {
    if high_contrast {
        unsafe {
            return Palette {
                bg: COLORREF(GetSysColor(COLOR_WINDOW)),
                backdrop_tint: COLORREF(GetSysColor(COLOR_WINDOW)),
                surface: COLORREF(GetSysColor(COLOR_WINDOW)),
                border: COLORREF(GetSysColor(COLOR_WINDOWTEXT)),
                text: COLORREF(GetSysColor(COLOR_WINDOWTEXT)),
                muted: COLORREF(GetSysColor(COLOR_GRAYTEXT)),
                accent: COLORREF(GetSysColor(COLOR_HIGHLIGHT)),
                accent_bg: COLORREF(GetSysColor(COLOR_HIGHLIGHT)),
                accent_text: COLORREF(GetSysColor(COLOR_HIGHLIGHTTEXT)),
                track: COLORREF(GetSysColor(COLOR_GRAYTEXT)),
                warning: COLORREF(GetSysColor(COLOR_WINDOWTEXT)),
                danger: COLORREF(GetSysColor(COLOR_WINDOWTEXT)),
                button_hover: COLORREF(GetSysColor(COLOR_BTNFACE)),
            };
        }
    }
    Palette {
        bg: if dark {
            color(28, 28, 28)
        } else {
            color(243, 243, 243)
        },
        backdrop_tint: if dark {
            color(0, 0, 0)
        } else {
            color(243, 243, 243)
        },
        surface: if dark {
            color(48, 48, 48)
        } else {
            color(255, 255, 255)
        },
        border: if dark {
            color(68, 68, 68)
        } else {
            color(218, 218, 218)
        },
        text: if dark {
            color(255, 255, 255)
        } else {
            color(26, 26, 26)
        },
        muted: if dark {
            color(197, 197, 197)
        } else {
            color(96, 96, 96)
        },
        accent,
        accent_bg: accent,
        accent_text: accent_foreground(accent),
        track: if dark {
            color(72, 72, 72)
        } else {
            color(222, 222, 222)
        },
        warning: if dark {
            color(252, 225, 0)
        } else {
            color(157, 93, 0)
        },
        danger: if dark {
            color(255, 153, 164)
        } else {
            color(196, 43, 28)
        },
        button_hover: if dark {
            color(62, 62, 62)
        } else {
            color(248, 248, 248)
        },
    }
}

fn luminance(value: COLORREF) -> f64 {
    let channel = |shift: u32| {
        let c = ((value.0 >> shift) & 255) as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    channel(0) * 0.2126 + channel(8) * 0.7152 + channel(16) * 0.0722
}
fn accent_foreground(accent: COLORREF) -> COLORREF {
    let l = luminance(accent);
    if (l + 0.05) / 0.05 >= 1.05 / (l + 0.05) {
        color(0, 0, 0)
    } else {
        color(255, 255, 255)
    }
}

fn mix(a: COLORREF, b: COLORREF, amount: u32) -> COLORREF {
    let channel = |shift: u32| {
        ((((a.0 >> shift) & 255) * (100 - amount) + ((b.0 >> shift) & 255) * amount) / 100) as u8
    };
    color(channel(0), channel(8), channel(16))
}

// DWM's transient backdrop is explicitly the brightest Acrylic variant, not
// Explorer's taskbar brush. A theme veil reduces that lifted background while
// preserving the OS blur. The final color still depends on content behind it.
const BACKDROP_TINT_OPACITY: f32 = 2.0 / 3.0;

fn paint_background(
    canvas: &mut ui_render::Canvas<'_>,
    bounds: RECT,
    colors: Palette,
    glass: bool,
) {
    canvas.opacity(if glass { BACKDROP_TINT_OPACITY } else { 1.0 });
    canvas.fill(
        bounds,
        if glass {
            colors.backdrop_tint
        } else {
            colors.bg
        },
    );
    canvas.opacity(1.0);
}

pub(super) unsafe fn paint_panel(app: &App, dc: HDC, region: &RECT) {
    unsafe {
        let colors = app.palette();
        let mut client = RECT::default();
        let _ = GetClientRect(app.hwnd, &mut client);
        let px = |value| app.px(value);
        let rect = |x, y, w, h| RECT {
            left: px(x),
            top: px(y),
            right: px(x + w),
            bottom: px(y + h),
        };
        let scale = app.panel_dpi as f32 / 96.0;
        if !ui_render::paint(dc, client, |canvas| {
            paint_background(canvas, client, colors, app.glass);
            canvas.opacity(if app.glass { 0.58 } else { 1.0 });
            for (r, radius) in [
                (rect(12, 10, 296, 100), 8.0),
                (rect(12, 118, 142, 72), 8.0),
                (rect(166, 118, 142, 72), 8.0),
            ] {
                canvas.rounded(r, radius * scale, colors.surface, Some(colors.border));
            }
            canvas.opacity(1.0);
            canvas.progress_bar(
                rect(24, 77, 272, 6),
                app.selected()
                    .and_then(|(_, w)| w.remaining_percent())
                    .map(|_| app.meter_value),
                colors.track,
                if app.stale() {
                    colors.muted
                } else {
                    colors.accent
                },
            );
            canvas.line(
                (16.0 * scale, 221.0 * scale),
                (304.0 * scale, 221.0 * scale),
                1.0,
                colors.border,
            );
            let status = if app.error.is_some() {
                colors.danger
            } else if app.login_pending {
                colors.warning
            } else if app.stale() {
                colors.muted
            } else {
                colors.accent
            };
            canvas.ellipse(rect(16, 233, 5, 5), status, None);
        }) {
            FillRect(dc, region, app.brush);
        }
    }
}

pub(super) fn label_caption(index: usize, text: &str) -> &str {
    if index == 2 && text == "Usage unavailable" {
        // Keep the full native accessible name while using a compact visual label.
        "—"
    } else {
        text
    }
}

// Keep native controls and their accessible names; only their pixels use DirectWrite.
pub(super) unsafe fn paint_label(
    theme: PaintTheme,
    item: &DRAWITEMSTRUCT,
    index: usize,
    text: &str,
) {
    unsafe {
        let text = label_caption(index, text);
        let colors = theme.colors;
        let dpi = theme.dpi;
        let on_card = (0..=10).contains(&index);
        let background = if on_card { colors.surface } else { colors.bg };
        let ink = if matches!(index, 0 | 2 | 6 | 9 | 12) {
            colors.text
        } else {
            colors.muted
        };
        let (size, weight) = FONT_SPECS[LABEL_SPECS[index].4];
        let align = if matches!(index, 1 | 12) {
            ui_render::Align::Right
        } else {
            ui_render::Align::Left
        };
        if !ui_render::paint(item.hDC, item.rcItem, |canvas| {
            // Child DCs replace their pixels rather than blend over the parent.
            // Recreate the same base before the card so no bright rectangles show.
            paint_background(canvas, item.rcItem, colors, theme.glass);
            if on_card {
                canvas.opacity(if theme.glass { 0.58 } else { 1.0 });
                canvas.fill(item.rcItem, colors.surface);
                canvas.opacity(1.0);
            }
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
            let align = if matches!(index, 1 | 12) {
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
    theme: PaintTheme,
    item: &DRAWITEMSTRUCT,
    text: &str,
    primary: bool,
    hover: f32,
) {
    unsafe {
        let colors = theme.colors;
        let dpi = theme.dpi;
        let font = theme.button_font;
        let scale = dpi as f32 / 96.0;
        let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
        let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
        let focused =
            item.itemState.0 & ODS_FOCUS.0 != 0 && item.itemState.0 & ODS_NOFOCUSRECT.0 == 0;
        let (fill, border, foreground) = if disabled {
            (colors.surface, colors.border, colors.muted)
        } else if primary {
            let base = colors.accent_bg;
            let fill = if pressed {
                mix(base, color(0, 0, 0), 13)
            } else {
                mix(base, colors.accent_text, (hover * 10.0).round() as u32)
            };
            (
                fill,
                mix(fill, color(255, 255, 255), 15),
                colors.accent_text,
            )
        } else {
            let fill = if pressed {
                colors.track
            } else {
                mix(
                    colors.surface,
                    colors.button_hover,
                    (hover * 100.0).round() as u32,
                )
            };
            (fill, colors.border, colors.text)
        };
        if !ui_render::paint(item.hDC, item.rcItem, |canvas| {
            paint_background(canvas, item.rcItem, colors, theme.glass);
            canvas.rounded(item.rcItem, 4.0 * scale, fill, Some(border));
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
                    2.0 * scale,
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
                    size: 14.0 * scale,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_accent_keeps_its_exact_rgb_and_gets_legible_button_text() {
        for r in (0..=255).step_by(17) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(17) {
                    let accent = color(r, g, b);
                    for dark in [false, true] {
                        let p = palette_for(dark, accent, false);
                        assert_eq!(p.accent, accent);
                        assert_eq!(p.accent_bg, accent);
                        let a = luminance(accent);
                        let f = luminance(p.accent_text);
                        assert!((a.max(f) + 0.05) / (a.min(f) + 0.05) >= 4.5);
                    }
                }
            }
        }
    }
}
