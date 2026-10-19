//! Remote URLs. A remote is a folder (local disk or network share) that is either a
//! working copy with a `.crate` directory or a bare store as created by `push`.

use std::path::PathBuf;

use anyhow::{bail, Result};

use crate::repo::{Config, REPO_DIR};

/// Validates a user-supplied URL and turns paths into absolute ones.
pub fn normalize_url(input: &str) -> Result<String> {
    let path = match input.strip_prefix("file://") {
        // file:///C:/x -> C:/x
        Some(rest) => rest.strip_prefix('/').filter(|r| r.as_bytes().get(1) == Some(&b':')).unwrap_or(rest),
        None => {
            if let Some((scheme, _)) = input.split_once("://") {
                bail!("Remote-Typ {scheme}:// wird noch nicht unterstützt, bisher nur Ordner und Netzlaufwerke");
            }
            input
        }
    };
    if path.trim().is_empty() {
        bail!("Leere Remote-Adresse");
    }
    Ok(std::path::absolute(path)?.to_string_lossy().into_owned())
}

/// The store directory behind a remote URL.
pub fn store_path(url: &str) -> Result<PathBuf> {
    let root = PathBuf::from(url);
    let dir = root.join(REPO_DIR);
    let config = dir.join("config.json");
    if config.is_file() {
        return Ok(Config::load(&config)?.store.resolve(&dir));
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert!(normalize_url("s3://bucket/x").is_err());
        assert!(normalize_url("").is_err());
        let p = normalize_url("file:///C:/cad/produkt").unwrap();
        assert!(p.starts_with("C:") || p.starts_with('/'), "{p}");
        assert!(PathBuf::from(normalize_url("relativ").unwrap()).is_absolute());
    }
}
