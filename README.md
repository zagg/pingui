# pingui

Un « gping » en mieux, écrit en **Rust**. Comme l'ancien *Graphical Ping*, vous
voyez l'historique de latence en direct — mais avec ICMP natif (aucun processus
`ping` externe), trois modes d'affichage, percentiles, jitter, carte de trafic,
histogramme de distribution, export CSV/JSON et un mode simulation pour tout
explorer sans aucun privilège réseau.

```
        ┌───────────────────────────────────────────────┐
        │ pingui ▸ 8.8.8.8                              │
        │ 8.8.8.8 via raw socket ICMPv4   t+12s  émis 12│
        │ perte 0% rtt min/avg/max/mdev 29/32/35/1.4 ms │
        │                                               │
        │  ▃▂▄▂▅▃▂▄▂▁▃▅▇▅▃▂▄▂▁▂▃▅▃▂▁▂▄▂▃▅▇▅▃▂▄▂▁▂▃  100..0│
        └───────────────────────────────────────────────┘
```

## Fonctionnalités

- **ICMP natif** maison (checksum RFC 1071, horodatage dans la charge utile)
  IPv4/IPv6, `SOCK_RAW` avec repli automatique `SOCK_DGRAM` sans privilège
  (Linux `ping_group_range`, macOS, etc.).
- **3 modes** :
  - interface **fenêtrée** (egui/eframe) — la reprise moderne du *Graphical
    Ping* : hôte / taille / intervalle / Démarrer-Arrêter, stats textuelles et
    grand graphique en barres colorées ;
  - **TUI** terminal (défaut) — courbe historique + 3 autres vues ;
  - **--batch** texte à la `ping` (pilotable en script).
- **4 vues** : courbe sparkline colorée, tableau des paquets, carte de trafic
  (heat-map par paquet), histogramme de distribution.
- **Mesures riches** : min/avg/max/mdev, écart-type, **jitter** (RFC 3550),
  percentiles **p50/p90/p95/p99** sur fenêtre glissante.
- **Contrôle temps réel** : pause, paquet isolé, cadence et taille ajustables
  au clavier, 4 jeux de seuils de couleur pré-enregistrés, reset.
- **Export** CSV et JSON Lines (fichier ou continu).
- **--demo** : trafic synthétique déterministe (seed) — pertes, pics et jitter
  sans privilège ni réseau.

## Construction

```bash
cargo build --release            # TUI + fenêtre + batch (feature « gui » par défaut)
cargo build --no-default-features   # sans eframe (TUI/batch seuls, build plus léger)
cargo test                       # 30+ tests unitaires
```

## Utilisation

```bash
pingui example.com                 # TUI temps réel
pingui 8.8.8.8 -i 200ms -n 50      # 50 paquets toutes les 200 ms
pingui --gui example.com           # ouvrir la fenêtre graphique
pingui 1.1.1.1 -b                  # mode texte (comme ping)
pingui 1.1.1.1 --csv sortie.csv --json flux.jsonl
pingui ::1 -6                      # IPv6
pingui --demo                      # explorer l'interface sans droits réseau
```

### Clés (TUI)

| Touche      | Action                                        |
|-------------|-----------------------------------------------|
| `m` / Tab   | changer de vue (courbe, tableau, carte, dist.) |
| `p` / Espace| pause / reprise                               |
| `s`         | envoyer un paquet isolé (surtout en pause)     |
| `+` / `-`   | doubler / diviser la cadence d'émission        |
| `t`         | cycler les 4 jeux de seuils de couleur         |
| `e`         | changer la taille du paquet ICMP               |
| `r`         | réinitialiser les mesures                      |
| `h` / `?`   | aide                                          |
| `q` / Ctrl+C| quitter (affiche l'état final)                 |

## Privilèges ICMP

- **IPv6** : `SOCK_DGRAM`/ICMPv6 sans privilège sur la plupart des systèmes.
- **Linux** : `SOCK_DGRAM` ICMP via `net.ipv4.ping_group_range`, sinon `SOCK_RAW`
  en root / `CAP_NET_RAW`.
- **macOS / \*BSD** : `SOCK_RAW`, donc root ou `CAP_NET_RAW`.
- **Windows** : socket raw ICMPv4 (peut demander une session admin). En cas
  d'échec, `pingui --demo` fait tourner toute l'interface sans privilège.

## Notes techniques

- `--batch` force l'écriture à la ligne pour les scripts ; l'export vers `-`
  (stdout) est refusé avec la TUI (fournissez un nom de fichier).
- Les pertes sont détectées soit par dépassement de délai, soit par une
  réponse ICMP *Destination Unreachable* incorporate.
- Le RTT est mesuré en interne via un horodatage de la session, indépendant de
  l'horloge système.

Licence MIT — voir [LICENSE](LICENSE).
