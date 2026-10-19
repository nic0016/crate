use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

fn crate_cmd(dir: &Path, author: &str) -> Command {
    let mut cmd = Command::cargo_bin("crate").unwrap();
    cmd.current_dir(dir).env("CRATE_AUTHOR", author);
    cmd
}

fn run(dir: &Path, args: &[&str]) -> String {
    let out = crate_cmd(dir, "nico").args(args).assert().success().get_output().stdout.clone();
    String::from_utf8(out).unwrap()
}

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

fn setup() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    fs::create_dir_all(p.join("Rahmen")).unwrap();
    fs::write(p.join("Rahmen/Hauptbaugruppe.SLDASM"), noise(3_000_000, 1)).unwrap();
    fs::write(p.join("Rahmen/Strebe.SLDPRT"), noise(200_000, 2)).unwrap();
    fs::write(p.join("Rahmen/Strebe.SLDDRW"), noise(100_000, 3)).unwrap();
    fs::write(p.join("Export.STEP"), b"ISO-10303-21;\nHEADER;\n".repeat(5000)).unwrap();
    // Must be ignored by default
    fs::write(p.join("Rahmen/~$Strebe.SLDPRT"), b"lock").unwrap();
    fs::create_dir_all(p.join("OldVersions")).unwrap();
    fs::write(p.join("OldVersions/alt.ipt"), b"old").unwrap();
    dir
}

#[test]
fn full_workflow() {
    let dir = setup();
    let p = dir.path();

    let out = run(p, &["init"]);
    assert!(out.contains("5 Dateien"), "{out}"); // 4 CAD files + .crateignore

    let out = run(p, &["status"]);
    assert!(out.contains("Baugruppe (1)") && out.contains("neu"), "{out}");
    assert!(!out.contains("~$Strebe") && !out.contains("OldVersions"), "{out}");

    let out = run(p, &["commit", "-m", "Erster Stand"]);
    assert!(out.contains("5 neu, 0 geändert, 0 gelöscht"), "{out}");
    assert!(run(p, &["status"]).contains("Keine Änderungen"));
    assert!(run(p, &["commit", "-m", "leer"]).contains("nichts zu committen"));
    run(p, &["tag", "Rev-A"]);

    // Modify part, delete drawing, add a new part.
    let original_part = fs::read(p.join("Rahmen/Strebe.SLDPRT")).unwrap();
    fs::write(p.join("Rahmen/Strebe.SLDPRT"), noise(210_000, 99)).unwrap();
    fs::remove_file(p.join("Rahmen/Strebe.SLDDRW")).unwrap();
    fs::write(p.join("Rahmen/Winkel.SLDPRT"), noise(50_000, 5)).unwrap();

    let out = run(p, &["status"]);
    assert!(out.contains("geändert  Rahmen/Strebe.SLDPRT"), "{out}");
    assert!(out.contains("gelöscht  Rahmen/Strebe.SLDDRW"), "{out}");
    assert!(out.contains("neu       Rahmen/Winkel.SLDPRT"), "{out}");

    let out = run(p, &["commit", "-m", "Strebe verlängert"]);
    assert!(out.contains("1 neu, 1 geändert, 1 gelöscht"), "{out}");

    let out = run(p, &["log"]);
    assert!(out.contains("Strebe verlängert") && out.contains("Tag: Rev-A") && out.contains("HEAD, latest"), "{out}");
    let out = run(p, &["log", "Rahmen/Strebe.SLDPRT"]);
    assert!(out.contains("Strebe verlängert") && out.contains("Erster Stand"), "{out}");
    let out = run(p, &["log", "Export.STEP"]);
    assert!(!out.contains("Strebe verlängert"), "{out}");

    let out = run(p, &["diff", "Rev-A", "HEAD"]);
    assert!(out.contains("Rahmen/Winkel.SLDPRT") && out.contains("Rahmen/Strebe.SLDDRW"), "{out}");

    // Restore a single old version next to the current one, then in place.
    run(p, &["restore", "Rahmen/Strebe.SLDPRT", "--rev", "Rev-A", "-o", "alt.SLDPRT"]);
    assert_eq!(fs::read(p.join("alt.SLDPRT")).unwrap(), original_part);
    fs::remove_file(p.join("alt.SLDPRT")).unwrap();
    run(p, &["restore", "Rahmen/Strebe.SLDPRT", "--rev", "HEAD~1"]);
    assert_eq!(fs::read(p.join("Rahmen/Strebe.SLDPRT")).unwrap(), original_part);
    run(p, &["restore", "Rahmen/Strebe.SLDPRT"]);
    assert!(run(p, &["status"]).contains("Keine Änderungen"));

    // Whole project back to Rev-A.
    run(p, &["checkout", "Rev-A"]);
    assert!(p.join("Rahmen/Strebe.SLDDRW").exists());
    assert!(!p.join("Rahmen/Winkel.SLDPRT").exists());
    assert_eq!(fs::read(p.join("Rahmen/Strebe.SLDPRT")).unwrap(), original_part);
    assert!(p.join("Rahmen/~$Strebe.SLDPRT").exists(), "ignored files must be left alone");

    // Committing on an old state needs --force.
    fs::write(p.join("Export.STEP"), b"neu").unwrap();
    crate_cmd(p, "nico").args(["commit", "-m", "x"]).assert().failure().stderr(contains("checkout latest"));
    crate_cmd(p, "nico").args(["checkout", "latest"]).assert().failure().stderr(contains("ungespeicherte"));
    run(p, &["checkout", "latest", "--force"]);
    assert!(p.join("Rahmen/Winkel.SLDPRT").exists());
    assert!(run(p, &["status"]).contains("Keine Änderungen"));

    let out = run(p, &["verify", "--full"]);
    assert!(out.contains("Alles in Ordnung"), "{out}");
    let out = run(p, &["wal"]);
    assert!(out.contains("commit") && out.contains("tag     Rev-A"), "{out}");
    let out = run(p, &["gc", "--grace", "0"]);
    assert!(out.contains("0 nicht referenzierte"), "{out}");
}

#[test]
fn locks_block_foreign_commits() {
    let dir = setup();
    let p = dir.path();
    run(p, &["init"]);
    run(p, &["commit", "-m", "Start"]);

    crate_cmd(p, "anna").args(["lock", "Rahmen/Strebe.SLDPRT", "-m", "überarbeite Bohrungen"]).assert().success();
    crate_cmd(p, "nico").args(["lock", "Rahmen/Strebe.SLDPRT"]).assert().failure().stderr(contains("anna"));
    assert!(run(p, &["locks"]).contains("anna"));

    fs::write(p.join("Rahmen/Strebe.SLDPRT"), noise(1000, 7)).unwrap();
    assert!(run(p, &["status"]).contains("fremd gesperrt"));
    crate_cmd(p, "nico").args(["commit", "-m", "x"]).assert().failure().stderr(contains("gesperrt von anna"));
    crate_cmd(p, "nico").args(["unlock", "Rahmen/Strebe.SLDPRT"]).assert().failure();
    crate_cmd(p, "anna").args(["unlock", "Rahmen/Strebe.SLDPRT"]).assert().success();
    run(p, &["commit", "-m", "jetzt geht's"]);
}

#[test]
fn dedup_keeps_repo_small() {
    let dir = setup();
    let p = dir.path();
    run(p, &["init"]);
    run(p, &["commit", "-m", "Start"]);
    let size_of_store = || -> u64 {
        walkdir::WalkDir::new(p.join(".crate/objects"))
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.metadata().unwrap().len())
            .sum()
    };
    let before = size_of_store();
    // Copy of the 3 MB assembly under a new name plus a tiny edit to the original.
    fs::copy(p.join("Rahmen/Hauptbaugruppe.SLDASM"), p.join("Rahmen/Kopie.SLDASM")).unwrap();
    let mut data = fs::read(p.join("Rahmen/Hauptbaugruppe.SLDASM")).unwrap();
    let n = data.len();
    data[n - 10..].fill(0);
    fs::write(p.join("Rahmen/Hauptbaugruppe.SLDASM"), data).unwrap();
    let out = run(p, &["commit", "-m", "Kopie"]);
    let growth = size_of_store() - before;
    assert!(growth < 1_500_000, "repo grew by {growth} bytes: {out}");
}

fn run_as(dir: &Path, author: &str, args: &[&str]) -> String {
    let out = crate_cmd(dir, author).args(args).assert().success().get_output().stdout.clone();
    String::from_utf8(out).unwrap()
}

#[test]
fn push_pull_clone() {
    let dir = setup();
    let a = dir.path();
    let base = tempfile::tempdir().unwrap();
    let remote = base.path().join("remote");
    let b = base.path().join("anna");

    run(a, &["init"]);
    run(a, &["commit", "-m", "Start"]);
    run(a, &["tag", "Rev-A"]);
    crate_cmd(a, "nico").arg("push").assert().failure().stderr(contains("remote add"));
    run(a, &["remote", "add", "origin", remote.to_str().unwrap()]);
    assert!(run(a, &["remote"]).contains("origin"));
    let out = run(a, &["push"]);
    assert!(out.contains("1 Version(en) nach origin") && out.contains("1 Tag(s)"), "{out}");
    assert!(run(a, &["push"]).contains("bereits aktuell"));

    // Clone gets files, history and tags.
    let out = run_as(base.path(), "anna", &["clone", remote.to_str().unwrap(), "anna"]);
    assert!(out.contains("1 neue Version(en)") && out.contains("1 Tag(s)"), "{out}");
    assert_eq!(fs::read(b.join("Rahmen/Strebe.SLDPRT")).unwrap(), fs::read(a.join("Rahmen/Strebe.SLDPRT")).unwrap());
    assert!(run_as(&b, "anna", &["status"]).contains("Keine Änderungen"));
    assert!(run_as(&b, "anna", &["log"]).contains("Tag: Rev-A"));
    crate_cmd(base.path(), "anna").args(["clone", remote.to_str().unwrap(), "anna"]).assert().failure();

    // Fast-forward: anna pushes, nico pulls.
    fs::write(b.join("Rahmen/Strebe.SLDPRT"), noise(220_000, 10)).unwrap();
    run_as(&b, "anna", &["commit", "-m", "Strebe von Anna"]);
    run_as(&b, "anna", &["push"]);
    let out = run(a, &["pull"]);
    assert!(out.contains("1 neue Version(en)") && out.contains("1 Dateien geschrieben"), "{out}");
    assert_eq!(fs::read(a.join("Rahmen/Strebe.SLDPRT")).unwrap(), noise(220_000, 10));
    assert!(run(a, &["status"]).contains("Keine Änderungen"));
    assert!(run(a, &["pull"]).contains("Bereits aktuell"));

    // Both commit different files: push is rejected, pull rebases automatically.
    fs::write(a.join("Export.STEP"), b"nico").unwrap();
    run(a, &["commit", "-m", "Export von Nico"]);
    fs::write(b.join("Rahmen/Winkel.SLDPRT"), noise(40_000, 11)).unwrap();
    run_as(&b, "anna", &["commit", "-m", "Winkel von Anna"]);
    run_as(&b, "anna", &["push"]);
    crate_cmd(a, "nico").arg("push").assert().failure().stderr(contains("crate pull"));
    let out = run(a, &["pull"]);
    assert!(out.contains("1 lokale Version(en) auf den neuen Stand"), "{out}");
    assert!(a.join("Rahmen/Winkel.SLDPRT").exists());
    assert_eq!(fs::read(a.join("Export.STEP")).unwrap(), b"nico");
    run(a, &["push"]);
    run_as(&b, "anna", &["pull"]);
    assert_eq!(fs::read(b.join("Export.STEP")).unwrap(), b"nico");
    let log = run_as(&b, "anna", &["log"]);
    assert!(log.contains("Export von Nico") && log.contains("Winkel von Anna"), "{log}");

    // Uncommitted edits to files the pull does not touch survive it.
    fs::write(b.join("Rahmen/Strebe.SLDDRW"), b"anna arbeitet noch").unwrap();
    fs::write(a.join("Rahmen/Winkel.SLDPRT"), noise(41_000, 12)).unwrap();
    run(a, &["commit", "-m", "Winkel angepasst"]);
    run(a, &["push"]);
    run_as(&b, "anna", &["pull"]);
    assert_eq!(fs::read(b.join("Rahmen/Strebe.SLDDRW")).unwrap(), b"anna arbeitet noch");
    assert_eq!(fs::read(b.join("Rahmen/Winkel.SLDPRT")).unwrap(), noise(41_000, 12));
    run_as(&b, "anna", &["restore", "Rahmen/Strebe.SLDDRW"]);

    // Same file on both sides: conflict until someone decides.
    fs::write(a.join("Rahmen/Hauptbaugruppe.SLDASM"), noise(100_000, 20)).unwrap();
    run(a, &["commit", "-m", "Baugruppe Nico"]);
    run(a, &["push"]);
    fs::write(b.join("Rahmen/Hauptbaugruppe.SLDASM"), noise(100_000, 24)).unwrap();
    run_as(&b, "anna", &["commit", "-m", "Baugruppe Anna"]);
    crate_cmd(&b, "anna").arg("pull").assert().failure().stderr(contains("Konflikt").and(contains("Hauptbaugruppe")));
    run_as(&b, "anna", &["pull", "--theirs"]);
    assert_eq!(fs::read(b.join("Rahmen/Hauptbaugruppe.SLDASM")).unwrap(), noise(100_000, 20));
    assert!(run_as(&b, "anna", &["status"]).contains("Keine Änderungen"));
    assert!(run_as(&b, "anna", &["push"]).contains("bereits aktuell"));

    // --keep-both: remote version keeps the name, the local one becomes a copy.
    fs::write(a.join("Rahmen/Strebe.SLDDRW"), noise(50_000, 40)).unwrap();
    run(a, &["commit", "-m", "Zeichnung Nico"]);
    run(a, &["push"]);
    fs::write(b.join("Rahmen/Strebe.SLDDRW"), noise(50_000, 42)).unwrap();
    run_as(&b, "anna", &["commit", "-m", "Zeichnung Anna"]);
    let out = run_as(&b, "anna", &["pull", "--keep-both"]);
    assert!(out.contains("Strebe (Konflikt anna).SLDDRW"), "{out}");
    let copy = "Rahmen/Strebe (Konflikt anna).SLDDRW";
    assert_eq!(fs::read(b.join("Rahmen/Strebe.SLDDRW")).unwrap(), noise(50_000, 40));
    assert_eq!(fs::read(b.join(copy)).unwrap(), noise(50_000, 42));
    assert!(run_as(&b, "anna", &["status"]).contains("Keine Änderungen"));
    assert!(run_as(&b, "anna", &["log", copy]).contains("Zeichnung Anna"));
    run_as(&b, "anna", &["push"]);
    run(a, &["pull"]);
    assert_eq!(fs::read(a.join(copy)).unwrap(), noise(50_000, 42));

    // Locks live on origin and are seen by everyone.
    run(a, &["lock", "Rahmen/Strebe.SLDPRT"]);
    assert!(run_as(&b, "anna", &["locks"]).contains("nico"));
    fs::write(b.join("Rahmen/Strebe.SLDPRT"), noise(1000, 30)).unwrap();
    crate_cmd(&b, "anna").args(["commit", "-m", "x"]).assert().failure().stderr(contains("gesperrt von nico"));
    run(a, &["unlock", "Rahmen/Strebe.SLDPRT"]);
    run_as(&b, "anna", &["commit", "-m", "Strebe frei"]);
    run_as(&b, "anna", &["push"]);

    assert!(run(a, &["verify", "--full"]).contains("Alles in Ordnung"));
    assert!(run_as(&b, "anna", &["verify", "--full"]).contains("Alles in Ordnung"));
}

#[test]
fn errors_outside_repo() {
    let dir = tempfile::tempdir().unwrap();
    crate_cmd(dir.path(), "nico").arg("status").assert().failure().stderr(contains("crate init"));
}
