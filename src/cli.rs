//! Command-line interface.

use clap::Parser;
use std::num::NonZeroU32;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "pingui",
    version,
    about = "Ping graphique en terminal — ICMP natif, multi-vues, percentiles, jitter, export CSV/JSON",
    after_help = "EXEMPLES:\n  \
        pingui example.com                  TUI temps réel sur example.com\n  \
        pingui 8.8.8.8 -i 200ms -n 50       50 paquets toutes les 200 ms\n  \
        pingui 1.1.1.1 --csv out.csv        export CSV en continu\n  \
        pingui ::1 -6                       IPv6 (fonctionne sans privilèges)\n  \
        pingui --demo                       trafic simulé, aucun droit requis\n  \
        pingui example.com -b               mode texte classique, sans TUI\n\n\
        CLAVES (TUI):\n  \
        m vue (tableau/courbe/carte/histogramme) · p pause · s paquet unique\n  \
        +/- cadence · t seuils · e taille paquet · r reset · q/Ctrl-C quitte"
)]
pub struct Cli {
    /// Hôte(s) ou adresse(s) IP à interroger (plusieurs = bascule avec Tab… on ping la première)
    #[arg(value_name = "DST")]
    pub hosts: Vec<String>,

    /// Nombre de paquets à envoyer (défaut : infini, Ctrl-C pour l'état final)
    #[arg(short = 'n', long, value_name = "N")]
    pub count: Option<NonZeroU32>,

    /// Intervalle entre les paquets, ex. 1s, 200ms (borné à 10ms–60s)
    #[arg(short = 'i', long, default_value = "1s", value_name = "DUREE")]
    pub interval: humantime::Duration,

    /// Délai avant qu'une réponse sans suite soit marquée « perdue »
    #[arg(long, default_value = "2s", value_name = "DUREE")]
    pub timeout: humantime::Duration,

    /// Taille du chargement ICMP en octets (min 16, max 65 000)
    #[arg(short = 's', long, default_value = "56", value_name = "N")]
    pub size: usize,

    /// Forcer IPv4 (ICMPv4)
    #[arg(short = '4', long, conflicts_with = "six")]
    pub four: bool,

    /// Forcer IPv6 (ICMPv6)
    #[arg(short = '6', long, conflicts_with = "four")]
    pub six: bool,

    /// Seuils de latence (ms) pour les 4 couleurs, ex. 25,75,150,300
    #[arg(long, default_value = "25,75,150,300", value_name = "A,B,C,D")]
    pub thresholds: String,

    /// Rafraîchissement accéléré de l'affichage (30 ms au lieu de 80 ms)
    #[arg(long)]
    pub fast: bool,

    /// Échantillons affichés sur la courbe et la carte thermique
    #[arg(long, default_value = "240", value_name = "N")]
    pub window: usize,

    /// Exporter chaque échantillon en CSV (« - » = stdout, non dispo avec la TUI)
    #[arg(long, value_name = "FICHIER")]
    pub csv: Option<String>,

    /// Exporter chaque échantillon en JSON Lines (« - » = stdout)
    #[arg(long, value_name = "FICHIER")]
    pub json: Option<String>,

    /// Mode simulation : trafic synthétique reproductible (aucun privilège requis)
    #[arg(long)]
    pub demo: bool,

    /// Graine du générateur de simulation
    #[arg(long, default_value = "1", value_name = "N")]
    pub seed: u64,

    /// Mode texte sans TUI (une ligne par paquet, comme ping)
    #[arg(short = 'b', long)]
    pub batch: bool,

    /// Interface fenêtrée — l'ancien « Graphical Ping » en mieux (nécessite la
    /// feature de build « gui », activée par défaut)
    #[arg(short = 'G', long)]
    pub gui: bool,
}

impl Cli {
    pub fn interval_duration(&self) -> Duration {
        Duration::from_secs_f64(self.interval.as_secs_f64().clamp(0.01, 60.0))
    }

    pub fn timeout_duration(&self) -> Duration {
        Duration::from_secs_f64(self.timeout.as_secs_f64().clamp(0.05, 600.0))
    }

    pub fn count_limit(&self) -> Option<u64> {
        self.count.map(|c| c.get() as u64)
    }

    /// Parse `a,b,c,d` into strictly usable ascending thresholds.
    pub fn parse_thresholds(s: &str) -> anyhow::Result<[f64; 4]> {
        let parts: Result<Vec<f64>, _> = s.split(',').map(|p| p.trim().parse::<f64>()).collect();
        let v = parts?;
        if v.len() != 4 {
            anyhow::bail!("4 seuils attendus (A,B,C,D), reçu {} valeurs", v.len());
        }
        for w in v.windows(2) {
            if !(w[0] < w[1]) {
                anyhow::bail!("les seuils doivent être croissants: {s}");
            }
        }
        Ok([v[0], v[1], v[2], v[3]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_ok_and_errors() {
        assert_eq!(Cli::parse_thresholds("25,75,150,300").unwrap(), [25.0, 75.0, 150.0, 300.0]);
        assert_eq!(Cli::parse_thresholds(" 10 , 20 ,30, 40 ").unwrap(), [10.0, 20.0, 30.0, 40.0]);
        assert!(Cli::parse_thresholds("1,2,3").is_err());
        assert!(Cli::parse_thresholds("1,5,3,9").is_err());
        assert!(Cli::parse_thresholds("a,b,c,d").is_err());
    }

    #[test]
    fn clap_defaults_parse() {
        let c = Cli::try_parse_from(["pingui", "example.com"]).unwrap();
        assert_eq!(c.hosts, vec!["example.com"]);
        assert_eq!(c.interval.as_secs_f64(), 1.0);
        assert!(c.count_limit().is_none());
        let c = Cli::try_parse_from(["pingui", "a", "b", "-n", "5", "-i", "200ms", "-s", "1400", "--demo"]).unwrap();
        assert_eq!(c.hosts.len(), 2);
        assert_eq!(c.count_limit(), Some(5));
        assert_eq!(c.interval_duration(), Duration::from_millis(200));
        assert_eq!(c.size, 1400);
        assert!(c.demo);
        assert!(Cli::try_parse_from(["pingui", "a", "-4", "-6"]).is_err());
        assert!(Cli::try_parse_from(["pingui", "a", "-n", "0"]).is_err());
    }
}
