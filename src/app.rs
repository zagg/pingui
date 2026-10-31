//! Application state, event loop, key bindings, exports (TUI side).

use crate::cli::Cli;
use crate::demo::DemoGen;
use crate::icmp::Pinger;
use crate::ring::Ring;
use crate::stats::Stats;

pub use crate::ring::Sample;

use anyhow::{anyhow, bail, Context, Result};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::{IpAddr, SocketAddr, SocketAddrV6, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const VIEW_NAMES: [&str; 4] = ["TABLEAU", "COURBE", "CARTE", "HISTO"];

/// Presets cycled with `t` — latency thresholds (ms) for the 4 colors.
pub const THRESHOLD_PRESETS: &[[f64; 4]] = &[
    [25.0, 75.0, 150.0, 300.0], // défaut : réseau mixte
    [5.0, 15.0, 40.0, 100.0],   // réseau local
    [50.0, 150.0, 300.0, 600.0], // WAN / VPN
    [100.0, 300.0, 600.0, 1200.0], // satellite / réseau saturé
];

const SIZE_PRESETS: [usize; 5] = [16, 56, 64, 512, 1400];

pub struct Config {
    pub host_label: String,
    pub interval: Duration,
    pub count: Option<u64>,
    pub size: usize,
    pub timeout: Duration,
    pub thresholds: [f64; 4],
    pub preset: usize,
    pub window: usize,
    pub fast: bool,
    pub seed: u64,
}

pub struct App {
    pub cfg: Config,
    pub ip: IpAddr,
    pub resolved: Option<SocketAddr>,
    pub pinger: Option<Pinger>,
    pub demo_gen: Option<DemoGen>,
    pub protocol: &'static str,
    pub stats: Stats,
    pub samples: Ring,
    pub start: Instant,
    pub next_send: Instant,
    pub pause_since: Option<Instant>,
    pub view: usize,
    pub help: bool,
    pub paused: bool,
    pub running: bool,
    pub dirty: bool,
    pub status: Option<(String, Instant)>,
    pub ctrl_c: Arc<AtomicBool>,
    pub csv: Option<(BufWriter<File>, String)>,
    pub json: Option<(BufWriter<File>, String)>,
}

impl App {
    pub fn new(cli: &Cli) -> Result<Self> {
        if cli.hosts.is_empty() && !cli.demo && !cli.gui {
            bail!("il faut au moins une destination (ou lancez avec --demo)\n\
                   Usage: pingui [OPTIONS] [DST]...\n\
                   Pour l'aide : pingui --help");
        }
        if cli.size < 16 {
            bail!("--size minimum 16 octets");
        }
        let thresholds = Cli::parse_thresholds(&cli.thresholds)?;
        let preset = THRESHOLD_PRESETS
            .iter()
            .position(|p| *p == thresholds)
            .unwrap_or(usize::MAX);
        let host_label =
            if cli.hosts.is_empty() { "démo (trafic simulé)".to_string() } else { cli.hosts.join(", ") };

        let cfg = Config {
            host_label,
            interval: cli.interval_duration(),
            count: cli.count_limit(),
            size: cli.size,
            timeout: cli.timeout_duration(),
            thresholds,
            preset,
            window: cli.window.clamp(40, 20_000),
            fast: cli.fast,
            seed: cli.seed,
        };
        let cap = cfg.count.map(|c| c as usize).unwrap_or(0).max(cfg.window).max(512);

        let mut app = App {
            cfg,
            ip: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            resolved: None,
            pinger: None,
            demo_gen: None,
            protocol: "SIMULATION",
            stats: Stats::default(),
            samples: Ring::new(cap + 64),
            start: Instant::now(),
            next_send: Instant::now(),
            pause_since: None,
            view: 1, // la vue « COURBE » = le graphique historique façon gping
            help: false,
            paused: false,
            running: true,
            dirty: true,
            status: None,
            ctrl_c: Arc::new(AtomicBool::new(false)),
            csv: None,
            json: None,
        };

        if cli.demo {
            app.demo_gen = Some(DemoGen::new(cli.seed));
        } else if let Some(host) = cli.hosts.first() {
            let dst = resolve(host, cli.six, cli.four)?;
            app.ip = dst.ip();
            app.resolved = Some(dst);
            let p = Pinger::new(dst, cli.size)?;
            app.protocol = p.protocol_label;
            app.pinger = Some(p);
        } else {
            // GUI sans destination de départ : en pause jusqu'à « Démarrer ».
            app.protocol = "en attente";
            app.paused = true;
        }

        if let Some(path) = &cli.csv {
            let w = open_export(path, "CSV", "seq,epoch_ms,latency_ms,lost,ttl,src_ip\n")?;
            app.csv = Some((w, path.clone()));
        }
        if let Some(path) = &cli.json {
            let w = open_export(path, "JSON", "")?;
            app.json = Some((w, path.clone()));
        }
        Ok(app)
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// IP affichée : « simulé » en mode --demo, sinon l'adresse résolue.
    pub fn display_ip(&self) -> String {
        if self.demo_gen.is_some() {
            "simulé".to_string()
        } else {
            self.ip.to_string()
        }
    }

    pub fn may_send(&self) -> bool {
        self.cfg.count.map_or(true, |n| self.stats.transmitted < n)
    }

    /// One loop iteration: send on schedule, drain replies, expire timeouts.
    pub fn step(&mut self) -> Result<()> {
        if self.paused {
            return Ok(());
        }
        let now = Instant::now();
        if self.demo_gen.is_some() {
            if self.may_send() && now >= self.next_send {
                let (s, dt) = self.demo_gen.as_mut().unwrap().tick();
                self.stats.transmitted += 1;
                self.commit(s);
                self.next_send = now + dt;
            }
        } else if self.pinger.is_some() {
            if self.may_send() && now >= self.next_send {
                self.pinger.as_mut().unwrap().send()?;
                self.stats.transmitted += 1;
                let mut t = self.next_send + self.cfg.interval;
                if t <= now {
                    t = now + self.cfg.interval;
                }
                self.next_send = t;
                self.dirty = true;
            }
            let replies = self.pinger.as_mut().unwrap().recv();
            for s in replies {
                self.commit(s);
            }
            let lost = self.pinger.as_mut().unwrap().expire(self.cfg.timeout);
            for seq in lost {
                self.commit(lost_sample(seq));
            }
        }
        if let Some(n) = self.cfg.count {
            if self.stats.transmitted >= n && self.stats.received + self.stats.lost >= self.stats.transmitted {
                self.running = false;
            }
        }
        Ok(())
    }

    fn commit(&mut self, s: Sample) {
        if s.lost {
            self.stats.record_lost();
        } else {
            self.stats.record_reply(s.ms);
        }
        self.export(&s);
        self.samples.push(s);
        self.dirty = true;
    }

    fn export(&mut self, s: &Sample) {
        let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or_default();
        if let Some((w, _)) = &mut self.csv {
            let _ = writeln!(
                w,
                "{},{},{},{},{},{}",
                s.seq,
                epoch,
                if s.lost { "NA".into() } else { format!("{:.3}", s.ms) },
                if s.lost { 1 } else { 0 },
                s.ttl.map(|t| t.to_string()).unwrap_or_default(),
                s.src.map(|i| i.to_string()).unwrap_or_default(),
            );
        }
        if let Some((w, _)) = &mut self.json {
            let _ = writeln!(
                w,
                "{{\"seq\":{},\"ts\":{},\"host\":\"{}\",\"lost\":{},\"latency_ms\":{},\"ttl\":{}}}",
                s.seq,
                epoch,
                escape_json(&self.cfg.host_label),
                s.lost,
                if s.lost { "null".into() } else { format!("{:.3}", s.ms) },
                s.ttl.map(|t| t.to_string()).unwrap_or_else(|| "null".into()),
            );
        }
    }

    pub fn flush_files(&mut self) -> Result<()> {
        if let Some((w, _)) = &mut self.csv {
            w.flush().context("écriture CSV")?;
        }
        if let Some((w, _)) = &mut self.json {
            w.flush().context("écriture JSON")?;
        }
        Ok(())
    }

    /// GUI : redirige la session vers un nouvel hôte (résolution + socket) et
    /// repart de zéro côté mesures.
    pub fn set_destination(&mut self, host: &str) -> Result<()> {
        let host = host.trim().to_string();
        if host.is_empty() {
            bail!("hôte vide");
        }
        let dst = resolve(&host, false, false)?;
        let p = Pinger::new(dst, self.cfg.size)?;
        self.protocol = p.protocol_label;
        self.pinger = Some(p);
        self.demo_gen = None;
        self.ip = dst.ip();
        self.resolved = Some(dst);
        self.cfg.host_label = host;
        self.reset();
        Ok(())
    }

    /// GUI : démarre/arrête un export CSV à chaud.
    pub fn start_csv(&mut self, path: &str) -> Result<()> {
        let w = open_export(path, "CSV", "seq,epoch_ms,latency_ms,lost,ttl,src_ip\n")?;
        self.csv = Some((w, path.to_string()));
        Ok(())
    }

    pub fn stop_csv(&mut self) {
        if let Some((mut w, _)) = self.csv.take() {
            let _ = w.flush();
        }
    }

    // ------------------------------------------------------------- controls --

    pub fn on_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode as K;
        match key.code {
            K::Char('q') | K::Esc | K::Char('c') => self.running = false,
            K::Char('m') | K::Tab => self.cycle_view(1),
            K::BackTab => self.cycle_view(VIEW_NAMES.len() - 1),
            K::Char('?') | K::Char('h') | K::F(1) => self.help = !self.help,
            K::Char('p') | K::Char(' ') => self.toggle_pause(),
            K::Char('s') => {
                if self.paused {
                    self.send_single();
                } else {
                    self.next_send = Instant::now();
                    self.set_status("paquet émis immédiatement");
                }
            }
            K::Char('+') | K::Char('=') => {
                self.cfg.interval = (self.cfg.interval / 2).max(Duration::from_millis(10));
                self.set_status(format!("intervalle : {} ms", self.cfg.interval.as_millis()));
            }
            K::Char('-') | K::Char('_') => {
                self.cfg.interval = (self.cfg.interval * 2).min(Duration::from_secs(60));
                self.set_status(format!("intervalle : {} ms", self.cfg.interval.as_millis()));
            }
            K::Char('t') => {
                let cur = if self.cfg.preset == usize::MAX { 0 } else { self.cfg.preset };
                let next = (cur + 1) % THRESHOLD_PRESETS.len();
                self.cfg.preset = next;
                self.cfg.thresholds = THRESHOLD_PRESETS[next];
                self.set_status(format!("seuils {} : {}–{}–{}–{} ms", next + 1, self.cfg.thresholds[0], self.cfg.thresholds[1], self.cfg.thresholds[2], self.cfg.thresholds[3]));
            }
            K::Char('e') => {
                let cur = SIZE_PRESETS.iter().position(|&s| s == self.cfg.size).unwrap_or(1);
                self.cfg.size = SIZE_PRESETS[(cur + 1) % SIZE_PRESETS.len()];
                self.set_status(format!("chargement : {} octets", self.cfg.size));
            }
            K::Char('r') => self.reset(),
            _ => {}
        }
    }

    fn cycle_view(&mut self, step: usize) {
        self.view = (self.view + step) % VIEW_NAMES.len();
        self.dirty = true;
    }

    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if self.paused {
            self.pause_since = Some(Instant::now());
            self.set_status("PAUSE — [p] reprendre · [s] paquet unique");
        } else {
            if let Some(p) = self.pause_since.take() {
                self.start += p.elapsed(); // l'horloge affichée ignore la pause
            }
            // Jeter les requêtes émises avant la pause : leurs réponses
            // arriveraient avec un RTT gonflé par le gel d'émission.
            let stale = self.pinger.as_mut().map(|p| p.expire(Duration::ZERO)).unwrap_or_default();
            for seq in stale {
                self.commit(lost_sample(seq));
            }
            let late = self.pinger.as_mut().map(|p| p.recv()).unwrap_or_default();
            for s in late {
                let _ = s; // replies to stale sequences are simply dropped
            }
            self.next_send = Instant::now();
            self.set_status("reprise");
        }
    }

    fn send_single(&mut self) {
        if let Some(gen) = self.demo_gen.as_mut() {
            let (s, _) = gen.tick();
            self.stats.transmitted += 1;
            self.commit(s);
        } else if let Some(p) = &mut self.pinger {
            if p.send().is_ok() {
                self.stats.transmitted += 1;
            }
        }
    }

    pub fn reset(&mut self) {
        self.stats = Stats::default();
        self.samples.clear();
        self.start = Instant::now();
        self.next_send = Instant::now();
        if self.demo_gen.is_some() {
            self.demo_gen = Some(DemoGen::new(self.cfg.seed));
        }
        if let Some(p) = &mut self.pinger {
            let _ = p.expire(Duration::ZERO); // oublie les requêtes en vol
        }
        self.set_status("statistiques réinitialisées");
        self.dirty = true;
    }

    // -------------------------------------------------------------- the loop --

    pub fn run_tui(&mut self) -> Result<()> {
        use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
        use ratatui::{backend::CrosstermBackend, Terminal};

        let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;
        enable_raw_mode()?;
        crossterm::execute!(term.backend_mut(), EnterAlternateScreen)?;
        let _ = term.hide_cursor();
        self.install_ctrlc();

        let result = self.event_loop(&mut term);

        let _ = term.show_cursor();
        let _ = crossterm::execute!(term.backend_mut(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
        result
    }

    fn event_loop(
        &mut self,
        term: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    ) -> Result<()> {
        use crossterm::event::{self, Event, KeyEventKind};
        let frame = Duration::from_millis(if self.cfg.fast { 30 } else { 80 });
        let mut last_draw = Instant::now() - frame;
        let mut last_flush = Instant::now();
        while self.running {
            if self.ctrl_c.load(Ordering::Relaxed) {
                self.running = false;
            }
            if event::poll(frame)? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press {
                        self.on_key(k);
                    }
                }
            }
            self.step()?;
            let expired = self
                .status
                .as_ref()
                .map(|(_, t)| t.elapsed() > Duration::from_secs(3))
                .unwrap_or(false);
            if expired {
                self.status = None;
            }
            let now = Instant::now();
            if self.dirty || now.duration_since(last_draw) >= frame {
                term.draw(|f| crate::ui::draw(f, self))?;
                last_draw = now;
                self.dirty = false;
            }
            if now.duration_since(last_flush) > Duration::from_secs(2) {
                let _ = self.flush_files();
                last_flush = now;
            }
        }
        self.flush_files()?;
        Ok(())
    }

    /// Headless line-per-packet mode (`-b`).
    pub fn run_batch(&mut self) -> Result<()> {
        use crossterm::event::{self, Event, KeyEventKind};
        let tick = Duration::from_millis(20);
        let mut printed_seq: u64 = 0;
        while self.running {
            if self.ctrl_c.load(Ordering::Relaxed) {
                self.running = false;
            }
            if event::poll(tick)? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press {
                        match k.code {
                            crossterm::event::KeyCode::Char('q')
                            | crossterm::event::KeyCode::Esc
                            | crossterm::event::KeyCode::Char('c') => self.running = false,
                            crossterm::event::KeyCode::Char('p') => self.paused = !self.paused,
                            _ => {}
                        }
                    }
                }
            }
            self.step()?;
            let new: Vec<Sample> = self.samples.iter().filter(|s| s.seq > printed_seq).cloned().collect();
            let mut out = std::io::stdout().lock();
            for s in new {
                printed_seq = s.seq;
                let src = s.src.map(|i| i.to_string()).unwrap_or_else(|| self.display_ip());
                if s.lost {
                    let _ = writeln!(
                        out,
                        "seq={seq} destination unreachable / délai dépassé ({timeout:.0} ms)",
                        seq = s.seq,
                        timeout = self.cfg.timeout.as_millis()
                    );
                } else {
                    let _ = writeln!(
                        out,
                        "{} octets from {src}: seq={seq} ttl={ttl} time={time:.2} ms",
                        self.cfg.size + 8,
                        seq = s.seq,
                        ttl = s.ttl.map(|t| t.to_string()).unwrap_or_else(|| "?".into()),
                        time = s.ms,
                    );
                }
            }
            let _ = out.flush();
        }
        self.flush_files()?;
        Ok(())
    }

    // --------------------------------------------------------------- ctrl-C --

    #[cfg(windows)]
    fn install_ctrlc(&mut self) {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        CTRL_PTR.store(Arc::into_raw(Arc::clone(&self.ctrl_c)) as *mut AtomicBool, Ordering::SeqCst);
        unsafe extern "system" fn handler(evt: u32) -> i32 {
            use windows_sys::Win32::System::Console::{CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT};
            if let Some(flag) = unsafe { CTRL_PTR.load(Ordering::SeqCst).as_ref() } {
                flag.store(true, Ordering::Relaxed);
            }
            match evt {
                // swallow bare Ctrl+C: the loop draws the summary first
                0 => 1,
                // window close / logoff / shutdown: terminate after our flag
                CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => 0,
                _ => 0,
            }
        }
        unsafe {
            SetConsoleCtrlHandler(Some(handler), 1);
        }
    }

    #[cfg(unix)]
    fn install_ctrlc(&mut self) {
        CTRL_PTR.store(Arc::into_raw(Arc::clone(&self.ctrl_c)) as *mut AtomicBool, Ordering::SeqCst);
        unsafe extern "C" fn on_int(_sig: libc::c_int) {
            if let Some(flag) = unsafe { CTRL_PTR.load(Ordering::SeqCst).as_ref() } {
                flag.store(true, Ordering::Relaxed);
            }
        }
        unsafe {
            libc::signal(libc::SIGINT, Some(on_int) as libc::sighandler_t);
        }
    }

    #[cfg(not(any(windows, unix)))]
    fn install_ctrlc(&mut self) {
        let _ = &self.ctrl_c;
    }
}

static CTRL_PTR: AtomicPtr<AtomicBool> = AtomicPtr::new(std::ptr::null_mut());

// ------------------------------------------------------------------ utils --

pub fn lost_sample(seq: u64) -> Sample {
    Sample { seq, ms: 0.0, ttl: None, lost: true, src: None, at: Instant::now() }
}

fn escape_json(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '"' | '\\' => format!("\\{c}"),
            c if (c as u32) < 0x20 => format!("\\u{:04x}", c as u32),
            c => c.to_string(),
        })
        .collect()
}

fn open_export(path: &str, kind: &str, header: &str) -> Result<BufWriter<File>> {
    if path == "-" {
        bail!("export {kind} vers stdout non disponible avec l'affichage TUI — donnez un nom de fichier");
    }
    let f = File::create(path).with_context(|| format!("création de «{path}»"))?;
    let mut w = BufWriter::new(f);
    let _ = w.write_all(header.as_bytes());
    Ok(w)
}

pub fn resolve(host: &str, six: bool, four: bool) -> Result<SocketAddr> {
    let mut addrs: Vec<SocketAddr> = Vec::new();
    if let Ok(ip) = host.parse::<IpAddr>() {
        addrs.push(SocketAddr::new(ip, 0));
    } else if let Ok(sa) = host.parse::<SocketAddr>() {
        addrs.push(sa);
    } else if let Ok(v6) = host.parse::<SocketAddrV6>() {
        addrs.push(SocketAddr::V6(v6));
    } else {
        addrs = (host, 0u16)
            .to_socket_addrs()
            .with_context(|| format!("résolution DNS de «{host}» impossible"))?
            .collect();
    }
    if addrs.is_empty() {
        return Err(anyhow!("aucune adresse trouvée pour «{host}»"));
    }
    if six {
        addrs.retain(|a| a.ip().is_ipv6());
    } else if four {
        addrs.retain(|a| a.ip().is_ipv4());
    }
    addrs.into_iter().next().ok_or_else(|| anyhow!("aucune adresse {} pour «{host}»", if six { "IPv6" } else { "IPv4" }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_literals() {
        assert_eq!(resolve("8.8.8.8", false, false).unwrap().ip().to_string(), "8.8.8.8");
        assert!(resolve("::1", true, false).unwrap().ip().is_ipv6());
        assert!(resolve("::1", false, true).is_err());
        assert!(resolve("[fe80::1%9]", true, false).is_ok());
        assert!(resolve("ce-hote-inconnu.invalid", false, false).is_err());
    }

    #[test]
    fn json_escape_quotes() {
        assert_eq!(escape_json("a\"b\\c"), "a\\\"b\\\\c");
    }

    #[test]
    fn presets_are_strictly_sorted() {
        for p in THRESHOLD_PRESETS {
            assert!(p[0] < p[1] && p[1] < p[2] && p[2] < p[3], "{p:?}");
        }
    }
}
