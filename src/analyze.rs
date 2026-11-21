use crate::model::{Cause, Incident, Sample};
use std::collections::HashMap;

pub const STALL_MS: u32 = 50;
pub const CPU_SAT: f32 = 95.0;
pub const MEM_PRESSURE: f32 = 92.0;
pub const DISK_LAT_MS: f32 = 50.0;
pub const DPC_PCT: f32 = 15.0;
const MERGE_GAP_MS: u64 = 3000;
const SPIKE_OVER_BASELINE: f32 = 15.0;

fn is_trigger(s: &Sample) -> bool {
    s.stall_ms >= STALL_MS
        || s.cpu_total >= CPU_SAT
        || s.mem_pct() >= MEM_PRESSURE
        || s.disk_lat_ms.unwrap_or(0.0) >= DISK_LAT_MS
        || s.dpc_pct.unwrap_or(0.0) >= DPC_PCT
}

fn median(v: &mut Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Обычная нагрузка каждого процесса за всю сессию (медиана по замерам, где он был в топе).
/// Если процесса в топе не было — считаем его базу нулевой.
fn baselines(samples: &[Sample]) -> HashMap<String, f32> {
    let mut per: HashMap<String, Vec<f32>> = HashMap::new();
    for s in samples {
        for p in &s.top {
            per.entry(p.name.clone()).or_default().push(p.cpu);
        }
    }
    // доля замеров, в которых процесс отсутствовал в топе, тоже «тянет» базу к нулю
    let n = samples.len();
    per.into_iter()
        .map(|(k, mut v)| {
            v.resize(n, 0.0);
            (k, median(&mut v))
        })
        .collect()
}

pub fn find_incidents(samples: &[Sample]) -> Vec<Incident> {
    let base = baselines(samples);
    let mut windows: Vec<(usize, usize)> = Vec::new();
    for (i, s) in samples.iter().enumerate() {
        if !is_trigger(s) {
            continue;
        }
        match windows.last_mut() {
            Some((_, end)) if s.t_ms.saturating_sub(samples[*end].t_ms) <= MERGE_GAP_MS => *end = i,
            _ => windows.push((i, i)),
        }
    }

    windows
        .into_iter()
        .map(|(a, b)| {
            let w = &samples[a..=b];
            let mut causes = Vec::new();

            // 1. Процесс, который в этот момент грузил заметно больше обычного
            let mut worst: HashMap<&str, f32> = HashMap::new();
            for s in w {
                for p in s.top.iter().filter(|p| p.name != s.fg) {
                    let e = worst.entry(p.name.as_str()).or_insert(0.0);
                    *e = e.max(p.cpu);
                }
            }
            let mut spikes: Vec<(&str, f32)> = worst
                .into_iter()
                .filter(|(n, c)| c - base.get(*n).copied().unwrap_or(0.0) >= SPIKE_OVER_BASELINE)
                .collect();
            spikes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            for (n, c) in spikes.into_iter().take(3) {
                causes.push(Cause::HeavyProcess { name: n.to_string(), cpu: c });
            }

            // 2. Одно ядро в потолок, общая загрузка низкая
            if let Some(s) = w.iter().find(|s| s.max_core() >= 95 && s.cpu_total < 60.0) {
                causes.push(Cause::SingleThreadBound { core_pct: s.max_core(), total_pct: s.cpu_total });
            }

            // 3. CPU забит целиком
            if let Some(s) = w.iter().max_by(|a, b| a.cpu_total.partial_cmp(&b.cpu_total).unwrap()) {
                if s.cpu_total >= CPU_SAT {
                    causes.push(Cause::CpuSaturated { total_pct: s.cpu_total });
                }
            }

            // 4. Память
            if let Some(s) = w.iter().max_by(|a, b| a.mem_pct().partial_cmp(&b.mem_pct()).unwrap()) {
                if s.mem_pct() >= MEM_PRESSURE {
                    causes.push(Cause::MemoryPressure { used_pct: s.mem_pct(), swap_mb: s.swap_used_mb });
                }
            }

            if let Some(s) = w.iter().filter(|s| s.gpu_pct.is_some()).max_by(|a, b| a.gpu_pct.partial_cmp(&b.gpu_pct).unwrap()) {
                if s.gpu_pct.unwrap() >= 97.0 && s.cpu_total < 85.0 {
                    causes.push(Cause::GpuBound { gpu_pct: s.gpu_pct.unwrap() });
                }
            }
            if let Some(s) = w.iter().filter(|s| s.disk_lat_ms.unwrap_or(0.0) >= DISK_LAT_MS).max_by(|a, b| a.disk_lat_ms.partial_cmp(&b.disk_lat_ms).unwrap()) {
                causes.push(Cause::DiskSlow { lat_ms: s.disk_lat_ms.unwrap(), busy_pct: s.disk_pct.unwrap_or(0.0) });
            }
            if let Some(s) = w.iter().filter(|s| s.dpc_pct.unwrap_or(0.0) >= DPC_PCT).max_by(|a, b| a.dpc_pct.partial_cmp(&b.dpc_pct).unwrap()) {
                causes.push(Cause::DriverLatency { dpc_pct: s.dpc_pct.unwrap() });
            }
            // троттлинг: частота ниже 70% при загрузке CPU выше 50% и при этом была выше 90% в начале сессии
            let max_f = samples.iter().filter_map(|s| s.cpu_freq_pct).fold(0.0f32, f32::max);
            if let Some(s) = w.iter().find(|s| s.cpu_freq_pct.map_or(false, |f| f < 70.0 && max_f >= 90.0) && s.cpu_total >= 50.0) {
                causes.push(Cause::CpuThrottle { freq_pct: s.cpu_freq_pct.unwrap() });
            }
            if let Some(s) = w.iter().filter(|s| s.pages_per_s.unwrap_or(0.0) >= 500.0).max_by(|a, b| a.pages_per_s.partial_cmp(&b.pages_per_s).unwrap()) {
                causes.push(Cause::HardPageFaults { pages_per_s: s.pages_per_s.unwrap() });
            }

            let peak = w.iter().map(|s| s.stall_ms).max().unwrap_or(0);
            // 5. Фриз есть, а объяснения в наших метриках нет — говорим честно
            if causes.is_empty() && peak >= STALL_MS {
                causes.push(Cause::UnexplainedStall { stall_ms: peak });
            }

            Incident { start_ms: w[0].t_ms, end_ms: w[w.len() - 1].t_ms, peak_stall_ms: peak, causes }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProcSample;

    fn s(t: u64, total: f32, core: u8, mem_pct: f32, stall: u32, top: Vec<(&str, f32)>) -> Sample {
        Sample {
            t_ms: t,
            cpu_total: total,
            cpu_cores: vec![core, 10, 10, 10],
            mem_used_mb: (mem_pct * 160.0) as u32,
            mem_total_mb: 16000,
            swap_used_mb: 0,
            stall_ms: stall,
            top: top.into_iter().map(|(n, c)| ProcSample { name: n.into(), pid: 1, cpu: c, mem_mb: 100 }).collect(),
            ..Default::default()
        }
    }

    fn calm(n: u64) -> Vec<Sample> {
        (0..n).map(|i| s(i * 1000, 40.0, 70, 60.0, 2, vec![("game.exe", 35.0)])).collect()
    }

    #[test]
    fn quiet_session_has_no_incidents() {
        assert!(find_incidents(&calm(60)).is_empty());
    }

    #[test]
    fn background_process_spike_is_named_but_game_is_not() {
        let mut v = calm(60);
        for i in 30..33 {
            v[i] = s(i as u64 * 1000, 70.0, 80, 60.0, 120, vec![("game.exe", 35.0), ("MsMpEng.exe", 30.0)]);
        }
        let inc = find_incidents(&v);
        assert_eq!(inc.len(), 1);
        assert!(inc[0].causes.contains(&Cause::HeavyProcess { name: "MsMpEng.exe".into(), cpu: 30.0 }));
        assert!(!inc[0].causes.iter().any(|c| matches!(c, Cause::HeavyProcess { name, .. } if name == "game.exe")));
        assert_eq!(inc[0].peak_stall_ms, 120);
    }

    #[test]
    fn single_core_bottleneck() {
        let mut v = calm(20);
        v[10] = s(10_000, 30.0, 100, 60.0, 80, vec![("game.exe", 25.0)]);
        let inc = find_incidents(&v);
        assert!(matches!(inc[0].causes[0], Cause::SingleThreadBound { core_pct: 100, .. }));
    }

    #[test]
    fn memory_pressure_detected() {
        let mut v = calm(20);
        v[5] = s(5000, 40.0, 70, 96.0, 0, vec![("game.exe", 35.0)]);
        let inc = find_incidents(&v);
        assert!(matches!(inc[0].causes[0], Cause::MemoryPressure { .. }));
    }

    #[test]
    fn stall_without_load_is_reported_honestly() {
        let mut v = calm(20);
        v[8] = s(8000, 40.0, 70, 60.0, 400, vec![("game.exe", 35.0)]);
        let inc = find_incidents(&v);
        assert_eq!(inc[0].causes, vec![Cause::UnexplainedStall { stall_ms: 400 }]);
    }

    #[test]
    fn nearby_triggers_merge_into_one_incident() {
        let mut v = calm(30);
        v[10] = s(10_000, 40.0, 70, 60.0, 100, vec![("game.exe", 35.0)]);
        v[12] = s(12_000, 40.0, 70, 60.0, 100, vec![("game.exe", 35.0)]);
        assert_eq!(find_incidents(&v).len(), 1);
    }
}
