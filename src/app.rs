use crate::advice::{self, Severity};
use crate::analyze;
use crate::collector::Collector;
use crate::model::{Incident, Sample};
use crate::config::{Config, Corner};
use crate::fps::{self, TraceStatus};
use crate::{paths, store, update};
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use egui_plot::{Line, Plot, PlotPoints, VLine};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CARD: Color32 = Color32::from_rgb(30, 34, 42);
const PANEL: Color32 = Color32::from_rgb(20, 22, 27);
const ACCENT: Color32 = Color32::from_rgb(86, 168, 255);
const ORANGE: Color32 = Color32::from_rgb(255, 160, 60);
const OK: Color32 = Color32::from_rgb(76, 195, 138);
const WARN: Color32 = Color32::from_rgb(242, 184, 75);
const BAD: Color32 = Color32::from_rgb(255, 90, 54);
const MUTED: Color32 = Color32::from_rgb(140, 148, 162);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Live,
    Sessions,
    Settings,
}

struct SysInfo {
    cpu: String,
    cores: String,
    ram: String,
    os: String,
}

pub struct App {
    styled: bool,
    tab: Tab,
    recording: bool,
    /// stop-файл отправлен, ждём, пока фоновый процесс реально завершится
    stopping: bool,
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
    info: SysInfo,
    confirm_delete: bool,
    note: String,
    cfg: Config,
    rights: Option<TraceStatus>,
    rights_msg: Arc<Mutex<String>>,
}

fn sys_info() -> SysInfo {
    let mut s = sysinfo::System::new();
    s.refresh_cpu_all();
    s.refresh_memory();
    SysInfo {
        cpu: s.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_else(|| "н/д".into()),
        cores: format!("{} физ. / {} потоков", s.physical_core_count().unwrap_or(0), s.cpus().len()),
        ram: format!("{:.1} ГБ", s.total_memory() as f64 / 1_073_741_824.0),
        os: sysinfo::System::long_os_version().unwrap_or_else(|| "н/д".into()),
    }
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
        let tab = match std::env::var("GAMELENS_TAB").as_deref() {
            Ok("sessions") => Tab::Sessions,
            Ok("settings") => Tab::Settings,
            _ => Tab::Live,
        };
        let mut a = App {
            styled: false,
            tab,
            recording: paths::recorder_running(),
            stopping: false,
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
            info: sys_info(),
            confirm_delete: false,
            note: String::new(),
            cfg: Config::load(),
            rights: None,
            rights_msg: Arc::new(Mutex::new(String::new())),
        };
        a.refresh_sessions();
        a
    }

    fn style(&mut self, ctx: &egui::Context) {
        if self.styled {
            return;
        }
        self.styled = true;
        ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = Vec2::new(10.0, 9.0);
            s.spacing.button_padding = Vec2::new(12.0, 6.0);
            s.visuals.panel_fill = PANEL;
            s.visuals.window_fill = PANEL;
            s.visuals.selection.bg_fill = Color32::from_rgb(40, 96, 158);
            for (ts, size) in [
                (egui::TextStyle::Body, 15.0),
                (egui::TextStyle::Button, 15.0),
                (egui::TextStyle::Heading, 24.0),
                (egui::TextStyle::Small, 12.5),
            ] {
                if let Some(f) = s.text_styles.get_mut(&ts) {
                    f.size = size;
                }
            }
        });
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
                // окно закрывается полностью — в памяти остаётся только крошечный рекордер
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn stop_game_mode(&mut self) {
        let _ = std::fs::create_dir_all(paths::data_dir());
        let _ = std::fs::write(paths::stop_file(), b"1");
        self.stopping = true;
        self.collector = None;
    }

    // ───────────────────────── вкладка «Сейчас» ─────────────────────────
    fn tab_live(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.recording || self.stopping {
            card(ui, |ui| {
                ui.heading(if self.stopping { "Останавливаю запись…" } else { "Идёт фоновая запись" });
                ui.label("Игровой режим включён: окно ничего не измеряет и не мешает игре, записью занимается маленький фоновый процесс.");
                ui.label(RichText::new("Чтобы закончить, сними галочку «Игровой режим» вверху. Потом открой вкладку «Записи»: там будет разбор.").color(MUTED));
            });
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
        let Some(l) = v.last().cloned() else { return };

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // статус
            let recent_stall = v.iter().rev().take(10).map(|s| s.stall_ms).max().unwrap_or(0);
            let (color, title, text) = if recent_stall >= analyze::STALL_MS {
                (BAD, "Система зависала", format!("За последние секунды система «стояла» до {recent_stall} мс. Это заметно как рывок в игре."))
            } else if l.cpu_total >= analyze::CPU_SAT || l.mem_pct() >= analyze::MEM_PRESSURE {
                (WARN, "Высокая нагрузка", format!("Процессор {:.0}%, память {:.0}%. Игра может начать лагать.", l.cpu_total, l.mem_pct()))
            } else {
                (OK, "Всё работает ровно", format!("Нагрузка в норме: процессор {:.0}%, память {:.0}%, зависаний нет.", l.cpu_total, l.mem_pct()))
            };
            banner(ui, color, title, &text);

            // датчики
            let w = (ui.available_width() - 3.0 * 10.0) / 4.0;
            ui.horizontal(|ui| {
                gauge_card(ui, w, "Процессор", l.cpu_total, format!("{:.0}%", l.cpu_total), load_color(l.cpu_total), &format!("ядро макс. {}%", l.max_core()));
                gauge_card(ui, w, "Память", l.mem_pct(), format!("{:.0}%", l.mem_pct()), load_color(l.mem_pct()), &format!("{} из {} МБ", l.mem_used_mb, l.mem_total_mb));
                match l.gpu_pct {
                    Some(g) => gauge_card(ui, w, "Видеокарта", g, format!("{g:.0}%"), load_color(g), "3D-нагрузка"),
                    None => gauge_card(ui, w, "Видеокарта", 0.0, "н/д".into(), MUTED, "счётчик недоступен"),
                }
                match l.disk_lat_ms {
                    Some(d) => gauge_card(ui, w, "Диск", (d / 50.0 * 100.0).min(100.0), format!("{d:.1} мс"), if d >= analyze::DISK_LAT_MS { BAD } else if d >= 20.0 { WARN } else { OK }, "задержка ответа"),
                    None => gauge_card(ui, w, "Диск", 0.0, "н/д".into(), MUTED, "счётчик недоступен"),
                }
            });

            // ядра + процессы + система
            ui.columns(2, |cols| {
                card(&mut cols[0], |ui| {
                    ui.label(RichText::new("Самые тяжёлые процессы").strong());
                    ui.label(RichText::new("доля от всего процессора").small().color(MUTED));
                    for p in &l.top {
                        ui.horizontal(|ui| {
                            let mut name = p.name.clone();
                            if name.chars().count() > 26 {
                                name = name.chars().take(25).collect::<String>() + "…";
                            }
                            let name_text = if p.name == l.fg { RichText::new(format!("{name}  (активное окно)")).color(ACCENT) } else { RichText::new(name) };
                            ui.allocate_ui_with_layout(Vec2::new(210.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| ui.label(name_text));
                            ui.add(egui::ProgressBar::new((p.cpu / 100.0).clamp(0.0, 1.0)).desired_width(150.0).text(format!("{:.1}%", p.cpu)));
                            ui.label(RichText::new(format!("{} МБ", p.mem_mb)).color(MUTED));
                        });
                    }
                    if !l.fg.is_empty() {
                        ui.label(RichText::new(format!("Окно на переднем плане: {}", l.fg)).small().color(ACCENT));
                    }
                });
                card(&mut cols[1], |ui| {
                    ui.label(RichText::new("Ядра процессора").strong());
                    egui::Grid::new("cores").num_columns(8).spacing([8.0, 6.0]).show(ui, |ui| {
                        for (i, c) in l.cpu_cores.iter().enumerate() {
                            ui.label(RichText::new(format!("{i}")).small().color(MUTED));
                            ui.add(egui::ProgressBar::new(*c as f32 / 100.0).desired_width(64.0).desired_height(10.0).fill(load_color(*c as f32)));
                            if (i + 1) % 4 == 0 {
                                ui.end_row();
                            }
                        }
                    });
                    ui.separator();
                    ui.label(RichText::new("Компьютер").strong());
                    info_row(ui, "Процессор", &self.info.cpu);
                    info_row(ui, "Ядра", &self.info.cores);
                    info_row(ui, "Память", &self.info.ram);
                    info_row(ui, "Система", &self.info.os);
                    if let Some(f) = l.cpu_freq_pct {
                        info_row(ui, "Частота CPU", &if f > 100.5 { format!("{f:.0}% от базовой (турбо)") } else { format!("{f:.0}% от базовой") });
                    }
                    if let Some(d) = l.dpc_pct {
                        info_row(ui, "Драйверы (DPC)", &format!("{d:.1}% времени"));
                    }
                });
            });

            card(ui, |ui| {
                ui.label(RichText::new("Как пользоваться").strong());
                ui.label("1. Поставь галочку «Игровой режим» вверху: окно закроется, а запись пойдёт в фоне.");
                ui.label("2. Играй. Если заметишь фризы или просадки, запомни время.");
                ui.label("3. Запусти GameLens ещё раз (ярлык на рабочем столе, см. «Настройки»), сними галочку и открой «Записи».");
            });

            charts_grid(ui, &v, &[], None, "live");
        });
    }

    // ───────────────────────── вкладка «Записи» ─────────────────────────
    fn tab_sessions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Запись:");
            let cur = self.selected.and_then(|i| self.sessions.get(i)).map(|p| session_label(p)).unwrap_or_else(|| "выбери".into());
            let mut pick = None;
            egui::ComboBox::from_id_salt("sess").width(320.0).selected_text(cur).show_ui(ui, |ui| {
                for (i, p) in self.sessions.iter().enumerate() {
                    if ui.selectable_label(self.selected == Some(i), session_label(p)).clicked() {
                        pick = Some(i);
                    }
                }
            });
            if let Some(i) = pick {
                self.load(i);
            }
            if ui.button("Обновить список").clicked() {
                self.refresh_sessions();
            }
        });
        if self.selected.is_none() && !self.sessions.is_empty() {
            self.load(0);
        }
        if self.loaded.is_empty() {
            card(ui, |ui| {
                ui.heading("Записей пока нет");
                ui.label("Включи «Игровой режим», поиграй, потом вернись сюда: здесь появится разбор фризов и советы, что сделать.");
            });
            return;
        }

        let sum = advice::summarize(&self.loaded, &self.incidents);
        let dur = self.loaded.last().map(|s| s.t_ms / 1000).unwrap_or(0);
        let marks: Vec<f64> = self.incidents.iter().map(|i| i.start_ms as f64 / 1000.0).collect();
        let focus = self.focus.take();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // итог
            let (vcolor, _) = score_color(sum.score);
            banner(ui, vcolor, &format!("Оценка плавности: {} из 100", sum.score), sum.verdict);

            let w = (ui.available_width() - 4.0 * 10.0) / 5.0;
            ui.horizontal(|ui| {
                stat_card(ui, w, "Длительность", &format!("{} мин {:02} с", dur / 60, dur % 60), ACCENT);
                stat_card(ui, w, "Проблемных моментов", &self.incidents.len().to_string(), if self.incidents.is_empty() { OK } else { WARN });
                stat_card(ui, w, "Худшее зависание", &format!("{} мс", sum.worst_stall_ms), if sum.worst_stall_ms >= 100 { BAD } else if sum.worst_stall_ms >= 50 { WARN } else { OK });
                stat_card(ui, w, "Средняя загрузка CPU", &format!("{:.0}%", sum.avg_cpu), load_color(sum.avg_cpu));
                stat_card(ui, w, "Пик памяти", &format!("{:.0}%", sum.peak_mem_pct), load_color(sum.peak_mem_pct));
            });

            // главные выводы
            let findings = advice::top_findings(&self.incidents);
            if findings.is_empty() {
                card(ui, |ui| {
                    ui.label(RichText::new("Проблем не найдено").strong().color(OK));
                    ui.label("За время записи не было зависаний системы, перегрузки процессора, памяти или диска. Если фризы в игре всё равно были, причина, скорее всего, внутри самой игры (оптимизация, шейдеры) или в сети.");
                });
            } else {
                card(ui, |ui| {
                    ui.label(RichText::new("Главные выводы: что чинить в первую очередь").strong().size(18.0));
                    for (n, (a, count)) in findings.iter().take(3).enumerate() {
                        ui.add_space(4.0);
                        ui.label(RichText::new(format!("{}. {}   (встречается: {})", n + 1, a.title, count)).strong().color(ACCENT));
                        for f in a.fixes.iter().take(3) {
                            ui.label(format!("      • {f}"));
                        }
                    }
                    ui.label(RichText::new("Подробные объяснения для каждого момента ниже.").small().color(MUTED));
                });
            }

            // инциденты
            if !self.incidents.is_empty() {
                ui.label(RichText::new("Что случилось: момент за моментом").strong().size(18.0));
            }
            for (n, inc) in self.incidents.iter().enumerate() {
                incident_card(ui, n, inc, &self.loaded, &mut self.focus);
            }

            ui.add_space(6.0);
            ui.label(RichText::new("Графики записи").strong().size(18.0));
            ui.label(RichText::new("Красные вертикальные линии — начало проблемных моментов. Колесо мыши — масштаб, перетаскивание — сдвиг.").small().color(MUTED));
            charts_grid(ui, &self.loaded, &marks, focus, "ses");
        });
    }

    // ───────────────────────── вкладка «Настройки» ─────────────────────────
    fn tab_settings(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card(ui, |ui| {
                ui.label(RichText::new("Быстрый возврат к программе").strong());
                ui.label("Игровой режим закрывает окно. Чтобы вернуться, запусти GameLens ещё раз: проще всего через ярлык на рабочем столе.");
                if ui.button("Создать ярлык на рабочем столе").clicked() {
                    self.note = create_shortcut();
                }
                if !self.note.is_empty() {
                    ui.label(RichText::new(&self.note).color(MUTED));
                }
            });
            card(ui, |ui| {
                ui.label(RichText::new("FPS поверх игры").strong());
                ui.label(RichText::new("Работает в игровом режиме. Белая мягкая надпись в углу экрана над игрой в оконном режиме или в «окне без рамки». В настоящем полноэкранном режиме Windows не позволяет рисовать поверх игры.").small().color(MUTED));
                let before = self.cfg.clone();
                ui.checkbox(&mut self.cfg.fps_overlay, "Показывать FPS в игровом режиме");
                ui.horizontal(|ui| {
                    ui.label("Угол экрана:");
                    for c in [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight] {
                        ui.selectable_value(&mut self.cfg.corner, c, c.label());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Яркость надписи:");
                    ui.add(egui::Slider::new(&mut self.cfg.opacity, 30..=100).suffix("%"));
                });
                ui.horizontal(|ui| {
                    ui.label("Размер:");
                    ui.add(egui::Slider::new(&mut self.cfg.size, 14..=40).suffix(" px"));
                });
                if self.cfg != before {
                    self.cfg.save();
                }
                ui.label(RichText::new("Изменения применяются при следующем включении игрового режима.").small().color(MUTED));
                ui.separator();
                ui.label(RichText::new("Права на измерение FPS").strong());
                ui.label(RichText::new("Windows даёт читать события показа кадров только администратору или членам группы «Performance Log Users». Достаточно выдать права один раз, потом нужно выйти из Windows и зайти снова.").small().color(MUTED));
                ui.horizontal(|ui| {
                    if ui.button("Проверить права").clicked() {
                        self.rights = Some(fps::probe_rights());
                    }
                    if ui.button("Выдать права (запрос администратора)").clicked() {
                        grant_fps_rights(self.rights_msg.clone());
                    }
                });
                match self.rights {
                    Some(TraceStatus::Ok) => { ui.colored_label(OK, "Права есть: FPS будет измеряться."); }
                    Some(TraceStatus::NoRights) => { ui.colored_label(WARN, "Прав пока нет. Нажми «Выдать права», подтверди запрос Windows, потом перезайди в Windows."); }
                    Some(TraceStatus::Unsupported) => { ui.colored_label(MUTED, "Измерение FPS работает только в Windows."); }
                    Some(TraceStatus::Error(c)) => { ui.colored_label(BAD, format!("Не удалось проверить (код {c}).")); }
                    None => {}
                }
                let m = self.rights_msg.lock().map(|m| m.clone()).unwrap_or_default();
                if !m.is_empty() {
                    ui.label(RichText::new(m).color(MUTED));
                }
            });
            card(ui, |ui| {
                ui.label(RichText::new("Записи").strong());
                ui.label(format!("Папка: {}", paths::sessions_dir().display()));
                ui.label(format!("Сохранено записей: {} (хранятся последние 30)", self.sessions.len()));
                ui.horizontal(|ui| {
                    if ui.button("Открыть папку").clicked() {
                        let _ = std::fs::create_dir_all(paths::sessions_dir());
                        open_folder(&paths::sessions_dir());
                    }
                    ui.checkbox(&mut self.confirm_delete, "Я уверен");
                    if ui.add_enabled(self.confirm_delete, egui::Button::new("Удалить все записи")).clicked() {
                        for p in &self.sessions {
                            let _ = std::fs::remove_file(p);
                        }
                        self.confirm_delete = false;
                        self.selected = None;
                        self.loaded.clear();
                        self.incidents.clear();
                        self.refresh_sessions();
                    }
                });
            });
            card(ui, |ui| {
                ui.label(RichText::new("Обновления").strong());
                ui.label(format!("Установлена версия {}", update::VERSION));
                let found = self.upd.lock().unwrap().clone();
                match found {
                    Some(f) => {
                        ui.label(RichText::new(format!("Доступна новая версия {}", f.version)).color(OK));
                        if ui.button("Обновить и перезапустить").clicked() {
                            match update::apply(&f) {
                                Ok(()) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                                Err(e) => self.upd_msg = e,
                            }
                        }
                        ui.colored_label(BAD, &self.upd_msg);
                    }
                    None => {
                        ui.label(RichText::new("Это последняя версия (или нет соединения с GitHub).").color(MUTED));
                    }
                }
            });
            card(ui, |ui| {
                ui.label(RichText::new("Как это работает и чего не умеет").strong());
                ui.label("• В игровом режиме работает отдельный маленький процесс без окна: раз в 2 секунды он записывает загрузку процессора, памяти, диска, видеокарты, FPS и список самых тяжёлых программ.");
                ui.label("• Фризы ловятся специальным таймером: если система «стояла» дольше 50 мс, это попадает в запись.");
                ui.label("• FPS игры читается из системных событий показа кадров (ETW), без внедрения в игру. Для этого нужны права (см. выше). Если прав нет, всё остальное работает, просто без FPS.");
                ui.label("• Причины фризов ищутся на стороне системы. Если причина внутри самой игры, программа честно напишет, что не нашла объяснения.");
                ui.label("• Температуры процессора и видеокарты не измеряются: для этого нужны драйверы, которые GameLens намеренно не ставит.");
            });
        });
    }
}

// ───────────────────────── вспомогательные виджеты ─────────────────────────

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(10)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn banner(ui: &mut egui::Ui, color: Color32, title: &str, text: &str) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).size(20.0).strong().color(color));
            ui.label(text);
        });
}

fn load_color(pct: f32) -> Color32 {
    if pct >= 90.0 {
        BAD
    } else if pct >= 70.0 {
        WARN
    } else {
        OK
    }
}

fn score_color(score: u32) -> (Color32, &'static str) {
    match score {
        90..=100 => (OK, "ok"),
        70..=89 => (ACCENT, "norm"),
        40..=69 => (WARN, "warn"),
        _ => (BAD, "bad"),
    }
}

fn info_row(ui: &mut egui::Ui, k: &str, v: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(120.0, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| ui.label(RichText::new(k).color(MUTED)));
        ui.label(v);
    });
}

fn stat_card(ui: &mut egui::Ui, w: f32, title: &str, value: &str, color: Color32) {
    egui::Frame::new().fill(CARD).corner_radius(10).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(w - 24.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).small().color(MUTED));
            ui.label(RichText::new(value).size(22.0).strong().color(color));
        });
    });
}

/// Круглый датчик: серая дуга-фон и цветная дуга со значением
fn gauge_card(ui: &mut egui::Ui, w: f32, title: &str, pct: f32, big: String, color: Color32, sub: &str) {
    egui::Frame::new().fill(CARD).corner_radius(10).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(w - 24.0);
        ui.vertical(|ui| {
        ui.label(RichText::new(title).strong());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 120.0), egui::Sense::hover());
        let c = rect.center_top() + Vec2::new(0.0, 56.0);
        let r = 46.0;
        let arc = |from: f32, to: f32| -> Vec<egui::Pos2> {
            let steps = 48;
            (0..=steps)
                .map(|i| {
                    let a = (from + (to - from) * i as f32 / steps as f32).to_radians();
                    c + Vec2::new(a.cos(), a.sin()) * r
                })
                .collect()
        };
        let p = ui.painter();
        p.add(egui::Shape::line(arc(135.0, 405.0), Stroke::new(9.0, Color32::from_rgb(48, 54, 66))));
        let end = 135.0 + 270.0 * (pct / 100.0).clamp(0.0, 1.0);
        if pct > 0.5 {
            p.add(egui::Shape::line(arc(135.0, end), Stroke::new(9.0, color)));
        }
        p.text(c, egui::Align2::CENTER_CENTER, big, egui::FontId::proportional(21.0), Color32::WHITE);
        ui.label(RichText::new(sub).small().color(MUTED));
        });
    });
}

fn incident_card(ui: &mut egui::Ui, n: usize, inc: &Incident, samples: &[Sample], focus: &mut Option<(f64, f64)>) {
    let sev = advice::severity(inc);
    let color = match sev {
        Severity::Heavy => BAD,
        Severity::Noticeable => WARN,
        Severity::Minor => ACCENT,
    };
    let t = inc.start_ms / 1000;
    let what = if inc.peak_stall_ms >= analyze::STALL_MS {
        format!("зависание до {} мс (≈{} кадров при 60 FPS)", inc.peak_stall_ms, advice::frames_lost(inc.peak_stall_ms))
    } else {
        "перегрузка системы".to_string()
    };
    let head = RichText::new(format!("#{}   {}:{:02}   ·   {}   ·   {}", n + 1, t / 60, t % 60, sev.label(), what)).strong().color(color);
    egui::CollapsingHeader::new(head).id_salt(("inc", n)).default_open(n < 2).show(ui, |ui| {
        card(ui, |ui| {
            // что происходило
            let w: Vec<&Sample> = samples.iter().filter(|s| s.t_ms >= inc.start_ms && s.t_ms <= inc.end_ms).collect();
            if let Some(p) = w.iter().max_by_key(|s| (s.stall_ms, (s.cpu_total * 10.0) as u32)) {
                ui.label(RichText::new("Что происходило в этот момент").strong());
                let mut line = format!("Процессор {:.0}% (самое загруженное ядро {}%), память {:.0}%", p.cpu_total, p.max_core(), p.mem_pct());
                if let Some(g) = p.gpu_pct {
                    line += &format!(", видеокарта {g:.0}%");
                }
                if let Some(d) = p.disk_lat_ms {
                    line += &format!(", диск {d:.0} мс");
                }
                ui.label(line);
                if !p.fg.is_empty() {
                    ui.label(format!("На переднем плане: {}", p.fg));
                }
                let tops: Vec<String> = p.top.iter().filter(|x| x.name != p.fg).take(3).map(|x| format!("{} ({:.0}%)", x.name, x.cpu)).collect();
                if !tops.is_empty() {
                    ui.label(format!("Фоновые процессы: {}", tops.join(", ")));
                }
                ui.add_space(4.0);
            }
            if ui.button("Показать на графиках").clicked() {
                *focus = Some((inc.start_ms as f64 / 1000.0 - 15.0, inc.end_ms as f64 / 1000.0 + 15.0));
            }
            for c in &inc.causes {
                let a = advice::advise(c);
                ui.separator();
                ui.label(RichText::new(&a.title).strong().size(17.0).color(ACCENT));
                ui.label(RichText::new("Почему это вызвало фриз").small().color(MUTED));
                ui.label(&a.why);
                ui.label(RichText::new("Что сделать").small().color(MUTED));
                for f in &a.fixes {
                    ui.label(format!("   • {f}"));
                }
            }
        });
    });
}

fn session_label(p: &std::path::Path) -> String {
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let secs = stem.trim_start_matches("session-").parse::<u64>().unwrap_or(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let ago = now.saturating_sub(secs);
    let when = if secs == 0 {
        stem
    } else if ago < 90 {
        "только что".to_string()
    } else if ago < 3600 {
        format!("{} мин назад", ago / 60)
    } else if ago < 86_400 {
        format!("{} ч {} мин назад", ago / 3600, (ago % 3600) / 60)
    } else {
        format!("{} дн назад", ago / 86_400)
    };
    when
}

fn create_shortcut() -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let Ok(exe) = std::env::current_exe() else { return "Не удалось определить путь к программе.".into() };
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut([Environment]::GetFolderPath('Desktop')+'\\GameLens.lnk'); $s.TargetPath='{}'; $s.Save()",
            exe.display().to_string().replace('\'', "''")
        );
        let ok = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .creation_flags(0x0800_0000)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        return if ok { "Ярлык GameLens создан на рабочем столе.".into() } else { "Не получилось создать ярлык. Создай его вручную: ПКМ на gamelens.exe, затем «Отправить» и «Рабочий стол».".into() };
    }
    #[cfg(not(windows))]
    {
        "Ярлыки создаются только в Windows.".into()
    }
}

/// Добавляет текущего пользователя в группу «Performance Log Users» (SID S-1-5-32-559, не зависит от языка Windows).
/// Windows покажет запрос администратора. Работает в отдельном потоке, чтобы окно не зависало.
fn grant_fps_rights(msg: Arc<Mutex<String>>) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let user = format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default());
        let inner = format!("Add-LocalGroupMember -SID S-1-5-32-559 -Member '{}'", user.replace('\'', "''"));
        let outer = format!(
            "Start-Process powershell -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile','-Command','{}'",
            inner.replace('\'', "''")
        );
        if let Ok(mut m) = msg.lock() {
            *m = "Ожидаю подтверждения запроса Windows…".into();
        }
        std::thread::spawn(move || {
            let ok = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &outer]).creation_flags(0x0800_0000).status().map(|s| s.success()).unwrap_or(false);
            if let Ok(mut m) = msg.lock() {
                *m = if ok {
                    "Готово. Теперь выйди из Windows и зайди снова (или перезагрузи ПК), потом нажми «Проверить права».".into()
                } else {
                    "Запрос отклонён или не удался. Права не выданы.".into()
                };
            }
        });
    }
    #[cfg(not(windows))]
    {
        if let Ok(mut m) = msg.lock() {
            *m = "Доступно только в Windows.".into();
        }
    }
}

fn open_folder(p: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}

// ───────────────────────── графики ─────────────────────────

struct ChartSpec {
    id: &'static str,
    title: &'static str,
    hint: &'static str,
    y_max: Option<f64>,
    lines: Vec<(&'static str, Vec<[f64; 2]>, Color32)>,
}

fn series(samples: &[Sample], f: impl Fn(&Sample) -> Option<f32>) -> Vec<[f64; 2]> {
    samples.iter().filter_map(|s| f(s).map(|v| [s.t_ms as f64 / 1000.0, v as f64])).collect()
}

fn build_charts(s: &[Sample]) -> Vec<ChartSpec> {
    let mut v = Vec::new();
    if s.iter().any(|x| x.fps.is_some()) {
        v.push(ChartSpec { id: "fps", title: "FPS игры", hint: "Провалы FPS сравни с остальными графиками: рядом с провалом обычно видна причина (процессор, память, диск).", y_max: None, lines: vec![("FPS", series(s, |x| x.fps), Color32::from_rgb(235, 238, 245))] });
    }
    v.extend(vec![
        ChartSpec { id: "cpu", title: "Процессор, %", hint: "Если линия долго у 100% — процессор не справляется. Нижняя линия — самое загруженное ядро.", y_max: Some(100.0), lines: vec![("Общая загрузка", series(s, |x| Some(x.cpu_total)), ACCENT), ("Самое загруженное ядро", series(s, |x| Some(x.max_core() as f32)), ORANGE)] },
        ChartSpec { id: "ram", title: "Оперативная память, %", hint: "Выше ~90% Windows начинает выгружать данные на диск — отсюда фризы.", y_max: Some(100.0), lines: vec![("Память", series(s, |x| Some(x.mem_pct())), OK)] },
    ]);
    if s.iter().any(|x| x.gpu_pct.is_some()) {
        v.push(ChartSpec { id: "gpu", title: "Видеокарта (3D), %", hint: "Около 100% при свободном процессоре — упор в видеокарту.", y_max: Some(100.0), lines: vec![("Видеокарта", series(s, |x| x.gpu_pct), Color32::from_rgb(190, 130, 255))] });
    }
    if s.iter().any(|x| x.disk_lat_ms.is_some()) {
        v.push(ChartSpec { id: "disk", title: "Диск: время ответа, мс", hint: "Больше 50 мс — диск тормозит, игра ждёт данные.", y_max: None, lines: vec![("Задержка диска", series(s, |x| x.disk_lat_ms), ORANGE)] });
    }
    if s.iter().any(|x| x.cpu_freq_pct.is_some()) {
        v.push(ChartSpec { id: "freq", title: "Частота процессора, % от базовой", hint: "Выше 100% — процессор разогнался сам (турбо), это нормально. Провал ниже 100% при высокой нагрузке — перегрев или лимит питания.", y_max: None, lines: vec![("Частота", series(s, |x| x.cpu_freq_pct), ACCENT)] });
    }
    if s.iter().any(|x| x.dpc_pct.is_some()) {
        v.push(ChartSpec { id: "dpc", title: "Драйверы (DPC), % времени", hint: "Больше 10–15% — какой-то драйвер перегружает процессор.", y_max: Some(20.0), lines: vec![("DPC", series(s, |x| x.dpc_pct), BAD)] });
    }
    v.push(ChartSpec { id: "stall", title: "Зависания системы, мс", hint: "Каждый пик — момент, когда система «стояла». Заметно уже от 50 мс.", y_max: Some(100.0), lines: vec![("Зависание", series(s, |x| Some(x.stall_ms as f32)), BAD)] });
    v
}

#[allow(deprecated)]
fn charts_grid(ui: &mut egui::Ui, s: &[Sample], marks: &[f64], focus: Option<(f64, f64)>, key: &'static str) {
    let specs = build_charts(s);
    let two = ui.available_width() > 1000.0;
    let cols = if two { 2 } else { 1 };
    ui.columns(cols, |c| {
        for (i, spec) in specs.iter().enumerate() {
            let ui = &mut c[i % cols];
            card(ui, |ui| {
                ui.label(RichText::new(spec.title).strong());
                ui.label(RichText::new(spec.hint).small().color(MUTED));
                let mut p = Plot::new((key, spec.id))
                    .height(210.0)
                    .allow_scroll(false)
                    .include_y(0.0)
                    .legend(egui_plot::Legend::default())
                    .link_axis(egui::Id::new(key), [true, false])
                    .link_cursor(egui::Id::new(key), [true, false]);
                if let Some(m) = spec.y_max {
                    p = p.include_y(m);
                }
                p.show(ui, |pui| {
                    if let Some((a, b)) = focus {
                        pui.set_plot_bounds_x(a..=b);
                    }
                    for (name, pts, color) in &spec.lines {
                        pui.line(Line::new(*name, PlotPoints::from(pts.clone())).color(*color).width(2.0));
                    }
                    for &m in marks {
                        pui.vline(VLine::new("", m).color(Color32::from_rgb(255, 90, 54)).width(1.5));
                    }
                });
            });
        }
    });
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        self.style(ctx);
        if self.stopping {
            // пока рекордер не завершился, состояние не перечитываем: иначе галочка включится обратно
            if paths::recorder_running() {
                ctx.request_repaint_after(Duration::from_millis(150));
            } else {
                self.stopping = false;
                self.recording = false;
                self.refresh_sessions();
                self.selected = None;
            }
        } else if self.last_rec_check.elapsed() > Duration::from_secs(2) {
            self.recording = paths::recorder_running();
            self.last_rec_check = Instant::now();
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("GameLens").size(22.0).strong().color(ACCENT));
                ui.add_space(12.0);
                for (t, name) in [(Tab::Live, "Сейчас"), (Tab::Sessions, "Записи"), (Tab::Settings, "Настройки")] {
                    if ui.selectable_label(self.tab == t, RichText::new(name).size(16.0)).clicked() {
                        self.tab = t;
                        if t == Tab::Sessions {
                            self.refresh_sessions();
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut gm = self.recording || self.stopping;
                    let resp = ui.add_enabled(!self.stopping, egui::Checkbox::new(&mut gm, RichText::new("Игровой режим").strong()));
                    if resp.on_hover_text("Окно закроется, запись пойдёт в фоне без нагрузки на игру").changed() {
                        if gm {
                            self.start_game_mode(ctx);
                        } else {
                            self.stop_game_mode();
                        }
                    }
                    if self.stopping {
                        ui.label(RichText::new("Останавливаю запись…").color(WARN));
                    }
                    let has_upd = self.upd.lock().unwrap().is_some();
                    if has_upd {
                        if ui.button(RichText::new("Есть обновление").color(OK)).clicked() {
                            self.tab = Tab::Settings;
                        }
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Live => self.tab_live(ui, ctx),
            Tab::Sessions => self.tab_sessions(ui),
            Tab::Settings => self.tab_settings(ui),
        });
    }
}
