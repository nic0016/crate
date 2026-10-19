use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use cad_crate::commands;

#[derive(Parser)]
#[command(name = "crate", version, about = "Versionskontrolle für CAD-Daten", long_about = None)]
struct Cli {
    /// Arbeitsverzeichnis (statt des aktuellen Ordners)
    #[arg(short = 'C', long = "dir", global = true)]
    dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Legt im aktuellen Ordner ein neues Repo an
    Init {
        /// Objekte woanders speichern (z. B. Netzlaufwerk)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Zeigt neue, geänderte und gelöschte Dateien
    Status,
    /// Speichert den aktuellen Stand des ganzen Ordners als neue Version
    Commit {
        #[arg(short, long)]
        message: String,
        /// Auch committen, wenn der Arbeitsstand nicht auf der neuesten Version basiert
        /// oder fremd gesperrte Dateien geändert wurden
        #[arg(long)]
        force: bool,
    },
    /// Zeigt die Versionsgeschichte (optional nur für eine Datei/einen Ordner)
    Log {
        path: Option<String>,
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },
    /// Vergleicht zwei Versionen (ohne Angabe: HEAD gegen Arbeitsordner)
    Diff { from: Option<String>, to: Option<String> },
    /// Stellt Dateien aus einer Version wieder her
    Restore {
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(short, long, default_value = "HEAD")]
        rev: String,
        /// Einzelne Datei an einen anderen Ort schreiben (z. B. zum Vergleichen)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Setzt den ganzen Ordner auf eine Version (Commit-ID, Tag, HEAD~n, latest)
    Checkout {
        rev: String,
        /// Lokale Änderungen verwerfen
        #[arg(long)]
        force: bool,
    },
    /// Listet Tags oder setzt/löscht einen Tag (z. B. für Freigaben)
    Tag {
        name: Option<String>,
        rev: Option<String>,
        #[arg(short, long)]
        delete: bool,
        #[arg(long)]
        force: bool,
    },
    /// Sperrt Dateien, damit niemand sonst sie gleichzeitig bearbeitet
    Lock {
        #[arg(required = true)]
        paths: Vec<String>,
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Gibt gesperrte Dateien wieder frei
    Unlock {
        #[arg(required = true)]
        paths: Vec<String>,
        /// Auch Sperren anderer Benutzer aufheben
        #[arg(long)]
        force: bool,
    },
    /// Listet alle Sperren
    Locks,
    /// Zeigt das Write-Ahead-Log
    Wal {
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },
    /// Prüft das Repo: WAL gegen Index, fehlende Objekte, optional alle Prüfsummen
    Verify {
        #[arg(long)]
        full: bool,
    },
    /// Entfernt nicht mehr referenzierte Objekte
    Gc {
        #[arg(long)]
        dry_run: bool,
        /// Nur Objekte löschen, die älter als so viele Minuten sind
        #[arg(long, default_value_t = 60)]
        grace: u64,
    },
}

fn run(cli: Cli) -> anyhow::Result<()> {
    if let Some(dir) = &cli.dir {
        std::env::set_current_dir(dir)?;
    }
    let cwd = std::env::current_dir()?;
    match cli.command {
        Command::Init { store } => commands::init::run(&cwd, store.as_deref()),
        Command::Status => commands::status::run(&cwd),
        Command::Commit { message, force } => commands::commit::run(&cwd, &message, force),
        Command::Log { path, limit } => commands::log::run(&cwd, path.as_deref(), limit),
        Command::Diff { from, to } => commands::diff::run(&cwd, from.as_deref(), to.as_deref()),
        Command::Restore { paths, rev, output } => commands::restore::run(&cwd, &paths, &rev, output.as_deref()),
        Command::Checkout { rev, force } => commands::checkout::run(&cwd, &rev, force),
        Command::Tag { name, rev, delete, force } => {
            commands::tag::run(&cwd, name.as_deref(), rev.as_deref(), delete, force)
        }
        Command::Lock { paths, message } => commands::lock::lock(&cwd, &paths, message),
        Command::Unlock { paths, force } => commands::lock::unlock(&cwd, &paths, force),
        Command::Locks => commands::lock::list(&cwd),
        Command::Wal { limit } => commands::verify::wal(&cwd, limit),
        Command::Verify { full } => commands::verify::run(&cwd, full),
        Command::Gc { dry_run, grace } => commands::gc::run(&cwd, dry_run, grace),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fehler: {e:#}");
            ExitCode::FAILURE
        }
    }
}
