use crate::model::Theme;
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND},
        Graphics::{
            Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute},
            Gdi::*,
        },
        System::Registry::*,
    },
    core::w,
};
#[derive(Clone, Copy)]
pub struct Palette {
    pub background: COLORREF,
    pub surface: COLORREF,
    pub text: COLORREF,
    pub muted: COLORREF,
    pub accent: COLORREF,
    pub selected: COLORREF,
}
fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
    COLORREF(r | (g << 8) | (b << 16))
}
pub fn dark(theme: Theme) -> bool {
    match theme {
        Theme::Dark => true,
        Theme::Light => false,
        Theme::System => unsafe {
            let mut value = 1u32;
            let mut size = 4;
            let _ = RegGetValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
                w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut value as *mut u32).cast()),
                Some(&mut size),
            );
            value == 0
        },
    }
}
fn palette_for_dark(is_dark: bool) -> Palette {
    if is_dark {
        Palette {
            background: rgb(30, 32, 36),
            surface: rgb(39, 42, 47),
            text: rgb(238, 240, 244),
            muted: rgb(165, 173, 187),
            accent: rgb(88, 157, 245),
            selected: rgb(40, 58, 81),
        }
    } else {
        Palette {
            background: rgb(246, 247, 249),
            surface: rgb(255, 255, 255),
            text: rgb(30, 37, 48),
            muted: rgb(106, 117, 132),
            accent: rgb(43, 112, 222),
            selected: rgb(232, 241, 255),
        }
    }
}
/// Own cached colors and brushes; snapshots keep them alive through nested paint callbacks.
pub struct Colors {
    pub palette: Palette,
    pub dark: bool,
    pub background: HBRUSH,
    pub surface: HBRUSH,
    pub selected: HBRUSH,
    pub accent: HBRUSH,
}
impl Colors {
    pub fn new(theme: Theme) -> Self {
        let dark = dark(theme);
        let palette = palette_for_dark(dark);
        unsafe {
            Self {
                palette,
                dark,
                background: CreateSolidBrush(palette.background),
                surface: CreateSolidBrush(palette.surface),
                selected: CreateSolidBrush(palette.selected),
                accent: CreateSolidBrush(palette.accent),
            }
        }
    }
}
impl Drop for Colors {
    fn drop(&mut self) {
        unsafe {
            for brush in [self.background, self.surface, self.selected, self.accent] {
                if !brush.is_invalid() {
                    let _ = DeleteObject(brush.into());
                }
            }
        }
    }
}
pub unsafe fn apply(hwnd: HWND, dark: bool) {
    unsafe {
        let value = i32::from(dark);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&value as *const i32).cast(),
            4,
        );
    }
}
pub unsafe fn font(dpi: u32) -> HFONT {
    unsafe {
        CreateFontW(
            -((15 * dpi / 96) as i32),
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            w!("Segoe UI"),
        )
    }
}
