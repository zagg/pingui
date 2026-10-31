//! Fenêtre graphique (egui/eframe) — reprise moderne de l'ancien
//! « Graphical Ping » : hôte / taille / intervalle, Démarrer-Arrêter,
//! statistiques textuelles et grand graphique historique en barres colorées,
//! + bonus : carte de trafic, histogramme, percentiles, survol, export CSV.

use crate::app::{App, THRESHOLD_PRESETS};
use crate::ring::Sample;
use crate::viz;
use anyhow::Result;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, RichText, Stroke};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LEVEL: [Color32; 4] = [
    Color32::from_rgb(60, 200, 110),  // vert
    Color32::from_rgb(240, 190, 40),  // jaune
    Color32::from_rgb(240, 130, 40),  // orange
    Color32::from_rgb(235, 70, 60),   // rouge
];
const C_LOST: Color32 = Color32::from_rgb(190, 40, 40);
const C_GRID: Color32 = Color32::from_gray(70);
const C_TEXT: Color32 = Color32::from_gray(160);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Courbe,
    Carte,
    Histogramme,
    Paquets,
}

pub struct GuiApp {
    core: App,
    host_text: String,
    interval_ms: u32,
    size_b: u32,
    window_pkts: u32,
    tab: Tab,
    y_max: f32,
    show_avg: bool,
    csv_on: bool,
    preset_idx: usize,
    note: Option<(String, Instant)>,
}

impl GuiApp {
    fn new(core: App) -> Self {
        let preset_idx = if core.cfg.preset == usize::MAX { 1 } else { core.cfg.preset };
        let host_text = if core.cfg.host_label.contains(',') || core.cfg.host_label.starts_with("démo") {
            core.resolved.map(|a| a.ip().to_string()).unwrap_or_else(|| core.cfg.host_label.clone())
        } else {
            core.cfg.host_label.clone()
        };
        let interval_ms = (core.cfg.interval.as_millis() as u32).clamp(10, 60_000);
        let size_b = core.cfg.size as u32;
        let window_pkts = core.cfg.window.min(4000) as u32;
        let csv_on = core.csv.is_some();
        Self {
            core,
            host_text,
            interval_ms,
            size_b,
            window_pkts,
            tab: Tab::Courbe,
            y_max: 0.0,
            show_avg: true,
            csv_on,
            preset_idx,
            note: None,
        }
    }

    fn set_note(&mut self, msg: impl Into<String>) {
        self.note = Some((msg.into(), Instant::now()));
    }

    fn demo(&self) -> bool {
        self.core.demo_gen.is_some()
    }

    fn running(&self) -> bool {
        !self.core.paused
    }

    // ------------------------------------------------------------- actions --

    fn start_stop(&mut self) {
        if self.running() {
            self.core.paused = true;
            return;
        }
        if self.core.demo_gen.is_some() {
            self.core.paused = false;
            self.core.running = true;
            return;
        }
        let host = self.host_text.trim().to_string();
        if host.is_empty() {
            self.set_note("hôte vide — entrez une destination");
            return;
        }
        let same = self.core.resolved.map(|a| a.ip().to_string() == host).unwrap_or(false);
        let res = if same {
            Ok(())
        } else {
            self.core.set_destination(&host).map_err(|e| format!("{e}"))
        };
        match res {
            Ok(()) => {
                self.core.paused = false;
                self.core.running = true;
                self.y_max = 0.0;
            }
            Err(e) => self.set_note(format!("échec : {e}")),
        }
    }

    fn toggle_csv(&mut self) {
        if self.csv_on {
            let name = {
                let host: String = self
                    .host_text
                    .chars()
                    .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
                    .collect();
                let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                format!("pingui-{host}-{stamp}.csv")
            };
            match self.core.start_csv(&name) {
                Ok(()) => self.set_note(format!("export CSV : {name}")),
                Err(e) => {
                    self.csv_on = false;
                    self.set_note(format!("CSV impossible : {e}"));
                }
            }
        } else {
            self.core.stop_csv();
            self.set_note("export CSV arrêté");
        }
    }

    // ------------------------------------------------------------- panels ---

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Hôte");
            let editable = !self.demo();
            let re = ui.add(
                egui::TextEdit::singleline(&mut self.host_text)
                    .desired_width(190.0)
                    .interactive(editable),
            );
            if re.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.running() {
                self.start_stop();
            }
            let running = self.running();
            let btn = if running {
                ui.button("■ Arrêter")
            } else {
                ui.button("▶ Démarrer")
            };
            if btn.clicked() {
                self.start_stop();
            }
            ui.separator();
            ui.label("Taille");
            let sz = ui.add(egui::DragValue::new(&mut self.size_b).range(16..=60_000).suffix(" o"));
            if sz.changed() {
                self.core.cfg.size = self.size_b as usize;
                if let Some(p) = &mut self.core.pinger {
                    p.set_payload(self.size_b as usize);
                }
            }
            ui.label("Intervalle");
            let iv = ui.add(egui::DragValue::new(&mut self.interval_ms).range(10..=60_000).suffix(" ms").speed(20));
            if iv.changed() {
                self.core.cfg.interval = Duration::from_millis(self.interval_ms as u64);
            }
            ui.label("Fenêtre");
            let wn = ui.add(egui::DragValue::new(&mut self.window_pkts).range(50..=4000).speed(10).suffix(" pkt"));
            if wn.changed() {
                self.core.cfg.window = self.window_pkts as usize;
            }
            ui.separator();
            egui::ComboBox::from_id_salt("seuils")
                .selected_text(format!("Seuils {}", self.preset_idx + 1))
                .show_ui(ui, |ui| {
                    let names = ["réseau local", "mixte (défaut)", "WAN / VPN", "satellite / saturé"];
                    for (i, n) in names.iter().enumerate() {
                        let t = THRESHOLD_PRESETS[i];
                        if ui
                            .selectable_value(&mut self.preset_idx, i, format!("{n} — {}/{}/{}/{} ms", t[0], t[1], t[2], t[3]))
                            .changed()
                        {
                            self.core.cfg.preset = i;
                            self.core.cfg.thresholds = THRESHOLD_PRESETS[i];
                        }
                    }
                });
            ui.checkbox(&mut self.show_avg, "moyenne");
            if ui.checkbox(&mut self.csv_on, "CSV").changed() {
                self.toggle_csv();
            }
        });
    }

    fn stats_block(&self, ui: &mut egui::Ui) {
        let st = &self.core.stats;
        let t = &self.core.cfg.thresholds;
        let win: usize = (st.received as usize).min(self.core.cfg.window);
        egui::Grid::new("stats").num_columns(6).spacing([14.0, 3.0]).show(ui, |ui| {
            let last = self.core.samples.back();
            ui.label(RichText::new("Dernier paquet :").strong());
            match last {
                Some(s) if s.lost => {
                    ui.colored_label(C_LOST, format!("#{} (perdu)", s.seq));
                }
                Some(s) => {
                    ui.colored_label(LEVEL[viz::level_for(s.ms, t)], format!("#{} ({:.0} ms)", s.seq, s.ms));
                }
                None => {
                    ui.label("—");
                }
            }
            ui.label(RichText::new("RTT récents :").strong());
            ui.label(if win == 0 {
                "—".to_string()
            } else {
                format!("{}–{} ms", viz::fmt_ms(st.cur_min), viz::fmt_ms(st.cur_max))
            });
            ui.label(RichText::new("Récents perdus :").strong());
            ui.label(format!("{:.0} %", recent_loss_pct(&self.core.samples, win)));
            ui.end_row();

            ui.label(RichText::new("RTT total :").strong());
            ui.label(if st.received == 0 {
                "—".to_string()
            } else {
                format!(
                    "{}/{}/{} ms (moyenne/min/max)",
                    viz::fmt_ms(st.mean()),
                    viz::fmt_ms(st.cur_min),
                    viz::fmt_ms(st.cur_max)
                )
            });
            ui.label(RichText::new("Total perdu :").strong());
            ui.label(format!("{} % ({} / {})", format!("{:.1}", st.loss_pct()), st.lost, st.transmitted));
            ui.label(RichText::new("Émis / reçus :").strong());
            ui.label(format!("{} / {}", st.transmitted, st.received));
            ui.end_row();

            ui.label(RichText::new("Dispersion :").strong());
            ui.label(format!(
                "mdev {} · écart-type {} · jitter {} ms",
                viz::fmt_ms(st.mdev()),
                viz::fmt_ms(st.stddev()),
                viz::fmt_ms(st.jitter_ms())
            ));
            ui.label(RichText::new("Percentiles :").strong());
            ui.label(format!(
                "p50 {} · p90 {} · p95 {} · p99 {} ms",
                viz::fmt_ms(st.percentile(0.5)),
                viz::fmt_ms(st.percentile(0.9)),
                viz::fmt_ms(st.percentile(0.95)),
                viz::fmt_ms(st.percentile(0.99))
            ));
            ui.label(RichText::new("Durée :").strong());
            let prog = match self.core.cfg.count {
                Some(n) => format!("{:.0} s / {} pkt", self.core.elapsed().as_secs_f64(), n),
                None => format!("{:.0} s", self.core.elapsed().as_secs_f64()),
            };
            ui.label(prog);
            ui.end_row();
        });
    }

    fn tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, name) in [(Tab::Courbe, "Courbe"), (Tab::Carte, "Carte de trafic"), (Tab::Histogramme, "Histogramme"), (Tab::Paquets, "Paquets")] {
                if ui.selectable_label(self.tab == tab, name).clicked() {
                    self.tab = tab;
                }
            }
        });
    }

    // ------------------------------------------------------------ drawing ---

    fn draw_chart(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::hover());
        let full = response.rect;
        let plot = Rect::from_min_max(
            Pos2::new(full.min.x + 54.0, full.min.y + 8.0),
            Pos2::new(full.max.x - 8.0, full.max.y - 18.0),
        );
        if plot.width() < 40.0 || plot.height() < 30.0 {
            return;
        }
        match self.tab {
            Tab::Courbe => self.draw_bars(response, &painter, plot),
            Tab::Carte => self.draw_heatmap(&painter, plot),
            Tab::Histogramme => self.draw_histogram(&painter, plot),
            Tab::Paquets => {}
        }
    }

    fn draw_bars(&mut self, response: egui::Response, painter: &egui::Painter, plot: Rect) {
        let all: Vec<&Sample> = self.core.samples.iter().collect();
        let take = all.len().min(self.window_pkts as usize);
        let data = &all[all.len() - take..];
        let bottom = plot.max.y;

        let vmax = data.iter().filter(|s| !s.lost).map(|s| s.ms).fold(0.0f64, f64::max);
        let target = viz::nice_ceil(vmax.max(20.0)) as f32;
        // Lissage vertical : l'échelle grimpe tout de suite, redescend en douceur
        // (comme l'ancien Graphical Ping, sans les « sauts » de repères).
        if target > self.y_max || self.y_max <= 0.0 {
            self.y_max = target;
        } else {
            self.y_max += (target - self.y_max) * 0.15;
        }
        let scale = self.y_max;
        let t = &self.core.cfg.thresholds;

        // grille + axe Y
        for k in 0..=4 {
            let y = bottom - plot.height() * k as f32 / 4.0;
            painter.line_segment(
                [Pos2::new(plot.min.x, y), Pos2::new(plot.max.x, y)],
                Stroke::new(if k == 0 { 1.2_f32 } else { 0.6_f32 }, if k == 0 { C_TEXT } else { C_GRID }),
            );
            let v = scale * k as f32 / 4.0;
            painter.text(
                Pos2::new(plot.min.x - 6.0, y),
                Align2::RIGHT_CENTER,
                viz::fmt_ms(v as f64),
                FontId::monospace(11.0),
                C_TEXT,
            );
        }

        if data.is_empty() {
            painter.text(plot.center(), Align2::CENTER_CENTER, "en attente de paquets…", FontId::proportional(14.0), C_GRID);
            return;
        }

        let bar_w = plot.width() / data.len() as f32;
        let gap = if bar_w > 3.0 { 1.0 } else { 0.0 };

        for (i, s) in data.iter().enumerate() {
            let x0 = plot.min.x + i as f32 * bar_w + gap * 0.5;
            let x1 = x0 + bar_w - gap;
            if s.lost {
                // marque × en haut de la colonne (comme l'ancien Graphical Ping)
                let cy = plot.min.y + 10.0;
                let r = (bar_w - gap).min(9.0) * 0.45;
                painter.line_segment([Pos2::new(x0 + 1.0, cy - r), Pos2::new(x1 - 1.0, cy + r)], Stroke::new(1.6_f32, C_LOST));
                painter.line_segment([Pos2::new(x0 + 1.0, cy + r), Pos2::new(x1 - 1.0, cy - r)], Stroke::new(1.6_f32, C_LOST));
            } else {
                let h = ((s.ms / scale as f64).clamp(0.0, 1.0) * plot.height() as f64) as f32;
                let col = LEVEL[viz::level_for(s.ms, t)];
                painter.rect_filled(Rect::from_min_max(Pos2::new(x0, bottom - h), Pos2::new(x1, bottom)), 0.0, col);
            }
        }

        // courbe de moyenne mobile (11 paquets)
        if self.show_avg && data.len() > 3 {
            let mut prev: Option<Pos2> = None;
            for i in 0..data.len() {
                let a = i.saturating_sub(5);
                let b = (i + 6).min(data.len());
                let (mut acc, mut n) = (0.0f64, 0u32);
                for s in &data[a..b] {
                    if !s.lost {
                        acc += s.ms;
                        n += 1;
                    }
                }
                if n > 0 {
                    let p = Pos2::new(
                        plot.min.x + (i as f32 + 0.5) * bar_w,
                        bottom - (acc / n as f64 / scale as f64).clamp(0.0, 1.0) as f32 * plot.height(),
                    );
                    if let Some(pp) = prev {
                        painter.line_segment([pp, p], Stroke::new(1.2_f32, Color32::from_rgb(90, 190, 255)));
                    }
                    prev = Some(p);
                }
            }
        }

        // légende des seuils (bandes translucides en haut du graphe)
        let lx = plot.max.x - 150.0;
        for (i, &thv) in t.iter().enumerate() {
            painter.rect_filled(Rect::from_min_max(Pos2::new(lx + i as f32 * 38.0, plot.min.y - 4.0), Pos2::new(lx + i as f32 * 38.0 + 12.0, plot.min.y + 8.0)), 2.0, LEVEL[i]);
            painter.text(Pos2::new(lx + i as f32 * 38.0 + 16.0, plot.min.y + 2.0), Align2::LEFT_CENTER, format!("{}", thv as u32), FontId::monospace(10.5), C_TEXT);
        }

        // axe X : temps
        let span_secs = (data.last().unwrap().at - data.first().unwrap().at).as_secs_f64();
        painter.text(Pos2::new(plot.min.x, plot.max.y + 2.0), Align2::LEFT_TOP, format!("t+{:.0} s", (self.core.elapsed().as_secs_f64() - span_secs).max(0.0)), FontId::monospace(10.5), C_GRID);
        painter.text(Pos2::new(plot.max.x, plot.max.y + 2.0), Align2::RIGHT_TOP, format!("t+{:.0} s", self.core.elapsed().as_secs_f64()), FontId::monospace(10.5), C_TEXT);

        // survol → tooltip au curseur
        let hover = response.hover_pos().filter(|p| plot.contains(*p)).map(|p| (((p.x - plot.min.x) / bar_w) as usize).min(data.len() - 1));
        if let Some(idx) = hover {
            let s = data[idx];
            let secs = (s.at - self.core.start).as_secs_f64();
            let th = *t;
            response.on_hover_ui_at_pointer(move |ui| {
                ui.monospace(format!("#{}", s.seq));
                if s.lost {
                    ui.colored_label(C_LOST, "paquet perdu");
                } else {
                    ui.colored_label(LEVEL[viz::level_for(s.ms, &th)], format!("{:.2} ms", s.ms));
                }
                if let Some(ttl) = s.ttl {
                    ui.label(format!("ttl {ttl}"));
                }
                ui.label(format!("t+{secs:.2} s"));
            });
        }
    }

    fn draw_heatmap(&self, painter: &egui::Painter, plot: Rect) {
        let t = &self.core.cfg.thresholds;
        let cell = 10.0f32;
        let cols = (plot.width() / cell) as usize;
        let rows = (plot.height() / cell) as usize;
        if cols == 0 || rows == 0 {
            return;
        }
        let all: Vec<&Sample> = self.core.samples.iter().collect();
        let (grid, _) = viz::heatmap(&all, rows, cols, self.core.cfg.interval, t);
        for (r, row) in grid.iter().enumerate() {
            for (c, &v) in row.iter().enumerate() {
                let color = match v {
                    0 => Color32::from_gray(38),
                    1..=4 => LEVEL[v as usize - 1],
                    5 => C_LOST,
                    _ => C_GRID,
                };
                let x = plot.min.x + c as f32 * cell;
                let y = plot.min.y + r as f32 * cell;
                painter.rect_filled(Rect::from_min_max(Pos2::new(x + 1.0, y + 1.0), Pos2::new(x + cell - 1.0, y + cell - 1.0)), 2.0, color);
            }
        }
        painter.text(Pos2::new(plot.min.x, plot.max.y + 4.0), Align2::LEFT_TOP, "plus ancien ↑ · plus récent ↓ — ×=perdu, teinte = niveau de latence", FontId::proportional(10.5), C_GRID);
    }

    fn draw_histogram(&self, painter: &egui::Painter, plot: Rect) {
        let t = &self.core.cfg.thresholds;
        let sorted = self.core.stats.sorted_window();
        let bins = (plot.width() / 34.0).clamp(8.0, 24.0) as usize;
        let Some((edges, counts)) = viz::histogram(&sorted, bins) else {
            painter.text(plot.center(), Align2::CENTER_CENTER, "pas encore de paquets", FontId::proportional(14.0), C_GRID);
            return;
        };
        let maxc = *counts.iter().max().unwrap_or(&1).max(&1) as f32;
        let bw = plot.width() / counts.len() as f32;
        let total = counts.iter().sum::<usize>().max(1);
        for (i, &c) in counts.iter().enumerate() {
            let mid = (edges[i] + edges[i + 1]) * 0.5;
            let h = (c as f32 / maxc) * plot.height();
            let x = plot.min.x + i as f32 * bw;
            painter.rect_filled(Rect::from_min_max(Pos2::new(x + 1.0, plot.max.y - h), Pos2::new(x + bw - 1.0, plot.max.y)), 2.0, LEVEL[viz::level_for(mid, t)]);
            painter.text(Pos2::new(x + bw * 0.5, plot.max.y - h - 12.0), Align2::CENTER_BOTTOM, format!("{c}"), FontId::monospace(10.5), C_TEXT);
            if i % 2 == 0 {
                painter.text(Pos2::new(x + bw * 0.5, plot.max.y + 4.0), Align2::CENTER_TOP, viz::fmt_ms(edges[i]), FontId::monospace(9.5), C_GRID);
            }
        }
        painter.text(Pos2::new(plot.min.x, plot.min.y - 4.0), Align2::LEFT_BOTTOM, format!("distribution sur {} paquets — {total} mesurés", sorted.len()), FontId::proportional(11.0), C_GRID);
    }

    fn draw_table(&mut self, ui: &mut egui::Ui) {
        let t = self.core.cfg.thresholds;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("packets").num_columns(5).spacing([16.0, 2.0]).show(ui, |ui| {
                ui.strong("#");
                ui.strong("latence");
                ui.strong("barre");
                ui.strong("ttl");
                ui.strong("source");
                ui.end_row();
                let n = self.core.samples.len();
                let skip = n.saturating_sub(1000);
                for s in self.core.samples.iter().skip(skip).rev() {
                    ui.label(format!("#{}", s.seq));
                    if s.lost {
                        ui.colored_label(C_LOST, "PERDU");
                    } else {
                        ui.colored_label(LEVEL[viz::level_for(s.ms, &t)], format!("{:>8.2} ms", s.ms));
                    }
                    ui.label(viz::bar8(s.ms, self.core.stats.cur_max));
                    ui.label(s.ttl.map(|v| v.to_string()).unwrap_or_else(|| "—".into()));
                    ui.label(s.src.map(|i| i.to_string()).unwrap_or_else(|| "—".into()));
                    ui.end_row();
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        let st = &self.core.stats;
        ui.horizontal(|ui| {
            let proto = if self.demo() { "simulation".into() } else { self.core.protocol.to_string() };
            ui.label(RichText::new(format!("{} · [{}] · ICMP natif", self.core.display_ip(), proto)).weak().size(11.5));
            if let Some((msg, when)) = &self.note {
                if when.elapsed() < Duration::from_secs(4) {
                    ui.colored_label(Color32::from_rgb(240, 200, 90), format!("⚠ {msg}"));
                }
            }
            if !self.core.running {
                ui.colored_label(C_TEXT, "session terminée");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if st.received > 0 {
                    let last = self.core.samples.back().filter(|s| !s.lost);
                    if let Some(s) = last {
                        ui.colored_label(LEVEL[viz::level_for(s.ms, &self.core.cfg.thresholds)], format!("dernier : {:.0} ms", s.ms));
                    } else if st.lost > 0 {
                        ui.colored_label(C_LOST, "dernier : PERDU");
                    }
                }
                let (loss_col, txt) = if st.loss_pct() < 1.0 {
                    (Color32::from_rgb(60, 200, 110), "0% perdu".to_string())
                } else {
                    (C_LOST, format!("{:.1}% perdus", st.loss_pct()))
                };
                if st.transmitted > 0 {
                    ui.colored_label(loss_col, format!("  {txt}"));
                }
            });
        });
    }
}

impl Drop for GuiApp {
    fn drop(&mut self) {
        let _ = self.core.flush_files();
    }
}

impl eframe::App for GuiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            let _ = ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // pompe le moteur ICMP avant de dessiner
        if let Err(e) = self.core.step() {
            self.core.paused = true;
            self.set_note(format!("erreur réseau : {e}"));
        }
        ctx.request_repaint_after(Duration::from_millis(100));

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(2.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.add_space(2.0);
            self.status_bar(ui);
            ui.add_space(2.0);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            self.stats_block(ui);
            ui.add_space(4.0);
            self.tabs(ui);
            ui.add_space(4.0);
            if self.tab == Tab::Paquets {
                self.draw_table(ui);
            } else {
                self.draw_chart(ui);
            }
        });
    }
}

/// Fenêtre récente : % de perte sur les `n` derniers échantillons.
fn recent_loss_pct(samples: &crate::ring::Ring, n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    let all: Vec<&Sample> = samples.iter().collect();
    let k = all.len().min(n);
    let lost = all[all.len() - k..].iter().filter(|s| s.lost).count();
    lost as f64 * 100.0 / k as f64
}

pub fn run(core: App) -> Result<()> {
    let title = format!("pingui — {}", core.cfg.host_label);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([920.0, 620.0])
            .with_min_inner_size([560.0, 380.0])
            .with_title(title),
        ..Default::default()
    };
    eframe::run_native(
        "pingui",
        options,
        Box::new(move |_cc| Ok(Box::new(GuiApp::new(core)))),
    )
    .map_err(|e| anyhow::anyhow!("impossible d'ouvrir la fenêtre : {e}"))?;
    Ok(())
}
