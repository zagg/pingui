//! pingui — un « gping » en mieux, écrit en Rust.
//! Trois modes : interface fenêtrée (--gui, feature par défaut), TUI terminal
//! (le mode par défaut), et texte classique (-b/--batch).

mod app;
mod cli;
mod demo;
mod icmp;
mod ring;
mod stats;
mod ui;
mod viz;
#[cfg(feature = "gui")]
mod gui;

use anyhow::Result;
use clap::Parser;

fn main() {
    let opts = cli::Cli::parse();
    let code = match real(&opts) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("erreur: {e}");
            for cause in e.chain().skip(1) {
                eprintln!("  └─ {cause}");
            }
            1
        }
    };
    std::process::exit(code);
}

fn real(opts: &cli::Cli) -> Result<i32> {
    let app = app::App::new(opts)?;
    // pingui.exe (compilé avec la feature « gui ») : fenêtre graphique par
    // défaut. pingui-cli.exe (sans gui) : TUI par défaut. --tui/--batch
    // forcent le terminal dans les deux cas.
    #[cfg(feature = "gui")]
    if !(opts.tui || opts.batch) {
        return gui::run(app).map(|_| 0);
    }
    #[cfg(not(feature = "gui"))]
    if opts.gui {
        return Err(anyhow::anyhow!(
            "ce binaire (pingui-cli) est compilé sans interface graphique — utilisez pingui.exe, ou le mode TUI (-T)"
        ));
    }
    let mut app = app;
    if opts.batch {
        app.run_batch()?;
    } else {
        app.run_tui()?;
    }
    print_summary(&app);
    Ok(if app.stats.received > 0 { 0 } else { 1 })
}

fn print_summary(app: &app::App) {
    use std::io::Write;
    let st = &app.stats;
    println!();
    println!("--- pingui : statistiques pour {} [{}] ---", app.cfg.host_label, app.display_ip());
    println!(
        "{} paquets émis, {} reçus, {} perdus, {:.1}% de perte (délai d'expiration {} ms), durée {:.1} s",
        st.transmitted,
        st.received,
        st.lost,
        st.loss_pct(),
        app.cfg.timeout.as_millis(),
        app.elapsed().as_secs_f64(),
    );
    if st.received > 0 {
        println!(
            "rtt min/avg/max/mdev = {}/{}/{}/{} ms",
            viz::fmt_ms(st.cur_min),
            viz::fmt_ms(st.mean()),
            viz::fmt_ms(st.cur_max),
            viz::fmt_ms(st.mdev()),
        );
        println!(
            "    écart-type {} ms · jitter {} ms",
            viz::fmt_ms(st.stddev()),
            viz::fmt_ms(st.jitter_ms()),
        );
        println!(
            "    percentiles: p50 {} · p90 {} · p95 {} · p99 {} ms",
            viz::fmt_ms(st.percentile(0.5)),
            viz::fmt_ms(st.percentile(0.9)),
            viz::fmt_ms(st.percentile(0.95)),
            viz::fmt_ms(st.percentile(0.99)),
        );
        let width = crossterm::terminal::size()
            .map(|(w, _)| (w as usize).saturating_sub(9).clamp(20, 100))
            .unwrap_or(66);
        println!();
        println!("    courbe — {} derniers paquets (× = perdu) :", app.samples.len());
        println!("      {}", viz::sparkline_cells(&app.samples.spark_values(), width, '×'));
    }
    if let Some((_, path)) = &app.csv {
        println!("export CSV écrit : {path}");
    }
    if let Some((_, path)) = &app.json {
        println!("export JSON écrit : {path}");
    }
    let _ = std::io::stdout().flush();
}
