use crate::config::Config;
use crate::fps::{FpsMeter, TraceStatus};
use crate::overlay::Overlay;
use crate::{collector::Collector, paths, perf, store};
use std::time::Duration;

/// Фоновый рекордер для игрового режима: без окна и без истории в памяти.
/// Замер раз в 2 с; сигнал остановки проверяется каждые 250 мс; FPS обновляется раз в 0,5 с.
pub fn run() {
    if paths::recorder_running() {
        return; // уже идёт запись
    }
    let _ = std::fs::create_dir_all(paths::data_dir());
    let _ = std::fs::remove_file(paths::stop_file());
    let _ = std::fs::write(paths::pid_file(), std::process::id().to_string());
    let _ = store::prune(&paths::sessions_dir(), 30);

    let name = format!("session-{}.jsonl", chrono_like_stamp());
    let path = paths::sessions_dir().join(name);
    let Ok(mut w) = store::Writer::create(&path) else { return };
    let mut c = Collector::new(5);

    // FPS и надпись поверх игры (если включено в настройках)
    let cfg = Config::load();
    let overlay = if cfg.fps_overlay { Some(Overlay::spawn(cfg.clone())) } else { None };
    let mut meter = None;
    let mut note_ticks = 0u32; // сколько полусекунд показывать сообщение о проблеме
    if cfg.fps_overlay {
        match FpsMeter::start() {
            Ok(m) => meter = Some(m),
            Err(e) => {
                note_ticks = 20;
                if let Some(o) = &overlay {
                    o.set_text(match e {
                        TraceStatus::NoRights => "FPS: нужны права (GameLens > Настройки)",
                        _ => "FPS недоступен",
                    });
                }
            }
        }
    }
    let mut last_fps: Option<f32> = None;

    'rec: loop {
        for tick in 0..8 {
            std::thread::sleep(Duration::from_millis(250));
            if paths::stop_file().exists() {
                break 'rec;
            }
            if tick % 2 == 1 {
                if let Some(m) = meter.as_mut() {
                    last_fps = m.poll(perf::foreground_pid());
                    if let Some(o) = &overlay {
                        match last_fps {
                            Some(f) => o.set_text(&format!("{:.0} FPS", f)),
                            None => o.set_text(""), // игра не показывает кадры: надпись скрыта
                        }
                    }
                } else if note_ticks > 0 {
                    note_ticks -= 1;
                    if note_ticks == 0 {
                        if let Some(o) = &overlay {
                            o.set_text("");
                        }
                    }
                }
            }
        }
        let mut s = c.sample();
        s.fps = last_fps;
        if w.write(&s).is_err() {
            break;
        }
    }
    drop(meter); // останавливает ETW-сессию
    let _ = std::fs::remove_file(paths::stop_file());
    let _ = std::fs::remove_file(paths::pid_file());
}

fn chrono_like_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // сортируемое имя: число секунд с 1970 (дополнено нулями)
    format!("{:012}", secs)
}
