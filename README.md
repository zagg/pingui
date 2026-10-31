![pingui](https://img.shields.io/github/v/release/zagg/pingui) ![test](https://github.com/zagg/pingui/workflows/CI/badge.svg)

# pingui

**Ping graphique avec historique en temps réel, en Rust** — la reprise moderne
du vieil *Graphical Ping* de Windows (hôte, taille, intervalle, Démarrer/Arrêter,
stats + grand graphique en barres), **en mieux** : ICMP natif, tooltip au survol,
carte de trafic, histogramme, percentiles, jitter, export CSV/JSON, et un TUI
dans l'esprit de [gping](https://github.com/orhun/gping) pour le terminal.

```
┌──────────────────────────────────────────────────────────────┐
│ pingui — 8.8.8.8                                             │
│ Hôte [8.8.8.8            ] Taille[64 o] Int.[1000 ms] [■ Stop]│
│──────────────────────────────────────────────────────────────│
│ Dernier paquet : #509 (181 ms)   RTT récents : 175–251 ms    │
│ RTT total min/avg/max : 29/193/255 ms   Pertes tot. : 0 (0%) │
│ jitter 4.2 ms  p50 190 · p90 231 · p95 244 · p99 251 ms      │
│                                                              │
│ 500│▁▁▁▁▁▁▁▁▁▁▁▁▂▂▂▂▂▂▂▂▂▂▂▂▂▂▂▂▁▁▁▁▂▂▂▂▂▂▂▂▂▂▂▂▁▁▂        │
│ 250│███▇▇▇███▇▇█▇▇███▇▇▇▇███▇▇███▇▇▇▇▇██▇▇▇▇▇███▇▇  ██│  │
│    └──────────────────────────────────────────────────────────│
│   [Courbe] [Carte de trafic] [Histogramme] [Paquets]          │
│ 8.8.8.8 · [raw socket ICMPv4] · 0% perte                     │
└──────────────────────────────────────────────────────────────┘
```

## Les deux binaires

| Binaire | Rôle | Lancement sans argument |
|---|---|---|
| **`pingui.exe`** | interface **fenêtrée** (egui/eframe) : le « Graphical Ping » en mieux | ouvre la **fenêtre** directement |
| **`pingui-cli.exe`** | version terminal, légère, sans dépendances graphiques | démarre le **TUI** (esprit gping) |

Le code source est unique : `pingui-cli.exe` est simplement construit avec
`--no-default-features` (feature `gui` désactivée). Dans `pingui.exe`,
`-T/--tui` bascule vers le TUI et `-b/--batch` vers le mode texte scriptable.

## Utilisation

```bash
pingui                                # fenêtre graphique vide → taper l'hôte → Démarrer
pingui example.com                    # fenêtre déjà lancée sur example.com
pingui 8.8.8.8 -i 200ms -n 50         # 50 paquets toutes les 200 ms
pingui 1.1.1.1 --csv sortie.csv --json flux.jsonl   # exports continus
pingui example.com -T                 # TUI terminal (pingui-cli.exe : pas besoin de -T)
pingui example.com -b                 # mode texte, une ligne par paquet (code de retour style ping)
pingui --demo                         # trafic simulé : découvrir l'interface sans aucun droit
```

### Options principales

| Option | Description |
|---|---|
| `-n, --count N` | nombre de paquets (défaut : infini) |
| `-i, --interval` | cadence, ex. `200ms`, `1s` (10 ms – 60 s) |
| `-s, --size N` | taille de la charge ICMP (min 16) |
| `--timeout` | délai avant comptabilisation d'une perte (défaut 2 s) |
| `-4` / `-6` | forcer la famille d'adresse |
| `--thresholds a,b,c,d` | seuils de couleur (ms) ; touche `t` pour 4 préréglages |
| `--window N` | paquets affichés sur la courbe/carte (défaut 240) |
| `--csv` / `--json` | écrit chaque échantillon au fil de l'eau |
| `--demo [--seed N]` | générateur synthétique déterministe |

### Clés (TUI, `pingui-cli` ou `-T`)

`m`/Tab vue · `p`/Espace pause · `s` paquet isolé · `+`/`-` cadence · `t` seuils ·
`e` taille paquet · `r` reset · `?`/`h` aide · `q`/Ctrl-C quitter (état final imprimé).

## Privilèges ICMP

- **Windows** : socket brut ICMPv4 (testé non surélevé sur Windows 11 ; sinon,
  lancer en administrateur ou utiliser `--demo`).
- **Linux** : `SOCK_DGRAM` ICMP si `net.ipv4.ping_group_range` autorise le
  groupe, sinon `SOCK_RAW` (root / `CAP_NET_RAW`).
- **macOS / \*BSD** : `SOCK_RAW` → root ou `CAP_NET_RAW`.
- **IPv6 partout** : `SOCK_DGRAM`/ICMPv6, fonctionne sans privilèges.

## Compilation

```bash
cargo build --release                          # → target/release/pingui.exe (GUI + TUI)
cargo build --release --no-default-features    # → pingui-cli.exe (TUI seul, léger)
cargo test                                     # 30 tests unitaires (ICMP, stats, viz, CLI…)
```

Release CI : `dist/pingui-v*-windows-x86_64.zip` contient les **deux** exécutables
précompilés ; les mêmes fichiers sont joints aux releases GitHub et Gitea.

## Idées de fonctionnalités sympas

- **Alerte sonore et notification** quand les pertes dépassent un seuil.
- **Multi-hôtes côte à côte** (une courbe par cible, comparaison WAN/LAN).
- **Enregistreur / rejeu** de session (`--record session.jsonl`, `--play`) pour
  montrer un historique à quelqu'un d'autre.
- **Zoom / curseur sur la courbe** (déjà: tooltip) + export PNG du graphique.
- **GeoIP léger** : nom de l'opérateur/ville sous l'axe (base MaxMind Mini).
- **Sondage de chemin** en option (`--trace`, façon traceroute minimal).
- **Peaux et thème sombre/clair** dans la fenêtre.
- **Compilage en wasm** (`eframe` + `wasm32`) : une page web de ping permanent.

## Licence

MIT — voir [LICENSE](LICENSE).
