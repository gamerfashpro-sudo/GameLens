use crate::config::{Config, OverlayMode};
use crate::fps::{FpsMeter, TraceStatus};
use crate::overlay::Overlay;
use crate::rtss::Rtss;
use crate::{collector::Collector, paths, perf, procs, store};
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
    // закрываем программы, которые пользователь отметил как ненужные во время игры
    let stopped = if cfg.auto_stop_on { procs::stop_for_game(&cfg.auto_stop, perf::foreground_pid()) } else { vec![] };
    procs::save_stopped(&stopped);
    let mut display = Display::new(cfg.clone());
    let tray = if cfg.tray { Some(crate::tray::Tray::spawn()) } else { None };
    // FPS берём из RTSS, если он запущен: это ничего не стоит и не требует прав. Если RTSS нет, считаем
    // сами по событиям Windows (ETW), но только пока RTSS не появился.
    let mut meter: Option<FpsMeter> = None;
    let mut etw_tried = false;
    let mut rtss_miss = 0u32; // полусекунд подряд, когда RTSS запущен, но FPS игры не отдал
    let mut note_ticks = 0u32; // сколько полусекунд показывать сообщение о проблеме
    let mut last_fps: Option<f32> = None;

    'rec: loop {
        for tick in 0..8 {
            std::thread::sleep(Duration::from_millis(250));
            if paths::stop_file().exists() {
                break 'rec;
            }
            if tick % 2 == 1 {
                let fg = perf::foreground_pid();
                display.tick();
                let from_rtss = display.rtss_fps(fg);
                if from_rtss.is_some() {
                    // RTSS даёт FPS: своя трассировка не нужна и снимается; при его пропаже попробуем заново
                    meter = None;
                    etw_tried = false;
                    rtss_miss = 0;
                    note_ticks = 0;
                } else if display.has_rtss() {
                    rtss_miss += 1; // RTSS запущен, но игру не знает (не подключился к ней)
                } else {
                    rtss_miss = 0;
                }
                // своя трассировка: если RTSS нет вовсе или он 3 с подряд не видит игру
                if cfg.fps_overlay && meter.is_none() && !etw_tried && (!display.has_rtss() || rtss_miss >= 6) {
                    etw_tried = true;
                    match FpsMeter::start() {
                        Ok(m) => meter = Some(m),
                        Err(e) if !display.has_rtss() => {
                            note_ticks = 20;
                            display.show(match e {
                                TraceStatus::NoRights => Msg::NoRights,
                                _ => Msg::Unavailable,
                            });
                        }
                        Err(_) => {}
                    }
                }
                if let Some(m) = meter.as_mut() {
                    let from_etw = m.poll(fg);
                    last_fps = from_rtss.or(from_etw);
                } else {
                    last_fps = from_rtss;
                }
                if meter.is_some() || display.has_rtss() {
                    // игра не показывает кадры: надпись скрыта
                    display.show(last_fps.map_or(Msg::Hidden, Msg::Fps));
                } else if note_ticks > 0 {
                    note_ticks -= 1;
                    if note_ticks == 0 {
                        display.show(Msg::Hidden);
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
    display.show(Msg::Hidden);
    drop(display); // отдаёт запись в RTSS
    drop(tray); // убирает значок из трея
    if cfg.restart_after {
        procs::restart(&stopped); // возвращаем то, что закрыли при входе
    }
    drop(meter); // останавливает ETW-сессию
    let _ = std::fs::remove_file(paths::stop_file());
    let _ = std::fs::remove_file(paths::pid_file());
}

/// Что показать на экране.
enum Msg {
    Fps(f32),
    NoRights,
    Unavailable,
    Hidden,
}

/// Куда рисовать надпись: в RTSS (любой режим игры) или в своё окно (оконный режим и «окно без рамки»).
struct Display {
    cfg: Config,
    own: Option<Overlay>,
    rtss: Option<Rtss>,
    /// сколько вызовов ждать до следующей попытки найти RTSS (ищем раз в ~4 с, а не каждые полсекунды)
    wait: u32,
}

impl Display {
    fn new(cfg: Config) -> Display {
        Display { cfg, own: None, rtss: None, wait: 0 }
    }

    /// Раз в полсекунды: ищет RTSS (не чаще раза в ~4 с), если он ещё не подключён.
    fn tick(&mut self) {
        if self.rtss.is_none() {
            if self.wait == 0 {
                self.rtss = Rtss::connect();
                self.wait = 8;
            } else {
                self.wait -= 1;
            }
        }
    }

    fn has_rtss(&self) -> bool {
        self.rtss.is_some()
    }

    /// FPS игры из RTSS (None: RTSS нет, не знает игру или кадры не идут).
    fn rtss_fps(&mut self, fg: Option<u32>) -> Option<f32> {
        self.rtss.as_mut().and_then(|r| r.fps(fg))
    }

    fn show(&mut self, m: Msg) {
        if !self.cfg.fps_overlay {
            return;
        }
        if self.cfg.mode != OverlayMode::Own {
            if let Some(r) = self.rtss.as_mut() {
                let text = match m {
                    Msg::Fps(f) => format!("{:.0} FPS", f),
                    Msg::NoRights => "FPS: need rights (GameLens > Settings)".to_string(),
                    Msg::Unavailable => "FPS unavailable".to_string(),
                    Msg::Hidden => String::new(),
                };
                if r.show(&text) {
                    // своё окно при этом убираем, чтобы надпись не двоилась
                    if let Some(o) = &self.own {
                        o.set_text("");
                    }
                    return;
                }
                self.rtss = None; // RTSS закрыли или память не подошла
            }
            if self.cfg.mode == OverlayMode::Rtss {
                return;
            }
        }
        let text = match m {
            Msg::Fps(f) => format!("{:.0} FPS", f),
            Msg::NoRights => "FPS: нужны права (GameLens > Настройки)".to_string(),
            Msg::Unavailable => "FPS недоступен".to_string(),
            Msg::Hidden => String::new(),
        };
        if text.is_empty() && self.own.is_none() {
            return; // нечего скрывать: окно даже не создаём
        }
        let cfg = &self.cfg;
        self.own.get_or_insert_with(|| Overlay::spawn(cfg.clone())).set_text(&text);
    }
}

fn chrono_like_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // сортируемое имя: число секунд с 1970 (дополнено нулями)
    format!("{:012}", secs)
}
