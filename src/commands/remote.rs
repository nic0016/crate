use std::path::Path;

use anyhow::{bail, Result};

use crate::remote;
use crate::repo::Repo;

pub fn list(cwd: &Path) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    if repo.config.remotes.is_empty() {
        println!("Keine Remotes. Einrichten mit `crate remote add origin <ordner>`.");
    }
    for (name, url) in &repo.config.remotes {
        println!("{name:<10} {url}");
    }
    Ok(())
}

pub fn add(cwd: &Path, name: &str, url: &str) -> Result<()> {
    let mut repo = Repo::discover(cwd)?;
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c == '/' || c == '\\') {
        bail!("Ungültiger Remote-Name {name:?}");
    }
    if repo.config.remotes.contains_key(name) {
        bail!("Remote {name} existiert bereits (zuerst `crate remote remove {name}`)");
    }
    let url = remote::normalize_url(url)?;
    repo.config.remotes.insert(name.to_string(), url.clone());
    repo.save_config()?;
    println!("Remote {name} → {url}");
    Ok(())
}

pub fn remove(cwd: &Path, name: &str) -> Result<()> {
    let mut repo = Repo::discover(cwd)?;
    if repo.config.remotes.remove(name).is_none() {
        bail!("Remote {name} existiert nicht");
    }
    repo.save_config()?;
    println!("Remote {name} entfernt.");
    Ok(())
}
