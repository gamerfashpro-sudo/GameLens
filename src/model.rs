use serde::{Deserialize, Serialize};

/// Один из самых «тяжёлых» процессов в момент замера.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProcSample {
    pub name: String,
    pub pid: u32,
    /// % от ВСЕЙ мощности CPU (0..100), не от одного ядра
    pub cpu: f32,
    pub mem_mb: u32,
}

/// Один замер состояния системы.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Sample {
    /// миллисекунды от старта записи
    pub t_ms: u64,
    pub cpu_total: f32,
    pub cpu_cores: Vec<u8>,
    pub mem_used_mb: u32,
    pub mem_total_mb: u32,
    pub swap_used_mb: u32,
    /// Самая длинная задержка планировщика за интервал, мс.
    /// Поток просит сон 10 мс; если проснулся на 60 — система «стояла» ~50 мс.
    pub stall_ms: u32,
    pub top: Vec<ProcSample>,
    /// Windows-счётчики (None = недоступно). Старые записи читаются без них.
    #[serde(default)]
    pub gpu_pct: Option<f32>,
    #[serde(default)]
    pub disk_pct: Option<f32>,
    /// средняя задержка диска на операцию, мс
    #[serde(default)]
    pub disk_lat_ms: Option<f32>,
    /// % от БАЗОВОЙ частоты CPU: выше 100 = турбо, ниже 100 при нагрузке = троттлинг
    #[serde(default)]
    pub cpu_freq_pct: Option<f32>,
    #[serde(default)]
    pub pages_per_s: Option<f32>,
    /// % времени в обработке прерываний драйверов (DPC)
    #[serde(default)]
    pub dpc_pct: Option<f32>,
    /// процесс на переднем плане (почти всегда это игра)
    #[serde(default)]
    pub fg: String,
}

impl Sample {
    pub fn mem_pct(&self) -> f32 {
        if self.mem_total_mb == 0 {
            0.0
        } else {
            self.mem_used_mb as f32 * 100.0 / self.mem_total_mb as f32
        }
    }
    pub fn max_core(&self) -> u8 {
        self.cpu_cores.iter().copied().max().unwrap_or(0)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Cause {
    /// Конкретный процесс сильно грузил CPU
    HeavyProcess { name: String, cpu: f32 },
    /// Одно ядро в потолок при низкой общей загрузке — игра упёрлась в один поток
    SingleThreadBound { core_pct: u8, total_pct: f32 },
    /// Весь CPU загружен
    CpuSaturated { total_pct: f32 },
    /// Мало свободной RAM / активный своп
    MemoryPressure { used_pct: f32, swap_mb: u32 },
    /// Система «зависала» без видимой нагрузки (драйвер, диск, антивирус, температура)
    UnexplainedStall { stall_ms: u32 },
    /// Видеокарта загружена почти полностью
    GpuBound { gpu_pct: f32 },
    /// Диск тормозил (высокая задержка / загрузка)
    DiskSlow { lat_ms: f32, busy_pct: f32 },
    /// Драйверы съели время обработкой прерываний
    DriverLatency { dpc_pct: f32 },
    /// Частота CPU упала ниже базовой при нагрузке — перегрев или лимит питания
    CpuThrottle { freq_pct: f32 },
    /// Активная подкачка (страницы читаются с диска)
    HardPageFaults { pages_per_s: f32 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Incident {
    pub start_ms: u64,
    pub end_ms: u64,
    pub peak_stall_ms: u32,
    pub causes: Vec<Cause>,
}
