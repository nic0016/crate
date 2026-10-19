use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::cad::{human_size, DEFAULT_IGNORES};
use crate::repo::{Repo, IGNORE_FILE};
use crate::scan;

pub fn run(cwd: &Path, store: Option<&Path>) -> Result<()> {
    let repo = Repo::init(cwd, store)?;
    let ignore_file = repo.root.join(IGNORE_FILE);
    if !ignore_file.exists() {
        let mut text = String::from(
            "# Dateien, die crate nicht versioniert (gitignore-Syntax, Groß-/Kleinschreibung egal).\n\
             # Diese Muster sind immer aktiv:\n",
        );
        for p in DEFAULT_IGNORES {
            text.push_str(&format!("#   {p}\n"));
        }
        text.push_str("\n# Eigene Muster, z. B.:\n# Exporte/\n# *.zip\n");
        fs::write(&ignore_file, text)?;
    }
    let files = scan::scan(&repo.root)?;
    let total: u64 = files.iter().map(|f| f.size).sum();
    println!("Leeres crate-Repo angelegt in {}", repo.dir.display());
    println!("{} Dateien ({}) können versioniert werden.", files.len(), human_size(total));
    println!("Nächster Schritt: crate commit -m \"Erster Stand\"");
    Ok(())
}
