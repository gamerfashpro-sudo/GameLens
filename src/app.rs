use crate::analyze;
use crate::collector::Collector;
use crate::model::{Cause, Incident, Sample};
use crate::{paths, store, update};
use eframe::egui;
use egui_plot::{Line, Plot, PlotPoints, VLine};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(PartialEq)]
enum Tab {
    Live,
    Sessions,
}

pub struct App {
    tab: Tab,
    recording: bool,
    last_rec_check: Instant,
    collector: Option<Collector>,
    live: VecDeque<Sample>,
    last_sample: Instant,
    sessions: Vec<PathBuf>,
    selected: Option<usize>,
    loaded: Vec<Sample>,
    incidents: Vec<Incident>,
    focus: Option<(f64, f64)>,
    upd: Arc<Mutex<Option<update::Found>>>,
    upd_msg: String,
}

impl App {
    pub fn new() -> Self {
        let upd = Arc::new(Mutex::new(None));
        let u2 = upd.clone();
        std::thread::spawn(move || {
            if let Some(f) = update::check() {
                *u2.lock().unwrap() = Some(f);
            }
        });
        let mut a = App {
            tab: if std::env::var("GAMELENS_TAB").as_deref() == Ok("sessions") { Tab::Sessions } else { Tab::Live },
            recording: paths::recorder_running(),
            last_rec_check: Instant::now(),
            collector: None,
            live: VecDeque::new(),
            last_sample: Instant::now() - Duration::from_secs(5),
            sessions: vec![],
            selected: None,
            loaded: vec![],
            incidents: vec![],
            focus: None,
            upd,
            upd_msg: String::new(),
        };
        a.refresh_sessions();
        a
    }

    fn refresh_sessions(&mut self) {
        let mut v: Vec<PathBuf> = std::fs::read_dir(paths::sessions_dir())
            .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
            .unwrap_or_default();
        v.retain(|p| p.extension().map(|e| e == "jsonl").unwrap_or(false));
        v.sort();
        v.reverse(); // новые сверху
        self.sessions = v;
    }

    fn load(&mut self, i: usize) {
        self.selected = Some(i);
        self.loaded = store::read_all(&self.sessions[i]).unwrap_or_default();
        self.incidents = analyze::find_incidents(&self.loaded);
        self.focus = None;
    }

    fn start_game_mode(&mut self, ctx: &egui::Context) {
        if let Ok(exe) = std::env::current_exe() {
            let mut cmd = std::process::Command::new(exe);
            cmd.arg("--record");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000 | 0x0000_0008); // без окна, отвязан от родителя
            }
            if cmd.spawn().is_ok() {
                // UI закрывается полностью — в памяти остаётся только крошечный рекордер
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn stop_game_mode(&mut self) {
        let _ = std::fs::create_dir_all(paths::data_dir());
        let _ = std::fs::write(paths::stop_file(), b"1");
        self.recording = false;
        self.collector = None;
        std::thread::sleep(Duration::from_millis(300));
        self.refresh_sessions();
    }
}

fn cause_text(c: &Cause) -> String {
    match c {
        Cause::HeavyProcess { name, cpu } => format!("Процесс {name} грузил CPU на {cpu:.0}% (выше его обычного уровня)"),
        Cause::SingleThreadBound { core_pct, total_pct } => format!("Одно ядро {core_pct}% при общей загрузке {total_pct:.0}%: игра упёрлась в один поток"),
        Cause::CpuSaturated { total_pct } => format!("Процессор забит целиком ({total_pct:.0}%)"),
        Cause::MemoryPressure { used_pct, swap_mb } => format!("Не хватало памяти: занято {used_pct:.0}%, своп {swap_mb} МБ"),
        Cause::UnexplainedStall { stall_ms } => format!("Система «стояла» {stall_ms} мс без видимой нагрузки (драйвер, антивирус, диск, перегрев?)"),
        Cause::GpuBound { gpu_pct } => format!("Видеокарта загружена на {gpu_pct:.0}% при свободном CPU: упор в GPU"),
        Cause::DiskSlow { lat_ms, busy_pct } => format!("Диск тормозил: задержка {lat_ms:.0} мс, занят {busy_pct:.0}%"),
        Cause::DriverLatency { dpc_pct } => format!("Драйверы заняли {dpc_pct:.0}% времени на прерывания (DPC)"),
        Cause::CpuThrottle { freq_pct } => format!("Частота CPU упала до {freq_pct:.0}% от максимума: перегрев или лимит питания"),
        Cause::HardPageFaults { pages_per_s } => format!("Активная подкачка с диска: {pages_per_s:.0} страниц/с"),
    }
}

fn series(samples: &[Sample], f: impl Fn(&Sample) -> Option<f32>) -> Vec<[f64; 2]> {
    samples.iter().filter_map(|s| f(s).map(|v| [s.t_ms as f64 / 1000.0, v as f64])).collect()
}

fn chart(ui: &mut egui::Ui, id: &str, title: &str, lines: Vec<(&str, Vec<[f64; 2]>)>, y_max: Option<f64>, marks: &[f64], focus: Option<(f64, f64)>) {
    ui.label(title);
    let mut p = Plot::new(id).height(120.0).allow_scroll(false).include_y(0.0).legend(egui_plot::Legend::default());
    if let Some(m) = y_max {
        p = p.include_y(m);
    }
    p.show(ui, |pui| {
        if let Some((a, b)) = focus {
            pui.set_plot_bounds_x(a..=b);
        }
        for (name, pts) in lines {
            pui.line(Line::new(name, PlotPoints::from(pts)));
        }
        for &m in marks {
            pui.vline(VLine::new("", m).color(egui::Color32::from_rgb(220, 60, 60)));
        }
    });
}

fn charts(ui: &mut egui::Ui, s: &[Sample], marks: &[f64], focus: Option<(f64, f64)>, key: &str) {
    chart(ui, &format!("{key}cpu"), "Процессор, %", vec![("Общая", series(s, |x| Some(x.cpu_total))), ("Самое загруженное ядро", series(s, |x| Some(x.max_core() as f32)))], Some(100.0), marks, focus);
    chart(ui, &format!("{key}ram"), "Оперативная память, %", vec![("RAM", series(s, |x| Some(x.mem_pct())))], Some(100.0), marks, focus);
    if s.iter().any(|x| x.gpu_pct.is_some()) {
        chart(ui, &format!("{key}gpu"), "Видеокарта, %", vec![("GPU 3D", series(s, |x| x.gpu_pct))], Some(100.0), marks, focus);
    }
    if s.iter().any(|x| x.disk_lat_ms.is_some()) {
        chart(ui, &format!("{key}disk"), "Диск: задержка, мс", vec![("Задержка", series(s, |x| x.disk_lat_ms))], None, marks, focus);
    }
    if s.iter().any(|x| x.cpu_freq_pct.is_some()) {
        chart(ui, &format!("{key}freq"), "Частота CPU, % от максимума", vec![("Частота", series(s, |x| x.cpu_freq_pct))], Some(100.0), marks, focus);
    }
    chart(ui, &format!("{key}stall"), "Задержки системы (фризы), мс", vec![("Фриз", series(s, |x| Some(x.stall_ms as f32)))], Some(100.0), marks, focus);
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if self.last_rec_check.elapsed() > Duration::from_secs(2) {
            self.recording = paths::recorder_running();
            self.last_rec_check = Instant::now();
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut gm = self.recording;
                if ui.checkbox(&mut gm, "Игровой режим (окно закроется, запись пойдёт в фоне)").changed() {
                    if gm {
                        self.start_game_mode(ctx);
                    } else {
                        self.stop_game_mode();
                    }
                }
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Live, "Сейчас");
                if ui.selectable_value(&mut self.tab, Tab::Sessions, "Записи").clicked() {
                    self.refresh_sessions();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("v{}", update::VERSION));
                });
            });
            let found = self.upd.lock().unwrap().clone();
            if let Some(f) = found {
                ui.horizontal(|ui| {
                    ui.label(format!("Доступна новая версия {}", f.version));
                    if ui.button("Обновить").clicked() {
                        match update::apply(&f) {
                            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(e) => self.upd_msg = e,
                        }
                    }
                    ui.colored_label(egui::Color32::LIGHT_RED, &self.upd_msg);
                });
            }
        });

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Live => {
                if self.recording {
                    ui.heading("Идёт фоновая запись");
                    ui.label("Окно ничего не измеряет, чтобы не мешать игре. Сними галочку «Игровой режим», чтобы остановить запись, затем открой вкладку «Записи».");
                    return;
                }
                if self.collector.is_none() {
                    self.collector = Some(Collector::new(8));
                }
                if self.last_sample.elapsed() >= Duration::from_secs(1) {
                    let s = self.collector.as_mut().unwrap().sample();
                    self.live.push_back(s);
                    while self.live.len() > 300 {
                        self.live.pop_front();
                    }
                    self.last_sample = Instant::now();
                }
                ctx.request_repaint_after(Duration::from_millis(500));
                let v: Vec<Sample> = self.live.iter().cloned().collect();
                if let Some(l) = v.last() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("CPU {:.0}%", l.cpu_total));
                        ui.label(format!("RAM {:.0}% ({} / {} МБ)", l.mem_pct(), l.mem_used_mb, l.mem_total_mb));
                        if let Some(g) = l.gpu_pct { ui.label(format!("GPU {g:.0}%")); }
                        if let Some(d) = l.disk_lat_ms { ui.label(format!("Диск {d:.1} мс")); }
                        if !l.fg.is_empty() { ui.label(format!("На переднем плане: {}", l.fg)); }
                    });
                    egui::CollapsingHeader::new("Самые тяжёлые процессы").default_open(true).show(ui, |ui| {
                        for p in &l.top {
                            ui.label(format!("{:<28} CPU {:>5.1}%   {:>6} МБ", p.name, p.cpu, p.mem_mb));
                        }
                    });
                }
                egui::ScrollArea::vertical().show(ui, |ui| charts(ui, &v, &[], None, "live"));
            }
            Tab::Sessions => {
                ui.horizontal(|ui| {
                    ui.label("Запись:");
                    let cur = self.selected.map(|i| self.sessions[i].file_name().unwrap().to_string_lossy().into_owned()).unwrap_or_else(|| "выбери".into());
                    let mut pick = None;
                    egui::ComboBox::from_id_salt("sess").selected_text(cur).show_ui(ui, |ui| {
                        for (i, p) in self.sessions.iter().enumerate() {
                            if ui.selectable_label(self.selected == Some(i), p.file_name().unwrap().to_string_lossy()).clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                    if let Some(i) = pick {
                        self.load(i);
                    }
                });
                if self.selected.is_none() && !self.sessions.is_empty() {
                    self.load(0);
                }
                if self.loaded.is_empty() {
                    ui.label("Записей пока нет. Включи игровой режим, поиграй, потом вернись сюда.");
                    return;
                }
                let dur = self.loaded.last().map(|s| s.t_ms / 1000).unwrap_or(0);
                ui.label(format!("Длительность {} мин {} с, замеров {}, проблемных моментов: {}", dur / 60, dur % 60, self.loaded.len(), self.incidents.len()));
                let marks: Vec<f64> = self.incidents.iter().map(|i| i.start_ms as f64 / 1000.0).collect();
                egui::CollapsingHeader::new("Что случилось").default_open(true).show(ui, |ui| {
                    if self.incidents.is_empty() {
                        ui.label("Проблемных моментов не найдено: фризов, перегрузки CPU/RAM/диска не было.");
                    }
                    for (n, inc) in self.incidents.iter().enumerate() {
                        let t = inc.start_ms / 1000;
                        let head = format!("#{} на {}:{:02}, пик фриза {} мс", n + 1, t / 60, t % 60, inc.peak_stall_ms);
                        if ui.button(head).clicked() {
                            self.focus = Some((inc.start_ms as f64 / 1000.0 - 15.0, inc.end_ms as f64 / 1000.0 + 15.0));
                        }
                        for c in &inc.causes {
                            ui.label(format!("   • {}", cause_text(c)));
                        }
                    }
                });
                let focus = self.focus.take();
                let loaded = std::mem::take(&mut self.loaded);
                egui::ScrollArea::vertical().show(ui, |ui| charts(ui, &loaded, &marks, focus, "ses"));
                self.loaded = loaded;
            }
        });
    }
}
