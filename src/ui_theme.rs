//! Read Windows appearance through public APIs; all notifications return to the UI thread.
use super::*;
use windows::{
    Foundation::TypedEventHandler,
    UI::ViewManagement::{UIColorType, UISettings},
    Win32::{
        System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize},
        UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
    },
};

pub(super) struct Runtime(bool);
impl Runtime {
    pub(super) fn new() -> Self {
        Self(unsafe { RoInitialize(RO_INIT_SINGLETHREADED).is_ok() })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                RoUninitialize();
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Appearance {
    pub accent: COLORREF,
    pub transparency: bool,
    pub animations: bool,
    pub high_contrast: bool,
}
impl Appearance {
    pub(super) fn read() -> Self {
        unsafe {
            let mut high = HIGHCONTRASTW {
                cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
                ..Default::default()
            };
            let _ = SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                high.cbSize,
                Some((&mut high as *mut HIGHCONTRASTW).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            let high_contrast = high.dwFlags.contains(HCF_HIGHCONTRASTON);
            let settings = UISettings::new().ok();
            // AccentColor is an ABGR DWORD, unlike DwmGetColorizationColor's ARGB.
            let accent = settings
                .as_ref()
                .and_then(|s| s.GetColorValue(UIColorType::Accent).ok())
                .map(|c| color(c.R, c.G, c.B))
                .unwrap_or_else(|| {
                    COLORREF(
                        registry_dword(w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor"))
                            .unwrap_or(GetSysColor(COLOR_HIGHLIGHT))
                            & 0xffffff,
                    )
                });
            let mut animate = BOOL(0);
            let _ = SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                Some((&mut animate as *mut BOOL).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
            let animations = settings
                .as_ref()
                .and_then(|s| s.AnimationsEnabled().ok())
                .unwrap_or(animate.as_bool());
            let transparency = settings
                .as_ref()
                .and_then(|s| s.AdvancedEffectsEnabled().ok())
                .unwrap_or_else(|| {
                    registry_dword(
                        w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                        w!("EnableTransparency"),
                    ) == Some(1)
                });
            Self {
                accent,
                transparency: transparency && !high_contrast,
                animations: animations && !high_contrast,
                high_contrast,
            }
        }
    }
}
fn registry_dword(path: PCWSTR, name: PCWSTR) -> Option<u32> {
    unsafe {
        let mut value = 0u32;
        let mut size = 4;
        RegGetValueW(
            HKEY_CURRENT_USER,
            path,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
        .ok()
        .ok()
        .map(|_| value)
    }
}

pub(super) struct Observer {
    settings: UISettings,
    colors: Option<i64>,
    effects: Option<i64>,
    animations: Option<i64>,
}
impl Observer {
    pub(super) fn new(hwnd: HWND) -> Option<Self> {
        let settings = UISettings::new().ok()?;
        let handle = hwnd.0 as isize;
        let changed = TypedEventHandler::new(move |_, _| {
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(handle as *mut _)),
                    APPEARANCE_CHANGED,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            Ok(())
        });
        let colors = settings.ColorValuesChanged(&changed).ok();
        let effects = settings.AdvancedEffectsEnabledChanged(&changed).ok();
        let animations = settings
            .AnimationsEnabledChanged(&TypedEventHandler::new(move |_, _| {
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(handle as *mut _)),
                        APPEARANCE_CHANGED,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
                Ok(())
            }))
            .ok();
        Some(Self {
            settings,
            colors,
            effects,
            animations,
        })
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        if let Some(token) = self.colors {
            let _ = self.settings.RemoveColorValuesChanged(token);
        }
        if let Some(token) = self.effects {
            let _ = self.settings.RemoveAdvancedEffectsEnabledChanged(token);
        }
        if let Some(token) = self.animations {
            let _ = self.settings.RemoveAnimationsEnabledChanged(token);
        }
    }
}
