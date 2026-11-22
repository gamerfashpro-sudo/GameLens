//! Одно окно GameLens: повторный запуск (ярлык, значок в трее) показывает уже открытое окно, а не открывает второе.

/// true — мы первые и можем открывать окно. `wait`: ждать до 5 с, пока прошлый экземпляр закроется
/// (после обновления новый процесс стартует раньше, чем старый успевает выйти).
pub fn acquire(wait: bool) -> bool {
    #[cfg(any(windows, gl_check))]
    unsafe {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name: Vec<u16> = "Local\\GameLensUI".encode_utf16().chain(std::iter::once(0)).collect();
        for _ in 0..if wait { 50 } else { 1 } {
            let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if h.is_null() {
                return true; // не смогли создать замок: лучше открыть окно, чем не открыть ничего
            }
            if GetLastError() != ERROR_ALREADY_EXISTS {
                return true; // замок держим до конца работы процесса
            }
            windows_sys::Win32::Foundation::CloseHandle(h);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }
    #[cfg(not(any(windows, gl_check)))]
    {
        let _ = wait;
        true
    }
}

/// Выводит уже открытое окно GameLens на передний план.
pub fn focus_existing() {
    #[cfg(any(windows, gl_check))]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let title: Vec<u16> = "GameLens".encode_utf16().chain(std::iter::once(0)).collect();
        let h = FindWindowW(std::ptr::null(), title.as_ptr());
        if !h.is_null() {
            ShowWindow(h, SW_RESTORE);
            SetForegroundWindow(h);
        }
    }
}
