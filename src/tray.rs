//! Значок в области уведомлений в игровом режиме: клик открывает окно GameLens, меню даёт «Остановить запись».
//! Живёт в процессе рекордера, ждёт сообщений без опроса, поэтому процессор не грузит.

pub struct Tray {
    #[cfg(any(windows, gl_check))]
    thread_id: std::sync::Arc<std::sync::atomic::AtomicU32>,
    #[cfg(any(windows, gl_check))]
    join: Option<std::thread::JoinHandle<()>>,
}

impl Tray {
    pub fn spawn() -> Tray {
        #[cfg(any(windows, gl_check))]
        {
            let thread_id = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let t2 = thread_id.clone();
            let join = std::thread::Builder::new().name("tray".into()).spawn(move || win::run(t2)).ok();
            Tray { thread_id, join }
        }
        #[cfg(not(any(windows, gl_check)))]
        Tray {}
    }
}

impl Drop for Tray {
    /// Убирает значок до выхода процесса, иначе он «призраком» остался бы в трее до наведения мыши.
    fn drop(&mut self) {
        #[cfg(any(windows, gl_check))]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
            for _ in 0..40 {
                let id = self.thread_id.load(std::sync::atomic::Ordering::Acquire);
                if id != 0 {
                    unsafe { PostThreadMessageW(id, WM_QUIT, 0, 0) };
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            if let Some(j) = self.join.take() {
                let _ = j.join();
            }
        }
    }
}

#[cfg(any(windows, gl_check))]
mod win {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Shell::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const WM_TRAY: u32 = WM_APP + 1;
    const ID_OPEN: usize = 1;
    const ID_STOP: usize = 2;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn nid(hwnd: HWND) -> NOTIFYICONDATAW {
        unsafe {
            let mut d: NOTIFYICONDATAW = std::mem::zeroed();
            d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            d.hWnd = hwnd;
            d.uID = 1;
            d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            d.uCallbackMessage = WM_TRAY;
            // иконка лежит в ресурсах exe под номером 1 (build.rs); если её нет, берём стандартную
            let hinst = GetModuleHandleW(std::ptr::null());
            let mut icon = LoadIconW(hinst, 1usize as *const u16);
            if icon.is_null() {
                icon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
            }
            d.hIcon = icon;
            let tip = wide("GameLens: идёт запись (клик — открыть)");
            for (i, c) in tip.iter().take(127).enumerate() {
                d.szTip[i] = *c;
            }
            d
        }
    }

    fn open_ui() {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
        }
    }

    unsafe fn menu(hwnd: HWND) {
        unsafe {
            let m = CreatePopupMenu();
            if m.is_null() {
                return;
            }
            AppendMenuW(m, MF_STRING, ID_OPEN, wide("Открыть GameLens").as_ptr());
            AppendMenuW(m, MF_STRING, ID_STOP, wide("Остановить запись").as_ptr());
            let mut p: POINT = std::mem::zeroed();
            GetCursorPos(&mut p);
            SetForegroundWindow(hwnd); // без этого меню не закрывается кликом вне него
            TrackPopupMenu(m, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, p.x, p.y, 0, hwnd, std::ptr::null());
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(m);
        }
    }

    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            if msg == WM_TRAY {
                match (lp as u32) & 0xFFFF {
                    WM_LBUTTONUP => open_ui(),
                    WM_RBUTTONUP => menu(hwnd),
                    _ => {}
                }
                return 0;
            }
            if msg == WM_COMMAND {
                match wp & 0xFFFF {
                    ID_OPEN => open_ui(),
                    ID_STOP => {
                        let _ = std::fs::create_dir_all(crate::paths::data_dir());
                        let _ = std::fs::write(crate::paths::stop_file(), b"1");
                    }
                    _ => {}
                }
                return 0;
            }
            // проводник перезапустился: значки трея пропали, ставим заново
            if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::Relaxed) {
                Shell_NotifyIconW(NIM_ADD, &nid(hwnd));
                return 0;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    }

    pub fn run(thread_id: Arc<AtomicU32>) {
        unsafe {
            let hinst = GetModuleHandleW(std::ptr::null());
            let class = wide("GameLensTray");
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wndproc);
            wc.hInstance = hinst;
            wc.lpszClassName = class.as_ptr();
            RegisterClassW(&wc);
            TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);
            // окно-невидимка только для сообщений: его не видно и в панели задач
            let hwnd = CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, HWND_MESSAGE, std::ptr::null_mut(), hinst, std::ptr::null());
            if hwnd.is_null() {
                return;
            }
            Shell_NotifyIconW(NIM_ADD, &nid(hwnd));
            thread_id.store(GetCurrentThreadId(), Ordering::Release);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            Shell_NotifyIconW(NIM_DELETE, &nid(hwnd));
            DestroyWindow(hwnd);
        }
    }
}
