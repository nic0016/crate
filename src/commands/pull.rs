use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};

use super::{apply_update, short, WorkState};
use crate::cad::human_size;
use crate::model::{diff_manifests, stats_of, Commit, Manifest};
use crate::repo::{author, Repo};
use crate::sync;
use crate::wal;

/// Whose version wins for files changed both locally and on the remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolve {
    Mine,
    Theirs,
    /// The remote version keeps the path, the local one is stored next to it.
    KeepBoth,
}

pub fn run(cwd: &Path, remote_name: &str, resolve: Option<Resolve>) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let me = author();
    let store = repo.store();
    let remote = repo.open_remote(remote_name, false)?;
    let remote = remote.as_ref();
    let (theirs, _) = wal::load_index(remote)?;
    let ours = repo.index()?;
    let head = repo.head()?;

    let Some(their_tip) = theirs.head.clone() else {
        println!("{remote_name} enthält noch keine Versionen.");
        return Ok(());
    };
    let our_tip = ours.head.clone();

    if our_tip.as_deref() == Some(their_tip.as_str()) {
        println!("Bereits aktuell mit {remote_name}.");
        return pull_tags(&repo, &theirs.tags, &me);
    }
    if sync::commits_since(store, our_tip.as_deref(), Some(&their_tip))?.is_some() {
        println!("Nichts Neues auf {remote_name}. `crate push` lädt deine lokalen Versionen hoch.");
        return pull_tags(&repo, &theirs.tags, &me);
    }

    let (incoming, local_only, base) = match sync::commits_since(remote, Some(&their_tip), our_tip.as_deref())? {
        Some(incoming) => (incoming, Vec::new(), None),
        None => {
            let base = sync::merge_base(store, our_tip.as_deref(), remote, &their_tip)?
                .with_context(|| format!("Dieses Repo und {remote_name} haben keine gemeinsame Historie."))?;
            if head != our_tip {
                bail!("Dein Arbeitsordner steht nicht auf der neuesten lokalen Version. Zuerst `crate checkout latest`, dann erneut `crate pull`.");
            }
            let incoming = sync::commits_since(remote, Some(&their_tip), Some(&base))?.context("Historie ist inkonsistent")?;
            let local_only = sync::commits_since(store, our_tip.as_deref(), Some(&base))?.context("Historie ist inkonsistent")?;
            (incoming, local_only, Some(base))
        }
    };

    let transfer = sync::copy_commits(remote, store, &incoming)?;
    let mut publish: Vec<(String, Option<String>)> = incoming.iter().map(|(id, c)| (id.clone(), c.parent.clone())).collect();
    let mut tip = their_tip.clone();
    let mut target = repo.manifest_of(&their_tip)?;
    let mut rebased_count = 0;
    let mut copies = BTreeMap::new();
    if let Some(base) = &base {
        let r = rebase(&repo, base, &their_tip, &local_only, resolve, remote_name, &me)?;
        rebased_count = r.commits.len();
        publish.extend(r.commits);
        tip = r.tip;
        target = r.manifest;
        copies = r.copies;
    }

    // The working directory follows only if it was on the latest local version.
    let mut state = None;
    if head == our_tip {
        let s = WorkState::collect(&repo)?;
        let incoming_paths: BTreeSet<String> = diff_manifests(&s.head, &target).into_iter().map(|(p, _)| p).collect();
        let blocked: Vec<&str> = s
            .changes
            .iter()
            .map(|(p, _)| p)
            .filter(|p| incoming_paths.contains(*p) && s.hashes.get(*p) != target.files.get(*p).map(|f| &f.hash))
            .map(String::as_str)
            .collect();
        if !blocked.is_empty() {
            bail!(
                "Ungespeicherte Änderungen an Dateien, die sich auch auf {remote_name} geändert haben:\n  {}\n\
                 Zuerst `crate commit`, dann erneut `crate pull`.",
                blocked.join("\n  ")
            );
        }
        state = Some(s);
    }

    sync::publish(
        store,
        &me,
        our_tip.clone(),
        &publish,
        "Während des Pulls wurde lokal committet. Bitte `crate pull` erneut ausführen.",
        |_, _| Ok(()),
    )?;

    println!(
        "{} neue Version(en) von {remote_name} geholt ({} Datenblöcke, {}).",
        incoming.len(),
        transfer.chunks,
        human_size(transfer.bytes)
    );
    if base.is_some() {
        let dropped = local_only.len() - rebased_count;
        let dropped = if dropped > 0 { format!(", {dropped} ohne verbleibende Änderungen entfallen") } else { String::new() };
        println!(
            "{rebased_count} lokale Version(en) auf den neuen Stand gesetzt{dropped}. `crate push` lädt sie hoch."
        );
    }
    for (path, copy) in &copies {
        println!("Konflikt {path}: Version von {remote_name} übernommen, deine liegt jetzt in {copy}");
    }
    if !copies.is_empty() {
        println!("Hinweis: Baugruppen verweisen weiter auf den Originalnamen, nicht auf die Kopie.");
    }

    match state {
        Some(mut s) => {
            let old = s.head.clone();
            let (written, removed) = apply_update(&repo, &mut s, &old, &target)?;
            repo.set_head(Some(&tip))?;
            s.cache.save(&repo.cache_path())?;
            let c = repo.commit(&tip)?;
            println!("Stand {} \"{}\": {written} Dateien geschrieben, {removed} entfernt.", short(&tip), c.message);
        }
        None => println!(
            "Hinweis: Dein Arbeitsordner steht auf einer älteren Version und wurde nicht verändert. \
             `crate checkout latest` holt den neuen Stand."
        ),
    }
    pull_tags(&repo, &theirs.tags, &me)
}

fn pull_tags(repo: &Repo, tags: &BTreeMap<String, String>, me: &str) -> Result<()> {
    let n = sync::copy_tags(tags, repo.store(), me, "lokal")?;
    if n > 0 {
        println!("{n} Tag(s) übernommen.");
    }
    Ok(())
}

struct Rebased {
    /// New commits as (id, parent), oldest first.
    commits: Vec<(String, Option<String>)>,
    tip: String,
    manifest: Manifest,
    /// Conflicting path -> path of the kept local copy (`--keep-both`).
    copies: BTreeMap<String, String>,
}

/// Replays `local` (oldest first, based on `base`) on top of `onto`.
fn rebase(
    repo: &Repo,
    base: &str,
    onto: &str,
    local: &[(String, Commit)],
    resolve: Option<Resolve>,
    remote_name: &str,
    me: &str,
) -> Result<Rebased> {
    let store = repo.store();
    let base_m = repo.manifest_of(base)?;
    let mut cur = repo.manifest_of(onto)?;
    let mine = repo.manifest_of(&local.last().context("keine lokalen Versionen")?.0)?;

    let theirs_changed: BTreeSet<String> = diff_manifests(&base_m, &cur).into_iter().map(|(p, _)| p).collect();
    let conflicts: BTreeSet<String> = diff_manifests(&base_m, &mine)
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| theirs_changed.contains(p) && mine.files.get(p).map(|f| &f.hash) != cur.files.get(p).map(|f| &f.hash))
        .collect();
    if !conflicts.is_empty() && resolve.is_none() {
        bail!(
            "Konflikt: Diese Dateien wurden lokal und auf {remote_name} geändert:\n  {}\n\
             CAD-Dateien lassen sich nicht zusammenführen. Entscheide, welche Version gilt:\n  \
             crate pull --keep-both   beide behalten, deine Version als Kopie daneben\n  \
             crate pull --mine        deine Version behalten (die andere bleibt in der Historie)\n  \
             crate pull --theirs      Version von {remote_name} übernehmen, deine verwerfen",
            conflicts.iter().cloned().collect::<Vec<_>>().join("\n  ")
        );
    }

    // With --keep-both, local changes to a conflicting file go to a copy instead.
    let mut redirect: BTreeMap<String, String> = BTreeMap::new();
    if resolve == Some(Resolve::KeepBoth) {
        let mut taken: BTreeSet<String> = cur.files.keys().chain(mine.files.keys()).cloned().collect();
        for path in &conflicts {
            let copy = conflict_copy_name(path, me, &taken);
            taken.insert(copy.clone());
            redirect.insert(path.clone(), copy);
        }
    }

    let mut parent = onto.to_string();
    let mut out = Vec::new();
    for (_, c) in local {
        let before = match &c.parent {
            Some(p) => repo.manifest_of(p)?,
            None => Manifest::default(),
        };
        let after = Manifest::load(store, &c.manifest)?;
        let mut next = cur.clone();
        for (path, _) in diff_manifests(&before, &after) {
            let dest = match resolve {
                Some(Resolve::Theirs) if conflicts.contains(&path) => continue,
                Some(Resolve::KeepBoth) if conflicts.contains(&path) => redirect[&path].clone(),
                _ => path.clone(),
            };
            match after.files.get(&path) {
                Some(entry) => {
                    next.files.insert(dest, entry.clone());
                }
                None => {
                    next.files.remove(&dest);
                }
            }
        }
        let changes = diff_manifests(&cur, &next);
        if changes.is_empty() {
            continue;
        }
        let commit = Commit {
            parent: Some(parent.clone()),
            manifest: next.save(store)?,
            author: c.author.clone(),
            timestamp: c.timestamp,
            message: c.message.clone(),
            stats: stats_of(&changes),
            total_files: next.files.len(),
            total_size: next.total_size(),
        };
        let id = commit.save(store)?;
        out.push((id.clone(), Some(parent)));
        parent = id;
        cur = next;
    }
    let copies = redirect.into_iter().filter(|(_, copy)| cur.files.contains_key(copy)).collect();
    Ok(Rebased { commits: out, tip: parent, manifest: cur, copies })
}

/// `Rahmen/Strebe.SLDPRT` -> `Rahmen/Strebe (Konflikt nico).SLDPRT`, unique within `taken`.
fn conflict_copy_name(path: &str, me: &str, taken: &BTreeSet<String>) -> String {
    let user: String = me
        .split('@')
        .next()
        .unwrap_or(me)
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let (dir, file) = path.rsplit_once('/').map(|(d, f)| (format!("{d}/"), f)).unwrap_or((String::new(), path));
    let (stem, ext) = match file.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (file, String::new()),
    };
    (1..)
        .map(|n| {
            let suffix = if n == 1 { String::new() } else { format!(" {n}") };
            format!("{dir}{stem} (Konflikt {user}{suffix}){ext}")
        })
        .find(|candidate| !taken.contains(candidate))
        .expect("unbounded range")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_names() {
        let taken = BTreeSet::from(["Rahmen/Strebe (Konflikt nico).SLDPRT".to_string()]);
        assert_eq!(conflict_copy_name("Rahmen/Strebe.SLDPRT", "nico@PC-1", &BTreeSet::new()), "Rahmen/Strebe (Konflikt nico).SLDPRT");
        assert_eq!(conflict_copy_name("Rahmen/Strebe.SLDPRT", "nico", &taken), "Rahmen/Strebe (Konflikt nico 2).SLDPRT");
        assert_eq!(conflict_copy_name("README", "a:b", &BTreeSet::new()), "README (Konflikt a_b)");
    }
}
