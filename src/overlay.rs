//! Белая мягкая надпись с FPS поверх игры.
//!
//! Это отдельное прозрачное окно без рамки: оно поверх всех окон, не берёт фокус и пропускает клики сквозь себя,
//! поэтому игре не мешает. Рисуется прямо через Win32 (без графических библиотек), потому что надпись
//! живёт в игровом режиме, где важна минимальная память: несколько МБ вместо сотен.
//!
//! Работает над играми в оконном режиме и «окне без рамки». В эксклюзивном полноэкранном режиме Windows не даёт
//! обычным окнам рисоваться поверх игры.

use crate::config::{Config, Corner};

const MARGIN: i32 = 24;
#[allow(dead_code)]
const PAD: i32 = 8;

/// Верхний левый угол окна надписи для выбранного угла экрана.
pub fn corner_pos(corner: Corner, screen_w: i32, screen_h: i32, w: i32, h: i32) -> (i32, i32) {
    let x = match corner {
        Corner::TopLeft | Corner::BottomLeft => MARGIN,
        Corner::TopRight | Corner::BottomRight => screen_w - w - MARGIN,
    };
    let y = match corner {
        Corner::TopLeft | Corner::TopRight => MARGIN,
        Corner::BottomLeft | Corner::BottomRight => screen_h - h - MARGIN,
    };
    (x, y)
}

/// Превращает маску (яркость белого текста на чёрном, 0..255 на пиксель) в готовые пиксели BGRA с
/// предумноженной прозрачностью: белый текст со сглаженными краями и мягкой тенью-размытием.
/// `opacity` — 0.0..1.0. Тень смещена на 1 пиксель вниз-вправо, размыта и слабее текста: надпись читается
/// на светлом фоне, но не выглядит «рубленой».
pub fn compose(mask: &[u8], w: usize, h: usize, opacity: f32) -> Vec<u32> {
    let get = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            0.0
        } else {
            mask[y as usize * w + x as usize] as f32 / 255.0
        }
    };
    let mut out = vec![0u32; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let text = get(x, y) * opacity;
            // тень: среднее 3x3 вокруг точки, сдвинутой на (-1,-1) → тень уходит вниз-вправо
            let mut s = 0.0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    s += get(x - 1 + dx, y - 1 + dy);
                }
            }
            let shadow = (s / 9.0) * 0.55 * opacity;
            let alpha = text + shadow * (1.0 - text);
            // цвет текста белый, тени чёрный: предумноженный цвет = text * 255
            let c = (text * 255.0).round().clamp(0.0, 255.0) as u32;
            let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u32;
            out[y as usize * w + x as usize] = (a << 24) | (c << 16) | (c << 8) | c;
        }
    }
    out
}

pub struct Overlay {
    #[cfg(any(windows, gl_check))]
    text: std::sync::Arc<std::sync::Mutex<String>>,
}

impl Overlay {
    /// Запускает окно надписи в отдельном потоке.
    pub fn spawn(cfg: Config) -> Overlay {
        #[cfg(any(windows, gl_check))]
        {
            let text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let t2 = text.clone();
            let _ = std::thread::Builder::new().name("overlay".into()).spawn(move || win::run(cfg, t2));
            Overlay { text }
        }
        #[cfg(not(any(windows, gl_check)))]
        {
            let _ = cfg;
            Overlay {}
        }
    }

    /// Пустая строка скрывает надпись.
    pub fn set_text(&self, s: &str) {
        #[cfg(any(windows, gl_check))]
        if let Ok(mut t) = self.text.lock() {
            if *t != s {
                *t = s.to_string();
            }
        }
        #[cfg(not(any(windows, gl_check)))]
        let _ = s;
    }
}

#[cfg(any(windows, gl_check))]
mod win {
    use super::{compose, corner_pos, PAD};
    use crate::config::Config;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn run(cfg: Config, text: Arc<Mutex<String>>) {
        unsafe {
            SetProcessDPIAware();
            let hinst = GetModuleHandleW(std::ptr::null());
            let class = wide("GameLensOverlay");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.hInstance = hinst;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            let ex = WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW;
            let hwnd = CreateWindowExW(ex, class.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 10, 10, std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null());
            if hwnd.is_null() {
                return;
            }
            let mut shown = String::new();
            let mut visible = false;
            loop {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                let want = text.lock().map(|t| t.clone()).unwrap_or_default();
                if want.is_empty() {
                    if visible {
                        ShowWindow(hwnd, SW_HIDE);
                        visible = false;
                        shown.clear();
                    }
                } else {
                    if want != shown || !visible {
                        render(hwnd, &want, &cfg);
                        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                        visible = true;
                        shown = want;
                    }
                    // игра может перехватить верхний слой: поднимаем надпись обратно
                    SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }

    unsafe fn render(hwnd: HWND, text: &str, cfg: &Config) {
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            let mem = CreateCompatibleDC(screen);
            let face = wide("Segoe UI");
            let font = CreateFontW(-(cfg.size as i32), 0, 0, 0, 600, 0, 0, 0, 1, 0, 0, ANTIALIASED_QUALITY as u32, 0, face.as_ptr());
            let old_font = SelectObject(mem, font);
            let t = wide(text);
            let n = (t.len() - 1) as i32;
            let mut ext: SIZE = std::mem::zeroed();
            GetTextExtentPoint32W(mem, t.as_ptr(), n, &mut ext);
            let w = ext.cx + PAD * 2;
            let h = ext.cy + PAD * 2;

            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // сверху вниз
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                SelectObject(mem, old_font);
                DeleteObject(font);
                DeleteDC(mem);
                ReleaseDC(std::ptr::null_mut(), screen);
                return;
            }
            let old_bmp = SelectObject(mem, bmp);
            let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
            px.fill(0);
            SetBkMode(mem, TRANSPARENT as i32);
            SetTextColor(mem, 0x00FF_FFFF);
            TextOutW(mem, PAD, PAD, t.as_ptr(), n);

            // яркость каждого пикселя = покрытие текста (белое на чёрном)
            let mask: Vec<u8> = px.iter().map(|p| ((p >> 16) & 0xFF).max((p >> 8) & 0xFF).max(p & 0xFF) as u8).collect();
            let out = compose(&mask, w as usize, h as usize, cfg.opacity as f32 / 100.0);
            px.copy_from_slice(&out);

            let sw = GetSystemMetrics(SM_CXSCREEN);
            let sh = GetSystemMetrics(SM_CYSCREEN);
            let (x, y) = corner_pos(cfg.corner, sw, sh, w, h);
            let dst = POINT { x, y };
            let src = POINT { x: 0, y: 0 };
            let size = SIZE { cx: w, cy: h };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(hwnd, screen, &dst, &size, mem, &src, 0, &blend, ULW_ALPHA);

            SelectObject(mem, old_bmp);
            SelectObject(mem, old_font);
            DeleteObject(bmp);
            DeleteObject(font);
            DeleteDC(mem);
            ReleaseDC(std::ptr::null_mut(), screen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners() {
        assert_eq!(corner_pos(Corner::TopLeft, 1920, 1080, 100, 40), (24, 24));
        assert_eq!(corner_pos(Corner::TopRight, 1920, 1080, 100, 40), (1796, 24));
        assert_eq!(corner_pos(Corner::BottomLeft, 1920, 1080, 100, 40), (24, 1016));
        assert_eq!(corner_pos(Corner::BottomRight, 1920, 1080, 100, 40), (1796, 1016));
    }

    #[test]
    fn empty_mask_is_fully_transparent() {
        let out = compose(&vec![0u8; 20 * 10], 20, 10, 0.8);
        assert!(out.iter().all(|p| *p == 0));
    }

    #[test]
    fn white_text_is_premultiplied_and_respects_opacity() {
        let mut mask = vec![0u8; 9 * 9];
        mask[4 * 9 + 4] = 255; // одна белая точка в центре
        let out = compose(&mask, 9, 9, 0.5);
        let p = out[4 * 9 + 4];
        let (a, c) = ((p >> 24) & 0xFF, p & 0xFF);
        assert!((a as i32 - 128).abs() <= 3, "alpha {a}");
        assert!((c as i32 - 128).abs() <= 3, "color {c}");
        // предумноженный цвет не превышает альфу, иначе Windows покажет пересвет
        assert!(c <= a);
    }

    #[test]
    fn shadow_is_dimmer_than_text_and_offset() {
        let mut mask = vec![0u8; 11 * 11];
        mask[5 * 11 + 5] = 255;
        let out = compose(&mask, 11, 11, 1.0);
        let text_alpha = out[5 * 11 + 5] >> 24;
        let shadow_px = out[6 * 11 + 6]; // вниз-вправо
        let shadow_alpha = shadow_px >> 24;
        assert!(shadow_alpha > 0 && shadow_alpha < text_alpha);
        assert_eq!(shadow_px & 0xFF, 0, "тень чёрная");
        assert_eq!(out[3 * 11 + 3], 0, "далеко от текста пусто");
    }
}
