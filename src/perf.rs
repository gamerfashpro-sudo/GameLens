//! Windows-счётчики производительности (PDH) и определение игры на переднем плане.
//! Объявления функций написаны вручную по документации Win32 (pdh.dll / user32.dll),
//! без сторонних обёрток. Имена счётчиков — английские (PdhAddEnglishCounterW),
//! поэтому работают и на русской Windows. Любой сбой = None, программа не падает.

#[derive(Default, Clone, Debug)]
pub struct PerfSnap {
    pub gpu_pct: Option<f32>,
    pub disk_pct: Option<f32>,
    pub disk_lat_ms: Option<f32>,
    pub cpu_freq_pct: Option<f32>,
    pub pages_per_s: Option<f32>,
    pub dpc_pct: Option<f32>,
}

#[cfg(windows)]
mod imp {
    use super::PerfSnap;
    use std::collections::HashMap;

    #[repr(C)]
    struct CounterValue {
        status: u32,
        value: f64,
    }
    #[repr(C)]
    struct Item {
        name: *const u16,
        status: u32,
        value: f64,
    }

    #[link(name = "pdh")]
    extern "system" {
        fn PdhOpenQueryW(src: *const u16, user: usize, q: *mut isize) -> u32;
        fn PdhAddEnglishCounterW(q: isize, path: *const u16, user: usize, c: *mut isize) -> u32;
        fn PdhCollectQueryData(q: isize) -> u32;
        fn PdhGetFormattedCounterValue(c: isize, fmt: u32, typ: *mut u32, v: *mut CounterValue) -> u32;
        fn PdhGetFormattedCounterArrayW(c: isize, fmt: u32, size: *mut u32, count: *mut u32, buf: *mut u8) -> u32;
        fn PdhCloseQuery(q: isize) -> u32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> isize;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    }

    const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
    const PDH_MORE_DATA: u32 = 0x8000_07D2;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub struct Perf {
        q: isize,
        gpu: isize,
        disk: isize,
        lat: isize,
        freq: isize,
        pages: isize,
        dpc: isize,
    }

    impl Perf {
        pub fn new() -> Self {
            let mut q: isize = 0;
            unsafe {
                if PdhOpenQueryW(std::ptr::null(), 0, &mut q) != 0 {
                    q = 0;
                }
            }
            let add = |path: &str| -> isize {
                if q == 0 {
                    return 0;
                }
                let mut c: isize = 0;
                let r = unsafe { PdhAddEnglishCounterW(q, wide(path).as_ptr(), 0, &mut c) };
                if r == 0 { c } else { 0 }
            };
            let p = Perf {
                q,
                gpu: add("\\GPU Engine(*)\\Utilization Percentage"),
                disk: add("\\PhysicalDisk(_Total)\\% Disk Time"),
                lat: add("\\PhysicalDisk(_Total)\\Avg. Disk sec/Transfer"),
                freq: add("\\Processor Information(_Total)\\% Processor Performance"),
                pages: add("\\Memory\\Pages Input/sec"),
                dpc: add("\\Processor Information(_Total)\\% DPC Time"),
            };
            if q != 0 {
                unsafe { PdhCollectQueryData(q) };
            }
            p
        }

        fn val(&self, c: isize) -> Option<f32> {
            if c == 0 {
                return None;
            }
            let mut v = CounterValue { status: 0, value: 0.0 };
            let r = unsafe { PdhGetFormattedCounterValue(c, PDH_FMT_DOUBLE, std::ptr::null_mut(), &mut v) };
            if r != 0 || v.status != 0 || !v.value.is_finite() {
                None
            } else {
                Some(v.value as f32)
            }
        }

        /// GPU: как в диспетчере задач — суммируем процессы по каждому физическому движку 3D, берём самый занятый.
        fn gpu_val(&self) -> Option<f32> {
            if self.gpu == 0 {
                return None;
            }
            let mut size: u32 = 0;
            let mut count: u32 = 0;
            let r = unsafe { PdhGetFormattedCounterArrayW(self.gpu, PDH_FMT_DOUBLE, &mut size, &mut count, std::ptr::null_mut()) };
            if r != PDH_MORE_DATA || size == 0 {
                return None;
            }
            let mut buf = vec![0u64; (size as usize + 7) / 8];
            let r = unsafe {
                PdhGetFormattedCounterArrayW(self.gpu, PDH_FMT_DOUBLE, &mut size, &mut count, buf.as_mut_ptr() as *mut u8)
            };
            if r != 0 {
                return None;
            }
            let items = buf.as_ptr() as *const Item;
            let mut per_engine: HashMap<String, f64> = HashMap::new();
            for i in 0..count as usize {
                let it = unsafe { &*items.add(i) };
                if it.status != 0 || it.name.is_null() {
                    continue;
                }
                let mut len = 0usize;
                unsafe {
                    while *it.name.add(len) != 0 {
                        len += 1;
                    }
                }
                let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(it.name, len) });
                if !name.contains("engtype_3D") {
                    continue;
                }
                let key = name.split("_luid_").nth(1).unwrap_or(&name).to_string();
                *per_engine.entry(key).or_insert(0.0) += it.value;
            }
            per_engine.values().cloned().fold(None, |m: Option<f64>, v| Some(m.map_or(v, |x| x.max(v)))).map(|v| v.min(100.0) as f32)
        }

        pub fn read(&mut self) -> PerfSnap {
            if self.q == 0 {
                return PerfSnap::default();
            }
            if unsafe { PdhCollectQueryData(self.q) } != 0 {
                return PerfSnap::default();
            }
            PerfSnap {
                gpu_pct: self.gpu_val(),
                disk_pct: self.val(self.disk).map(|v| v.min(100.0)),
                disk_lat_ms: self.val(self.lat).map(|v| v * 1000.0),
                cpu_freq_pct: self.val(self.freq),
                pages_per_s: self.val(self.pages),
                dpc_pct: self.val(self.dpc),
            }
        }
    }

    impl Drop for Perf {
        fn drop(&mut self) {
            if self.q != 0 {
                unsafe { PdhCloseQuery(self.q) };
            }
        }
    }

    pub fn foreground_pid() -> Option<u32> {
        unsafe {
            let h = GetForegroundWindow();
            if h == 0 {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, &mut pid);
            if pid == 0 { None } else { Some(pid) }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::PerfSnap;
    pub struct Perf;
    impl Perf {
        pub fn new() -> Self {
            Perf
        }
        pub fn read(&mut self) -> PerfSnap {
            PerfSnap::default()
        }
    }
    pub fn foreground_pid() -> Option<u32> {
        None
    }
}

pub use imp::{foreground_pid, Perf};
