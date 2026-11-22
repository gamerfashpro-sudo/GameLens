use crate::model::{ProcSample, Sample};
use crate::perf::{self, Perf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use sysinfo::{MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, CpuRefreshKind};

const WATCH_SLEEP_MS: u64 = 10;

/// Поток-«сторож»: спит 10 мс и измеряет, насколько опоздал проснуться.
/// Если система где-то «застыла» (драйвер, антивирус, диск, нехватка CPU),
/// опоздание вырастет. Стоит почти ноль: ~100 пробуждений в секунду.
struct StallWatch {
    max_over_ms: Arc<AtomicU32>,
}

impl StallWatch {
    fn start() -> Self {
        let max_over_ms = Arc::new(AtomicU32::new(0));
        let m = max_over_ms.clone();
        std::thread::Builder::new()
            .name("stall-watch".into())
            .spawn(move || loop {
                let t = Instant::now();
                std::thread::sleep(Duration::from_millis(WATCH_SLEEP_MS));
                let over = t.elapsed().as_millis() as u64;
                let over = over.saturating_sub(WATCH_SLEEP_MS) as u32;
                m.fetch_max(over, Ordering::Relaxed);
            })
            .expect("spawn stall-watch");
        Self { max_over_ms }
    }
    fn take(&self) -> u32 {
        self.max_over_ms.swap(0, Ordering::Relaxed)
    }
}

pub struct Collector {
    sys: System,
    start: Instant,
    watch: StallWatch,
    cores: usize,
    top_n: usize,
    perf: Perf,
}

impl Collector {
    pub fn new(top_n: usize) -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram().with_swap()),
        );
        sys.refresh_cpu_usage();
        let cores = sys.cpus().len().max(1);
        Self { sys, start: Instant::now(), watch: StallWatch::start(), cores, top_n, perf: Perf::new() }
    }

    pub fn sample(&mut self) -> Sample {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );

        let fg = perf::foreground_pid()
            .and_then(|pid| self.sys.process(sysinfo::Pid::from_u32(pid)))
            .map(|p| p.name().to_string_lossy().into_owned())
            .unwrap_or_default();
        let snap = self.perf.read();

        let mut procs: Vec<ProcSample> = self
            .sys
            .processes()
            .values()
            .map(|p| ProcSample {
                name: p.name().to_string_lossy().into_owned(),
                pid: p.pid().as_u32(),
                cpu: p.cpu_usage() / self.cores as f32,
                mem_mb: (p.memory() / 1_048_576) as u32,
            })
            .filter(|p| p.name != "System Idle Process")
            .collect();
        procs.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
        procs.truncate(self.top_n);

        Sample {
            t_ms: self.start.elapsed().as_millis() as u64,
            cpu_total: self.sys.global_cpu_usage(),
            cpu_cores: self.sys.cpus().iter().map(|c| c.cpu_usage().round() as u8).collect(),
            mem_used_mb: (self.sys.used_memory() / 1_048_576) as u32,
            mem_total_mb: (self.sys.total_memory() / 1_048_576) as u32,
            swap_used_mb: (self.sys.used_swap() / 1_048_576) as u32,
            stall_ms: self.watch.take(),
            top: procs,
            gpu_pct: snap.gpu_pct,
            disk_pct: snap.disk_pct,
            disk_lat_ms: snap.disk_lat_ms,
            cpu_freq_pct: snap.cpu_freq_pct,
            pages_per_s: snap.pages_per_s,
            dpc_pct: snap.dpc_pct,
            fps: None,
            fg,
        }
    }
}
