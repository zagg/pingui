//! Terminal UI (ratatui): header, four switchable views, footer, help overlay.

use crate::app::{App, Sample, VIEW_NAMES};
use crate::viz;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Gauge, Paragraph, Row, Sparkline, SparklineBar, Table};
use ratatui::Frame;

const LEVEL_COLORS: [Color; 4] = [Color::Green, Color::Yellow, Color::LightRed, Color::Red];
const HEAT_CELL: [(char, Color); 6] = [
    ('·', Color::DarkGray), // empty
    ('•', Color::Green),
    ('◦', Color::Yellow),
    ('●', Color::LightRed),
    ('▲', Color::Red),
    ('×', Color::Red), // packet lost
];

fn lvl_style(ms: f64, t: &[f64; 4]) -> Style {
    let l = viz::level_for(ms, t);
    Style::default().fg(LEVEL_COLORS[l])
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(f.area());
    draw_header(f, rows[0], app);
    draw_view(f, rows[1], app);
    draw_footer(f, rows[2], app);
    if app.help {
        draw_help(f, app);
    }
}

// ----------------------------------------------------------------- header --

fn draw_header(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    let st = &app.stats;
    let t = &app.cfg.thresholds;
    let title = format!(
        " pingui ▸ {} ",
        if app.cfg.host_label.is_empty() { "(sans destination)" } else { &app.cfg.host_label }
    );
    let block = Block::bordered().title(Span::styled(
        title,
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    ));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows2 = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let mut info = vec![Span::styled(format!("{} via {}", app.display_ip(), app.protocol), dim()), Span::raw("   ")];
    if app.demo_gen.is_some() {
        info.push(Span::styled("SIMULATION", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)));
        info.push(Span::raw("   "));
    }
    info.push(Span::styled(
        format!("paquets émis {} · reçus {} · time +{:.0}s", st.transmitted, st.received, app.start.elapsed().as_secs_f64()),
        Style::default(),
    ));
    f.render_widget(Paragraph::new(Line::from(info)), rows2[0]);

    let line2 = if st.received == 0 {
        Line::from(Span::styled("en attente des premières réponses…", dim()))
    } else {
        let pct = st.loss_pct();
        let s = vec![
            Span::styled(
                format!("perte {:.1}% ", pct),
                if pct < 1.0 { Style::default().fg(Color::Green) } else { Style::default().fg(Color::Red).add_modifier(Modifier::BOLD) },
            ),
            Span::raw("rtt min/avg/max/mdev "),
            Span::styled(
                format!(
                    "{}/{}/{}/{} ms",
                    viz::fmt_ms(st.cur_min),
                    viz::fmt_ms(st.mean()),
                    viz::fmt_ms(st.cur_max),
                    viz::fmt_ms(st.mdev())
                ),
                lvl_style(st.mean(), t).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  ·  jitter {} ms · p95 {} ms", viz::fmt_ms(st.jitter_ms()), viz::fmt_ms(st.percentile(0.95)))),
        ];
        if let Some(n) = app.cfg.count {
            let done = ((st.received + st.lost) as f64 / n as f64).clamp(0.0, 1.0);
            let gw = 24.min(rows2[1].width);
            let wide = ratatui::layout::Rect {
                x: rows2[1].x + rows2[1].width.saturating_sub(gw),
                y: rows2[1].y,
                width: gw,
                height: 1,
            };
            f.render_widget(
                Gauge::default()
                    .gauge_style(Style::default().fg(Color::Cyan))
                    .ratio(done)
                    .label(format!("{}/{}", st.received + st.lost, n)),
                wide,
            );
        }
        Line::from(s)
    };
    f.render_widget(Paragraph::new(line2), rows2[1]);
}

// ------------------------------------------------------------------ views --

fn draw_view(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    match app.view {
        0 => draw_table(f, area, app),
        1 => draw_curve(f, area, app),
        2 => draw_heatmap(f, area, app),
        _ => draw_distribution(f, area, app),
    }
}

/// The historical latency curve (the core gping feature, enhanced).
fn draw_curve(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    let wide = area.width >= 100;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(if wide {
            vec![Constraint::Min(0), Constraint::Length(40)]
        } else {
            vec![Constraint::Min(0)]
        })
        .split(area);

    let t = &app.cfg.thresholds;
    let hi = app.stats.cur_max.max(1.0);
    let bars: Vec<SparklineBar> = app
        .samples
        .iter()
        .map(|s| {
            let (v, sty) = if s.lost {
                (hi, Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
            } else {
                (s.ms, lvl_style(s.ms, t))
            };
            SparklineBar::from((v * 100.0) as u64).style(Some(sty))
        })
        .collect();
    let block = Block::bordered().title(format!(
        " LATENCE · {} paquets · échelle 0–{} ms ",
        bars.len(),
        viz::fmt_ms(hi)
    ));
    if bars.is_empty() {
        f.render_widget(block.title(" LATENCE — en attente de paquets "), chunks[0]);
    } else {
        let sp = Sparkline::default()
            .block(block)
            .data(bars)
            .max((hi * 100.0) as u64);
        f.render_widget(sp, chunks[0]);
    }
    if wide {
        draw_stats_panel(f, chunks[1], app);
    }
}

fn draw_table(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    let t = &app.cfg.thresholds;
    let height = area.height.saturating_sub(2) as usize;
    let rows: Vec<Row> = app
        .samples
        .iter()
        .rev()
        .take(height)
        .map(|s| {
            let cells = vec![
                Cell::from(format!("#{}", s.seq)),
                Cell::from(if s.lost { "perdu".into() } else { format!("{} ms", viz::fmt_ms(s.ms)) }),
                Cell::from(viz::bar8(s.ms, app.stats.cur_max)),
                Cell::from(s.ttl.map(|v| v.to_string()).unwrap_or_else(|| "-".into())),
                Cell::from(s.src.map(|i| i.to_string()).unwrap_or_else(|| "simulé".into())),
            ];
            let st = if s.lost { Style::default().fg(Color::Red) } else { lvl_style(s.ms, t) };
            Row::new(cells).style(st)
        })
        .collect();
    let header = Row::new(vec!["seq", "latence", "signal", "ttl", "source"]).style(dim());
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(11),
            Constraint::Length(9),
            Constraint::Length(5),
            Constraint::Min(10),
        ],
    )
    .header(header)
    .block(Block::bordered().title(" PAQUETS (les plus récents en haut) "));
    f.render_widget(table, area);
}

fn draw_heatmap(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    let rows_n = area.height.saturating_sub(2).max(1) as usize;
    let cols_n = (area.width as usize).saturating_sub(9).max(1);
    let samples: Vec<&Sample> = app.samples.iter().collect();
    let (grid, _span) = viz::heatmap(&samples, rows_n, cols_n, app.cfg.interval, &app.cfg.thresholds);
    let mut lines = Vec::with_capacity(grid.len());
    let row_span = cols_n as f64 * app.cfg.interval.as_secs_f64();
    for (r, row) in grid.iter().enumerate() {
        let label = format!(
            "t+{:>6.1}s ",
            (app.elapsed().as_secs_f64() - (rows_n - 1 - r) as f64 * row_span).max(0.0)
        );
        let mut spans = vec![Span::styled(label, dim())];
        for &cell in row {
            let (ch, col) = HEAT_CELL[cell as usize];
            let mut st = Style::default().fg(col);
            if cell == 5 {
                st = st.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(ch.to_string(), st));
        }
        lines.push(Line::from(spans));
    }
    let para = Paragraph::new(lines).block(Block::bordered().title(" CARTE DE TRAFIC · chaque cellule = 1 paquet · ▏vert ▕ok ▏jaune ▕médiocre ▏rouge ▕mauvais · × perdu "));
    f.render_widget(para, area);
}

fn draw_distribution(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    let sorted = app.stats.sorted_window();
    let bins = (area.height.saturating_sub(4)).clamp(6, 14) as usize;
    let lines: Vec<Line> = match viz::histogram(&sorted, bins) {
        None => vec![Line::from(Span::styled("pas encore de paquets", dim()))],
        Some((edges, counts)) => {
            let n = counts.iter().sum::<usize>().max(1);
            let maxc = *counts.iter().max().unwrap_or(&1).max(&1);
            let barw = (area.width as usize).saturating_sub(34);
            let mut out = Vec::new();
            for (i, &c) in counts.iter().enumerate() {
                let lo = edges[i];
                let hi = edges[i + 1];
                let mid = (lo + hi) / 2.0;
                let w = c * barw / maxc;
                out.push(Line::from(vec![
                    Span::styled(format!("{:>7}–{:<7}", viz::fmt_ms(lo), viz::fmt_ms(hi)), dim()),
                    Span::styled("█".repeat(w), lvl_style(mid, &app.cfg.thresholds)),
                    Span::styled("░".repeat(barw - w), dim()),
                    Span::styled(format!(" {:>4} ({:4.1}%)", c, c as f64 * 100.0 / n as f64), Style::default()),
                ]));
            }
            out.push(Line::raw(""));
            out.push(Line::from(Span::styled(
                format!(
                    "p50 {} · p90 {} · p95 {} · p99 {} ms — fenêtre glissante {} paquets",
                    viz::fmt_ms(app.stats.percentile(0.5)),
                    viz::fmt_ms(app.stats.percentile(0.9)),
                    viz::fmt_ms(app.stats.percentile(0.95)),
                    viz::fmt_ms(app.stats.percentile(0.99)),
                    sorted.len()
                ),
                dim(),
            )));
            out
        }
    };
    f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" DISTRIBUTION DES LATENCES ")), area);
}

fn draw_stats_panel(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    let st = &app.stats;
    let t = &app.cfg.thresholds;
    let rows: Vec<Row> = [
        ("min", format!("{} ms", viz::fmt_ms(st.cur_min)), lvl_style(st.cur_min, t)),
        ("moyenne", format!("{} ms", viz::fmt_ms(st.mean())), lvl_style(st.mean(), t)),
        ("max", format!("{} ms", viz::fmt_ms(st.cur_max)), lvl_style(st.cur_max, t)),
        ("mdev", format!("{} ms", viz::fmt_ms(st.mdev())), Style::default()),
        ("écart-type", format!("{} ms", viz::fmt_ms(st.stddev())), Style::default()),
        ("jitter", format!("{} ms", viz::fmt_ms(st.jitter_ms())), Style::default()),
        ("p50", format!("{} ms", viz::fmt_ms(st.percentile(0.5))), lvl_style(st.percentile(0.5), t)),
        ("p90", format!("{} ms", viz::fmt_ms(st.percentile(0.9))), lvl_style(st.percentile(0.9), t)),
        ("p95", format!("{} ms", viz::fmt_ms(st.percentile(0.95))), lvl_style(st.percentile(0.95), t)),
        ("p99", format!("{} ms", viz::fmt_ms(st.percentile(0.99))), lvl_style(st.percentile(0.99), t)),
        ("taille", format!("{} o", app.cfg.size), Style::default()),
        ("cadence", format!("{} ms", app.cfg.interval.as_millis()), Style::default()),
        ("timeout", format!("{} ms", app.cfg.timeout.as_millis()), Style::default()),
        ("seuils", format!("{}–{}–{}–{}", t[0], t[1], t[2], t[3]), dim()),
    ]
    .into_iter()
    .map(|(k, v, sty)| {
        Row::new(vec![
            Cell::from(Span::styled(format!("{k:<12}"), dim())),
            Cell::from(Span::styled(v, sty)),
        ])
    })
    .collect();
    let table = Table::new(rows, [Constraint::Length(13), Constraint::Min(6)]).block(Block::bordered().title(" MESURES "));
    f.render_widget(table, area);
}

// ----------------------------------------------------------------- footer --

fn draw_footer(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let views: Vec<Span> = VIEW_NAMES
        .iter()
        .enumerate()
        .flat_map(|(i, name)| {
            let st = if i == app.view {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                dim()
            };
            [Span::styled(format!("[{name}]"), st), Span::raw(" ")]
        })
        .collect();
    let keys = Span::styled(
        "m:vue p:pause s:paquet +/-:cadence t:seuils e:taille r:reset h:aide q:quitter",
        dim(),
    );
    let status = app
        .status
        .as_ref()
        .map(|(m, _)| Span::styled(format!(" · {m}"), Style::default().fg(Color::Yellow)))
        .unwrap_or_default();
    let mut spans = views;
    spans.push(Span::raw(" "));
    spans.push(keys);
    spans.push(status);
    f.render_widget(Paragraph::new(Line::from(spans)).scroll((0, 0)), area);
}

// ------------------------------------------------------------------- help --

fn draw_help(f: &mut Frame, app: &App) {
    let area = f.area();
    let w = 74.min(area.width.saturating_sub(2));
    let h = 19.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = ratatui::layout::Rect { x, y, width: w, height: h };
    f.render_widget(Clear, popup);
    let lines = vec![
        Line::from(Span::styled("pingui — ping graphique en terminal, en Rust", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::raw(""),
        Line::from("Un gping amélioré : ICMP natif (aucun processus externe),"),
        Line::from("4 vues, percentiles, jitter, export CSV/JSON."),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  m / Tab      ", Style::default().fg(Color::Yellow)),
            Span::raw("changer de vue (courbe, tableau, carte, distribution)"),
        ]),
        Line::from(vec![
            Span::styled("  p / Espace   ", Style::default().fg(Color::Yellow)),
            Span::raw("pause / reprise ; s : paquet isolé pendant la pause"),
        ]),
        Line::from(vec![
            Span::styled("  +/-        ", Style::default().fg(Color::Yellow)),
            Span::raw("doubler/réduire la cadence d'émission"),
        ]),
        Line::from(vec![
            Span::styled("  t          ", Style::default().fg(Color::Yellow)),
            Span::raw("cycler les seuils de couleur (4 préréglages)"),
        ]),
        Line::from(vec![
            Span::styled("  e / r      ", Style::default().fg(Color::Yellow)),
            Span::raw("taille du paquet / réinitialiser les mesures"),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::raw("Socket actuelle : "),
            Span::styled(app.protocol, Style::default().fg(Color::Green)),
        ]),
        Line::from(Span::styled("q ou Ctrl+C : état final en style ping classique", dim())),
    ];
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Aide ")),
        popup,
    );
}
