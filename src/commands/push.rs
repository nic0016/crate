use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::cad::human_size;
use crate::repo::{author, Repo};
use crate::sync;
use crate::wal::{self, LockInfo};

pub fn run(cwd: &Path, remote_name: &str, force: bool) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let me = author();
    let store = repo.store();
    let ours = repo.index()?;
    let tip = ours.head.clone().context("Es gibt noch keine Version zum Hochladen. Zuerst `crate commit`.")?;
    let remote = repo.open_remote(remote_name, true)?;
    let remote = remote.as_ref();
    let (theirs, _) = wal::load_index(remote)?;

    let outgoing = match sync::commits_since(store, Some(&tip), theirs.head.as_deref())? {
        Some(commits) => commits,
        None => {
            if sync::commits_since(remote, theirs.head.as_deref(), Some(&tip))?.is_some() {
                println!("Nichts hochzuladen: {remote_name} ist schon weiter. `crate pull` holt die neuen Versionen.");
                return Ok(());
            }
            bail!("{remote_name} enthält Versionen, die du noch nicht hast. Zuerst `crate pull`, dann erneut `crate push`.");
        }
    };

    if outgoing.is_empty() {
        println!("{remote_name} ist bereits aktuell.");
    } else {
        let changed: Vec<Vec<String>> =
            outgoing.iter().map(|(_, c)| sync::changed_paths(store, c)).collect::<Result<_>>()?;
        if !force {
            let foreign = foreign_locks(&theirs.locks, changed.iter().flatten(), &me);
            if !foreign.is_empty() {
                bail!(
                    "Geänderte Dateien sind auf {remote_name} von anderen gesperrt:\n  {}\nMit --force trotzdem hochladen.",
                    foreign.join("\n  ")
                );
            }
        }

        let transfer = sync::copy_commits(store, remote, &outgoing)?;
        let list: Vec<(String, Option<String>)> = outgoing.iter().map(|(id, c)| (id.clone(), c.parent.clone())).collect();
        let conflict = format!(
            "Während des Hochladens hat jemand anderes eine neue Version auf {remote_name} veröffentlicht. \
             `crate pull`, dann erneut `crate push`."
        );
        sync::publish(remote, &me, theirs.head.clone(), &list, &conflict, |idx, i| {
            let foreign = if force { Vec::new() } else { foreign_locks(&idx.locks, changed[i].iter(), &me) };
            if !foreign.is_empty() {
                bail!("Inzwischen gesperrt auf {remote_name}:\n  {}", foreign.join("\n  "));
            }
            Ok(())
        })?;
        println!(
            "{} Version(en) nach {remote_name} hochgeladen ({} neue Datenblöcke, {}).",
            outgoing.len(),
            transfer.chunks,
            human_size(transfer.bytes)
        );
    }

    let tags = sync::copy_tags(&ours.tags, remote, &me, &format!("auf {remote_name}"))?;
    if tags > 0 {
        println!("{tags} Tag(s) hochgeladen.");
    }
    Ok(())
}

fn foreign_locks<'a>(locks: &BTreeMap<String, LockInfo>, paths: impl Iterator<Item = &'a String>, me: &str) -> Vec<String> {
    let paths: BTreeSet<&String> = paths.collect();
    paths
        .into_iter()
        .filter_map(|p| locks.get(p).filter(|l| l.owner != me).map(|l| format!("{p} (gesperrt von {})", l.owner)))
        .collect()
}
