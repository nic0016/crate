# crate

Versionskontrolle für CAD-Daten – wie Git, aber für große Binärdateien wie SolidWorks-, Inventor-, CATIA- oder STEP-Dateien.

- Jeder Commit speichert den kompletten Stand eines Ordners.
- Große Dateien werden in Stücke zerlegt; bei kleinen Änderungen wird nur der geänderte Teil neu gespeichert.
- Dateien können gesperrt werden, damit nicht zwei Personen gleichzeitig dieselbe Baugruppe bearbeiten.
- Der Speicher kann auf einem Netzlaufwerk liegen, damit ein Team gemeinsam arbeitet.

## Installation

Voraussetzung ist [Rust](https://rustup.rs). Unter Windows wird zusätzlich ein Linker benötigt: entweder die Visual Studio Build Tools (MSVC) oder MinGW, z. B. `winget install BrechtSanders.WinLibs.POSIX.UCRT`.

```bat
cargo install --path .
crate --version
```

`crate.exe` liegt danach in `%USERPROFILE%\.cargo\bin`.

## Schnellstart

```bat
cd "C:\Projekte\Mein Produkt"
crate init
crate status
crate commit -m "Erster Stand"
crate log
```

`crate init` legt den Unterordner `.crate` an. Die CAD-Dateien selbst bleiben unverändert. Wer `.crate` löscht, entfernt das Repo wieder.

## Typischer Ablauf

```bat
REM Datei im CAD-Programm ändern und speichern, dann:
crate status
crate diff
crate commit -m "Bohrung 8 mm statt 6 mm"
```

### Alte Versionen zurückholen

```bat
REM alte Version als Kopie ablegen (überschreibt nichts)
crate restore Halter.SLDPRT -r HEAD~1 -o C:\Temp\Halter_alt.SLDPRT

REM Datei im Ordner auf eine ältere Version zurücksetzen
crate restore Halter.SLDPRT -r HEAD~1

REM ganzen Ordner auf eine Version setzen und wieder zurück
crate checkout Freigabe-1.0
crate checkout latest
```

Hinweise:

- `restore` ohne `-o` überschreibt die Datei ohne Nachfrage. Noch nicht committete Änderungen gehen dabei verloren.
- Datei vorher im CAD-Programm schließen.
- Eine wiederhergestellte Datei ist erst nach einem neuen `commit` wieder Teil der Historie.

### Versionsangaben (`-r`, `checkout`, `diff`)

| Angabe | Bedeutung |
| --- | --- |
| `HEAD` | Version, auf der der Arbeitsordner basiert |
| `HEAD~2` | zwei Versionen davor |
| `latest` / `main` | neueste Version im Speicher |
| `3f2a…` | Commit-ID (die ersten Zeichen genügen) |
| `Freigabe-1.0` | Tag |

## Befehle

| Befehl | Beschreibung |
| --- | --- |
| `crate init [--store PFAD]` | Neues Repo anlegen, optional mit Speicher woanders, z. B. auf einem Netzlaufwerk |
| `crate status` | Neue, geänderte und gelöschte Dateien anzeigen |
| `crate commit -m "…" [--force]` | Aktuellen Stand als neue Version speichern |
| `crate log [PFAD] [-n N]` | Versionsgeschichte, optional für eine Datei oder einen Ordner |
| `crate diff [VON] [BIS]` | Zwei Versionen vergleichen (ohne Angabe: letzte Version gegen Arbeitsordner) |
| `crate restore PFAD… [-r REV] [-o DATEI]` | Dateien aus einer Version wiederherstellen |
| `crate checkout REV [--force]` | Ganzen Ordner auf eine Version setzen |
| `crate tag [NAME] [REV] [-d]` | Tags auflisten, setzen oder löschen, z. B. für Freigaben |
| `crate lock PFAD… [-m "…"]` | Dateien für andere sperren |
| `crate unlock PFAD… [--force]` | Sperren aufheben |
| `crate locks` | Alle Sperren anzeigen |
| `crate verify [--full]` | Repo prüfen, mit `--full` alle Prüfsummen |
| `crate wal [-n N]` | Write-Ahead-Log anzeigen |
| `crate gc [--dry-run] [--grace MIN]` | Nicht mehr benötigte Objekte löschen |
| `crate remote [add NAME URL \| remove NAME]` | Remotes auflisten, einrichten oder entfernen |
| `crate push [REMOTE] [--force]` | Lokale Versionen und Tags hochladen (Standard: `origin`) |
| `crate pull [REMOTE] [--keep-both \| --mine \| --theirs]` | Neue Versionen holen und den Arbeitsordner aktualisieren |
| `crate clone URL [ORDNER]` | Remote in einen neuen Ordner kopieren |

Mit `-C ORDNER` lässt sich jeder Befehl auf einen anderen Ordner anwenden, z. B. `crate -C "C:\Projekte\Mein Produkt" status`.

## Arbeiten im Team mit push und pull

Wie bei Git hat jeder eine vollständige lokale Kopie und committet zuerst lokal, auch offline. Mit `push` und `pull` wird mit einem gemeinsamen Remote abgeglichen. Ein Remote ist bisher ein Ordner, lokal oder auf einem Netzlaufwerk.

```bat
REM Einmalig: vorhandenes Repo auf das Netzlaufwerk hochladen
crate remote add origin "\\server\cad\mein-produkt"
crate push

REM Kollegen holen sich eine eigene Kopie
crate clone "\\server\cad\mein-produkt" "C:\CAD\Mein Produkt"

REM Täglicher Ablauf
crate pull
crate lock Rahmen\Strebe.SLDPRT -m "Überarbeite Bohrungen"
REM ... bearbeiten ...
crate commit -m "Bohrungen 8 mm"
crate push
crate unlock Rahmen\Strebe.SLDPRT
```

Was dabei passiert:

- **`push`** lädt nur fehlende Datenblöcke hoch und veröffentlicht die Versionen erst danach. Hat jemand anderes inzwischen gepusht, wird `push` abgelehnt: dann zuerst `crate pull`.
- **`pull`** holt neue Versionen und aktualisiert den Arbeitsordner. Ungespeicherte Änderungen an Dateien, die der Pull nicht betrifft, bleiben erhalten.
- **Wenn beide Seiten committet haben**, setzt `pull` die eigenen Versionen automatisch auf den neuen Stand, solange unterschiedliche Dateien geändert wurden. Danach `crate push`.
- **Wurde dieselbe Datei auf beiden Seiten geändert**, ist das ein Konflikt, denn CAD-Dateien lassen sich nicht zusammenführen. Für die Auflösung gibt es drei Optionen:
  - `crate pull --keep-both` behält beide Versionen. Die vom Remote bleibt unter dem Originalnamen, die eigene wird als Kopie daneben gespeichert und mit eingecheckt, z. B. `Strebe (Konflikt nico).SLDPRT`. Baugruppen verweisen weiter auf den Originalnamen.
  - `crate pull --mine` behält die eigene Version.
  - `crate pull --theirs` übernimmt die Version vom Remote und verwirft die eigene.
- **Sperren** liegen auf `origin`, sobald es eingerichtet ist, und sind damit für alle sichtbar. `lock`, `unlock` und `locks` brauchen dafür Zugriff auf `origin`. `commit` und `push` brechen ab, wenn geänderte Dateien von jemand anderem gesperrt sind. Mit `--force` lässt sich das übergehen.
- **Tags** werden mit übertragen. Ein Tag, der auf dem Ziel schon auf eine andere Version zeigt, wird nie verschoben.

Als Remote-URL geht ein leerer oder neuer Ordner, in den beim ersten `push` ein reiner Speicher ohne Arbeitsdateien angelegt wird, oder ein anderes crate-Repo. Alternativ können alle direkt mit `crate init --store "\\server\cad\mein-produkt"` denselben Speicher nutzen, dann ohne `push` und `pull`.

Als Autor wird `BENUTZER@RECHNER` eingetragen. Mit der Umgebungsvariablen `CRATE_AUTHOR` lässt sich ein anderer Name setzen.

## Dateien ausschließen

Diese Dateien werden immer ignoriert:

- Office- und SolidWorks-Sperrdateien (`~$*`)
- `*.tmp`, `*.bak`, `*.swbak`, `*.lck`, `*.swp`
- `OldVersions/`
- `Thumbs.db`, `desktop.ini`, `.DS_Store`, `*.lnk`

Eigene Regeln kommen in die Datei `.crateignore` im Hauptordner. Die Syntax ist dieselbe wie bei `.gitignore`, Groß- und Kleinschreibung wird ignoriert:

```gitignore
Exporte/
*.log
```

## Funktionsweise

- **Chunking:** Dateien werden mit FastCDC in Stücke von etwa 1 MB zerlegt (256 KB bis 4 MB). Jedes Stück wird über seinen BLAKE3-Hash adressiert und nur einmal gespeichert, wenn möglich mit Deflate komprimiert.
- **Manifest und Commit:** Ein Manifest listet alle Dateien einer Version mit ihren Stücken. Ein Commit verweist auf das Manifest und auf die vorherige Version.
- **Write-Ahead-Log (WAL):** Commits, Tags und Sperren werden als Einträge an ein Log angehängt. Daraus entsteht der aktuelle Index mit neuester Version, Tags und Sperren.
- **Stat-Cache:** Dateien, deren Größe und Änderungszeit gleich geblieben sind, werden nicht neu eingelesen.

## Einschränkungen

- Keine Branches und kein Merge; die Historie ist linear.
- Remotes sind bisher nur Ordner und Netzlaufwerke. S3 und ein eigener Server (HTTP) sind vorgesehen, aber noch nicht umgesetzt.
- Keine Benutzerverwaltung oder Zugriffsrechte; der Autor wird vom Client angegeben.

## Entwicklung

```bat
cargo build
cargo test
```
