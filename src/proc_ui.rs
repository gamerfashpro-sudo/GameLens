//! Вкладка «Программы»: что запущено, что из этого можно отключить, кнопка «Отключить» справа
//! и галочка «закрывать при входе в игровой режим».

use crate::app::{card, load_color, stat_card, ACCENT, BAD, CARD, MUTED, OK, WARN};
use crate::config::Config;
use crate::procs::{self, Class, Proc, Stopped};
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use std::time::{Duration, Instant};
use sysinfo::System;

#[derive(PartialEq, Clone, Copy)]
enum Filter {
    /// то, что можно отключить
    Closable,
    All,
}

pub struct ProcView {
    sys: System,
    cores: usize,
    rows: Vec<Proc>,
    last: Instant,
    filter: Filter,
    search: String,
    /// имя программы, для которой ждём подтверждения закрытия
    confirm: Option<String>,
    msg: String,
    prev_stopped: Vec<Stopped>,
}

impl ProcView {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        let cores = sys.cpus().len().max(1);
        let mut v = ProcView {
            sys,
            cores,
            rows: vec![],
            last: Instant::now() - Duration::from_secs(10),
            filter: if std::env::var("GAMELENS_FILTER").as_deref() == Ok("all") { Filter::All } else { Filter::Closable },
            search: String::new(),
            confirm: None,
            msg: String::new(),
            prev_stopped: vec![],
        };
        v.refresh();
        v
    }

    fn refresh(&mut self) {
        procs::refresh(&mut self.sys);
        self.rows = procs::list(&self.sys, self.cores);
        self.prev_stopped = procs::load_stopped();
        self.last = Instant::now();
    }

    fn kill(&mut self, name: &str) {
        let (n, _) = procs::kill_by_name(&self.sys, name, crate::perf::foreground_pid());
        self.msg = if n > 0 { format!("Закрыто: {name} ({n} шт.)") } else { format!("Не удалось закрыть {name}: возможно, он уже закрыт или защищён.") };
        self.confirm = None;
        std::thread::sleep(Duration::from_millis(150));
        self.refresh();
    }

    pub fn show(&mut self, ui: &mut egui::Ui, cfg: &mut Config) {
        if self.last.elapsed() > Duration::from_secs(2) {
            self.refresh();
        }
        ui.ctx().request_repaint_after(Duration::from_secs(2));
        let before = cfg.clone();

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            self.header(ui, cfg);

            // что можно освободить
            let closable: Vec<&Proc> = self.rows.iter().filter(|p| matches!(p.class, Class::Safe | Class::Caution)).collect();
            let mem: u64 = closable.iter().map(|p| p.mem_mb).sum();
            let cpu: f32 = closable.iter().map(|p| p.cpu).sum::<f32>() + 0.0; // + 0.0 убирает «-0.0» у пустой суммы
            let marked = self.rows.iter().filter(|p| is_marked(cfg, &p.name)).count();
            let w = (ui.available_width() - 3.0 * 10.0) / 4.0;
            ui.horizontal(|ui| {
                stat_card(ui, w, "Можно отключить", &format!("{} программ", closable.len()), ACCENT);
                stat_card(ui, w, "Займут памяти", &format!("{} МБ", mem), if mem > 2000 { WARN } else { OK });
                stat_card(ui, w, "Грузят процессор", &format!("{:.1}%", cpu), load_color(cpu * 3.0));
                stat_card(ui, w, "Закроются в игровом режиме", &format!("{} шт.", marked), if marked > 0 { OK } else { MUTED });
            });

            // фильтр и поиск
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.filter, Filter::Closable, RichText::new("Можно отключить").size(15.0));
                ui.selectable_value(&mut self.filter, Filter::All, RichText::new("Все программы").size(15.0));
                ui.add_space(12.0);
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Поиск по названию").desired_width(220.0));
                if !self.msg.is_empty() {
                    ui.add_space(12.0);
                    ui.label(RichText::new(&self.msg).color(MUTED));
                }
            });

            let q = self.search.trim().to_lowercase();
            let rows: Vec<Proc> = self
                .rows
                .iter()
                .filter(|p| match self.filter {
                    Filter::Closable => matches!(p.class, Class::Safe | Class::Caution) || is_marked(cfg, &p.name),
                    Filter::All => true,
                })
                .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q) || p.known.is_some_and(|k| k.title.to_lowercase().contains(&q)))
                .cloned()
                .collect();

            if rows.is_empty() {
                card(ui, |ui| {
                    ui.label(RichText::new("Ничего не найдено").strong());
                    ui.label(RichText::new("Здесь появятся запущенные фоновые программы, которые можно закрыть на время игры.").color(MUTED));
                });
            }
            let mut to_kill: Option<String> = None;
            for p in &rows {
                self.row(ui, p, cfg, &mut to_kill);
            }
            if let Some(n) = to_kill {
                self.kill(&n);
            }

            // отмеченные, но сейчас не запущенные
            let absent: Vec<String> = cfg.auto_stop.iter().filter(|n| !self.rows.iter().any(|p| procs::norm(&p.name) == procs::norm(n))).cloned().collect();
            if !absent.is_empty() {
                card(ui, |ui| {
                    ui.label(RichText::new("Отмечены, но сейчас не запущены").strong());
                    ui.label(RichText::new("Они закроются в игровом режиме, если к тому моменту будут запущены. Сними галочку, чтобы убрать из списка.").small().color(MUTED));
                    for n in absent {
                        let mut on = true;
                        if ui.checkbox(&mut on, &n).changed() && !on {
                            cfg.auto_stop.retain(|x| procs::norm(x) != procs::norm(&n));
                        }
                    }
                });
            }
        });

        if *cfg != before {
            cfg.save();
        }
    }

    fn header(&mut self, ui: &mut egui::Ui, cfg: &mut Config) {
        card(ui, |ui| {
            ui.label(RichText::new("Программы и процессы").size(20.0).strong());
            ui.label(RichText::new("Отметь галочкой программы, которые тебе не нужны во время игры: игровой режим закроет их сам при входе. Кнопка «Отключить» справа закрывает программу прямо сейчас. Системные процессы и защита от читов закрыть нельзя, они помечены «Система».").color(MUTED));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut cfg.auto_stop_on, "Закрывать отмеченные при входе в игровой режим");
                ui.checkbox(&mut cfg.restart_after, "Запускать их обратно после игры");
            });
            ui.horizontal(|ui| {
                if ui.button("Отметить все безопасные").on_hover_text("Только те, что помечены зелёным «Можно отключить»").clicked() {
                    for p in self.rows.iter().filter(|p| p.class == Class::Safe) {
                        if !is_marked(cfg, &p.name) {
                            cfg.auto_stop.push(p.name.clone());
                        }
                    }
                }
                if ui.button("Снять все отметки").clicked() {
                    cfg.auto_stop.clear();
                }
            });
            if !self.prev_stopped.is_empty() {
                let names: Vec<String> = self.prev_stopped.iter().map(|s| format!("{} ({})", s.name, s.count)).collect();
                ui.add_space(2.0);
                ui.label(RichText::new(format!("В прошлый раз игровой режим закрыл: {}", names.join(", "))).small().color(MUTED));
            }
        });
    }

    fn row(&mut self, ui: &mut egui::Ui, p: &Proc, cfg: &mut Config, to_kill: &mut Option<String>) {
        let (color, tag) = class_style(p.class);
        egui::Frame::new()
            .fill(CARD)
            .corner_radius(10)
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.25)))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // галочка автозакрытия (у защищённых её нет)
                    ui.allocate_ui_with_layout(Vec2::new(26.0, 24.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(26.0);
                        if p.class == Class::Never {
                            ui.label(""); // у системных процессов галочки нет
                        } else {
                            let mut on = is_marked(cfg, &p.name);
                            if ui.checkbox(&mut on, "").on_hover_text("Закрывать при входе в игровой режим").changed() {
                                if on {
                                    cfg.auto_stop.push(p.name.clone());
                                } else {
                                    cfg.auto_stop.retain(|x| procs::norm(x) != procs::norm(&p.name));
                                }
                            }
                        }
                    });

                    // три колонки фиксированной ширины: название, цифры, кнопка. Название берёт всё остальное место
                    let (metrics_w, button_w) = (140.0, 112.0);
                    let left_w = (ui.available_width() - metrics_w - button_w - 3.0 * ui.spacing().item_spacing.x).max(180.0);
                    ui.allocate_ui_with_layout(Vec2::new(left_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_min_width(left_w);
                        ui.set_max_width(left_w);
                        ui.horizontal(|ui| {
                            let title = p.known.map(|k| k.title).unwrap_or(p.name.as_str());
                            ui.label(RichText::new(title).strong().size(15.0));
                            badge(ui, tag, color);
                        });
                        let sub = match p.known {
                            Some(k) => format!("{}  ·  {}", p.name, k.why),
                            None if p.class == Class::Never => format!("{}  ·  системный процесс или защищён правами", p.name),
                            None => format!("{}  ·  неизвестная программа, закрывай, только если знаешь, что это", p.name),
                        };
                        ui.add(egui::Label::new(RichText::new(sub).small().color(MUTED)).wrap());
                    });
                    ui.allocate_ui_with_layout(Vec2::new(metrics_w, 0.0), egui::Layout::top_down(egui::Align::Max), |ui| {
                        ui.set_min_width(metrics_w);
                        ui.label(RichText::new(format!("{} МБ", p.mem_mb)).strong().size(15.0));
                        ui.label(RichText::new(format!("CPU {:.1}%  ·  {} шт.", p.cpu, p.count)).small().color(load_color(p.cpu * 3.0).gamma_multiply(0.9)));
                    });
                    ui.allocate_ui_with_layout(Vec2::new(button_w, 0.0), egui::Layout::top_down(egui::Align::Max), |ui| {
                        ui.set_min_width(button_w);
                        self.action(ui, p, to_kill);
                    });
                });
                // подтверждение для небезопасных
                if self.confirm.as_deref() == Some(p.name.as_str()) {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let warn = match p.class {
                            Class::Unknown => "Неизвестная программа: данные в ней могут не сохраниться.",
                            _ => "Программа закроется сразу, несохранённое пропадёт.",
                        };
                        ui.label(RichText::new(warn).color(WARN));
                        if ui.button(RichText::new("Да, закрыть").color(BAD)).clicked() {
                            *to_kill = Some(p.name.clone());
                        }
                        if ui.button("Отмена").clicked() {
                            self.confirm = None;
                        }
                    });
                }
            });
        ui.add_space(2.0);
    }

    fn action(&mut self, ui: &mut egui::Ui, p: &Proc, to_kill: &mut Option<String>) {
        if p.class == Class::Never {
            ui.label(RichText::new("нельзя").small().color(MUTED));
            return;
        }
        let btn = egui::Button::new(RichText::new("Отключить").color(Color32::WHITE).strong())
            .fill(if p.class == Class::Safe { Color32::from_rgb(52, 140, 100) } else { Color32::from_rgb(70, 78, 96) })
            .min_size(Vec2::new(104.0, 28.0));
        if ui.add(btn).clicked() {
            if p.class == Class::Safe {
                *to_kill = Some(p.name.clone());
            } else {
                self.confirm = Some(p.name.clone());
            }
        }
    }
}

fn is_marked(cfg: &Config, name: &str) -> bool {
    let n = procs::norm(name);
    cfg.auto_stop.iter().any(|x| procs::norm(x) == n)
}

fn class_style(c: Class) -> (Color32, &'static str) {
    match c {
        Class::Safe => (OK, "Можно отключить"),
        Class::Caution => (WARN, "Осторожно"),
        Class::Unknown => (MUTED, "Неизвестно"),
        Class::Never => (Color32::from_rgb(110, 118, 135), "Система"),
    }
}

fn badge(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().strong().color(color));
        });
}
