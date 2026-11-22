//! Измерение FPS чужой игры без внедрения в неё.
//!
//! Принцип: Windows сама пишет системные события «игра вызвала Present» (показ кадра) от провайдеров
//! Microsoft-Windows-DXGI (DirectX 10/11/12) и Microsoft-Windows-D3D9. Мы подписываемся на эти события
//! через ETW (Event Tracing for Windows), считаем их по процессам и выбираем процесс игры.
//! Ничего не подгружается в игру, поэтому античиты это не считают вмешательством.
//! Для запуска трассировки нужны права администратора или членство в группе «Performance Log Users».
//!
//! Ограничения: Vulkan/OpenGL в эксклюзивном режиме могут не попасть в эти провайдеры.

use std::collections::{HashMap, VecDeque};

/// Скользящее окно подсчёта кадров: считает FPS по процессам и выбирает процесс игры.
/// Окно ~2 секунды даёт плавное число без резких скачков.
pub struct FpsCalc {
    window: VecDeque<(f32, HashMap<u32, u32>)>,
    max_secs: f32,
}

impl FpsCalc {
    pub fn new(max_secs: f32) -> Self {
        FpsCalc { window: VecDeque::new(), max_secs }
    }

    /// `secs` — сколько секунд прошло с прошлого вызова, `counts` — сколько кадров показал каждый процесс.
    pub fn push(&mut self, secs: f32, counts: HashMap<u32, u32>) {
        self.window.push_back((secs, counts));
        while self.window.len() > 1 && self.window.iter().map(|(s, _)| s).sum::<f32>() > self.max_secs {
            self.window.pop_front();
        }
    }

    fn fps_of(&self, pid: u32) -> f32 {
        let secs: f32 = self.window.iter().map(|(s, _)| s).sum();
        if secs <= 0.0 {
            return 0.0;
        }
        let frames: u32 = self.window.iter().map(|(_, c)| c.get(&pid).copied().unwrap_or(0)).sum();
        frames as f32 / secs
    }

    /// Процесс на переднем плане, если он показывает кадры; иначе самый «кадровый» процесс (не меньше 5 FPS).
    pub fn pick(&self, fg: Option<u32>) -> Option<(u32, f32)> {
        if let Some(p) = fg {
            let f = self.fps_of(p);
            if f >= 1.0 {
                return Some((p, f));
            }
        }
        let mut pids: Vec<u32> = self.window.iter().flat_map(|(_, c)| c.keys().copied()).collect();
        pids.sort_unstable();
        pids.dedup();
        pids.into_iter().map(|p| (p, self.fps_of(p))).filter(|(_, f)| *f >= 5.0).max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[allow(dead_code)]
pub enum TraceStatus {
    Ok,
    NoRights,
    Unsupported,
    Error(u32),
}

#[cfg(any(windows, gl_check))]
mod etw {
    use super::TraceStatus;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Diagnostics::Etw::*;

    const DXGI: GUID = GUID::from_u128(0xCA11C036_0102_4A2D_A6AD_F03CFED5D3C9);
    const D3D9: GUID = GUID::from_u128(0x783ACA0A_790E_4D7F_8451_AA850511C6B9);
    /// DXGI: событие 42 = начало Present; D3D9: событие 1 = начало Present
    const DXGI_PRESENT_START: u16 = 42;
    const D3D9_PRESENT_START: u16 = 1;
    const ERROR_ACCESS_DENIED: u32 = 5;

    fn same(a: &GUID, b: &GUID) -> bool {
        a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
    }

    fn counts() -> &'static Mutex<HashMap<u32, u32>> {
        static C: OnceLock<Mutex<HashMap<u32, u32>>> = OnceLock::new();
        C.get_or_init(|| Mutex::new(HashMap::new()))
    }

    unsafe extern "system" fn on_event(rec: *mut EVENT_RECORD) {
        if rec.is_null() {
            return;
        }
        let r = unsafe { &*rec };
        let id = r.EventHeader.EventDescriptor.Id;
        let prov = &r.EventHeader.ProviderId;
        let is_present = (same(prov, &DXGI) && id == DXGI_PRESENT_START) || (same(prov, &D3D9) && id == D3D9_PRESENT_START);
        if is_present {
            if let Ok(mut m) = counts().lock() {
                *m.entry(r.EventHeader.ProcessId).or_insert(0) += 1;
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Буфер EVENT_TRACE_PROPERTIES + имя сессии сразу за ним (так требует API).
    struct Props {
        buf: Vec<u64>,
    }

    impl Props {
        fn new(name: &[u16]) -> Props {
            let head = std::mem::size_of::<EVENT_TRACE_PROPERTIES>();
            let total = head + name.len() * 2;
            let mut buf = vec![0u64; (total + 7) / 8];
            let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
            unsafe {
                (*p).Wnode.BufferSize = total as u32;
                (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
                (*p).Wnode.ClientContext = 1; // время событий в QPC
                (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
                (*p).BufferSize = 64; // КБ
                (*p).MinimumBuffers = 2;
                (*p).MaximumBuffers = 8;
                (*p).LoggerNameOffset = head as u32;
                let dst = (buf.as_mut_ptr() as *mut u8).add(head) as *mut u16;
                std::ptr::copy_nonoverlapping(name.as_ptr(), dst, name.len());
            }
            Props { buf }
        }
        fn ptr(&mut self) -> *mut EVENT_TRACE_PROPERTIES {
            self.buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES
        }
    }

    fn stop_by_name(name: &[u16]) {
        let mut props = Props::new(name);
        unsafe {
            let _ = ControlTraceW(CONTROLTRACE_HANDLE { Value: 0 }, name.as_ptr(), props.ptr(), EVENT_TRACE_CONTROL_STOP);
        }
    }

    fn status_of(code: u32) -> TraceStatus {
        match code {
            0 => TraceStatus::Ok,
            ERROR_ACCESS_DENIED => TraceStatus::NoRights,
            c => TraceStatus::Error(c),
        }
    }

    /// Пробный запуск и остановка сессии: хватает ли прав.
    pub fn probe() -> TraceStatus {
        let name = wide("GameLensFpsProbe");
        stop_by_name(&name);
        let mut props = Props::new(&name);
        let mut h = CONTROLTRACE_HANDLE { Value: 0 };
        let r = unsafe { StartTraceW(&mut h, name.as_ptr(), props.ptr()) };
        if r == 0 {
            stop_by_name(&name);
        }
        status_of(r)
    }

    pub struct Session {
        name: Vec<u16>,
        reader: Option<std::thread::JoinHandle<()>>,
    }

    impl Session {
        pub fn start() -> Result<Session, TraceStatus> {
            let name = wide("GameLensFpsSession");
            stop_by_name(&name); // убираем сессию, оставшуюся после аварийного завершения
            let mut props = Props::new(&name);
            let mut control = CONTROLTRACE_HANDLE { Value: 0 };
            let r = unsafe { StartTraceW(&mut control, name.as_ptr(), props.ptr()) };
            if r != 0 {
                return Err(status_of(r));
            }
            for g in [&DXGI, &D3D9] {
                let r = unsafe { EnableTraceEx2(control, g, EVENT_CONTROL_CODE_ENABLE_PROVIDER, TRACE_LEVEL_INFORMATION as u8, u64::MAX, 0, 0, std::ptr::null()) };
                if r != 0 {
                    stop_by_name(&name);
                    return Err(status_of(r));
                }
            }
            let mut logger = name.clone();
            let reader = std::thread::Builder::new()
                .name("fps-etw".into())
                .spawn(move || unsafe {
                    let mut log = EVENT_TRACE_LOGFILEW::default();
                    log.LoggerName = logger.as_mut_ptr();
                    log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
                    log.Anonymous2.EventRecordCallback = Some(on_event);
                    let h = OpenTraceW(&mut log);
                    if h.Value == u64::MAX {
                        return; // INVALID_PROCESSTRACE_HANDLE
                    }
                    let _ = ProcessTrace(&h, 1, std::ptr::null(), std::ptr::null()); // блокируется, пока сессию не остановят
                    let _ = CloseTrace(h);
                })
                .map_err(|_| TraceStatus::Error(0))?;
            Ok(Session { name, reader: Some(reader) })
        }

        /// Забирает накопленные счётчики кадров по процессам.
        pub fn take(&self) -> HashMap<u32, u32> {
            counts().lock().map(|mut m| std::mem::take(&mut *m)).unwrap_or_default()
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            stop_by_name(&self.name);
            if let Some(h) = self.reader.take() {
                let _ = h.join();
            }
        }
    }
}

/// Живой измеритель FPS: держит ETW-сессию и скользящее окно.
pub struct FpsMeter {
    #[cfg(any(windows, gl_check))]
    session: etw::Session,
    calc: FpsCalc,
    last: std::time::Instant,
}

impl FpsMeter {
    pub fn start() -> Result<FpsMeter, TraceStatus> {
        #[cfg(any(windows, gl_check))]
        {
            let session = etw::Session::start()?;
            Ok(FpsMeter { session, calc: FpsCalc::new(2.0), last: std::time::Instant::now() })
        }
        #[cfg(not(any(windows, gl_check)))]
        {
            Err(TraceStatus::Unsupported)
        }
    }

    /// Вызывать примерно раз в 0.5 с. Возвращает FPS игры или None, если кадров нет.
    pub fn poll(&mut self, fg_pid: Option<u32>) -> Option<f32> {
        let now = std::time::Instant::now();
        let secs = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        #[cfg(any(windows, gl_check))]
        {
            let c = self.session.take();
            self.calc.push(secs, c);
        }
        #[cfg(not(any(windows, gl_check)))]
        {
            let _ = secs;
        }
        self.calc.pick(fg_pid).map(|(_, f)| f)
    }
}

/// Хватает ли прав на измерение FPS (для вкладки «Настройки»).
pub fn probe_rights() -> TraceStatus {
    #[cfg(any(windows, gl_check))]
    {
        etw::probe()
    }
    #[cfg(not(any(windows, gl_check)))]
    {
        TraceStatus::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(v: &[(u32, u32)]) -> HashMap<u32, u32> {
        v.iter().cloned().collect()
    }

    #[test]
    fn computes_fps_per_process() {
        let mut f = FpsCalc::new(2.0);
        f.push(0.5, c(&[(100, 30), (200, 2)]));
        f.push(0.5, c(&[(100, 30), (200, 2)]));
        assert_eq!(f.pick(Some(100)).unwrap().0, 100);
        assert!((f.pick(Some(100)).unwrap().1 - 60.0).abs() < 0.01);
    }

    #[test]
    fn prefers_foreground_but_falls_back() {
        let mut f = FpsCalc::new(2.0);
        f.push(1.0, c(&[(100, 120), (200, 60)]));
        // на переднем плане лаунчер без кадров: берём самый кадровый процесс
        assert_eq!(f.pick(Some(999)).unwrap().0, 100);
        // на переднем плане второй процесс с кадрами: берём его
        assert_eq!(f.pick(Some(200)).unwrap().0, 200);
    }

    #[test]
    fn window_smooths_and_forgets_old_frames() {
        let mut f = FpsCalc::new(2.0);
        f.push(1.0, c(&[(1, 100)]));
        f.push(1.0, c(&[(1, 100)]));
        f.push(1.0, c(&[(1, 40)])); // старые 100 уходят из окна по мере поступления
        let fps = f.pick(Some(1)).unwrap().1;
        assert!(fps > 40.0 && fps < 100.0, "{fps}");
    }

    #[test]
    fn nothing_when_no_frames() {
        let mut f = FpsCalc::new(2.0);
        f.push(1.0, c(&[(5, 1)]));
        assert!(f.pick(None).is_none());
    }
}
