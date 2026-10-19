# crate

**Version control for CAD data.** `crate` versions whole project folders of large binary files (SolidWorks, Inventor, CATIA, NX, STEP, …). It combines three ideas:

- **Content-defined chunking:** a small edit to a 500 MB assembly costs megabytes of storage, not another 500 MB.
- **Content-addressed, immutable object storage:** deduplication, integrity checks and idempotent, resumable transfers come for free.
- **A write-ahead log linearized by a single compare-and-swap:** several users can safely share one store, even on a plain network drive, with no server process.

Collaboration is pessimistic and lock-based (check-out / check-in), as is usual for CAD. Distributed `push` / `pull` / `clone` are built on top.

Author: **Nicolas Wörmann** · License: **MIT OR Apache-2.0**

---

## Contents

1. [Installation](#installation)
2. [Quick start](#quick-start)
3. [Command reference](#command-reference)
4. [Working in a team](#working-in-a-team)
5. [Design](#design)
6. [Limitations and roadmap](#limitations-and-roadmap)
7. [Development](#development)
8. [License](#license)

---

## Installation

`crate` is built from source with [Rust](https://rustup.rs). You also need a native linker.

### Windows

```powershell
winget install Rustlang.Rustup
```

Then install a linker for your Rust toolchain:

- **MSVC toolchain** (rustup default): Visual Studio Build Tools with the *Desktop development with C++* workload. The rustup installer offers to install them.
- **GNU toolchain** (`x86_64-pc-windows-gnu`): MinGW-w64, e.g. `winget install BrechtSanders.WinLibs.POSIX.UCRT`. Its `bin` directory must be on `PATH`.

Open a new terminal so `PATH` is refreshed, then:

```powershell
cargo install --git https://github.com/nic0016/crate
crate --version
```

### macOS

```bash
xcode-select --install                                           # Apple Command Line Tools (linker)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust
source "$HOME/.cargo/env"
cargo install --git https://github.com/nic0016/crate
crate --version
```

The binary ends up in `%USERPROFILE%\.cargo\bin` (Windows) or `~/.cargo/bin` (macOS/Linux). From a local checkout, use `cargo install --path .` instead.

---

## Quick start

```bat
cd "C:\Projects\Gearbox"
crate init
crate status
crate commit -m "Initial state"
```

`init` creates `.crate\` and a commented `.crateignore`. Your CAD files are not touched. `status` shows what would be versioned.

Then, after editing files in your CAD program:

```bat
crate status
crate diff
crate commit -m "Bore 8 mm instead of 6 mm"
crate log
```

`status` lists new, modified and deleted files grouped by CAD category (assembly, part, drawing, …). `diff` adds the size deltas.

To go back in time:

```bat
REM old version as a separate copy, nothing is overwritten
crate restore Housing.SLDPRT -r HEAD~1 -o C:\Temp\Housing_old.SLDPRT

REM overwrite the file in place
crate restore Housing.SLDPRT -r HEAD~1

REM whole folder to a tag, and back to the newest version
crate checkout Release-1.0
crate checkout latest
```

Close the file in your CAD program before restoring it. `restore` without `-o` overwrites uncommitted changes without asking. To stop using `crate`, delete the `.crate` folder.

---

## Command reference

Every command accepts `-C <DIR>` to run on another folder, e.g. `crate -C "C:\Projects\Gearbox" status`.

### Local versioning

| Command | Description |
| --- | --- |
| `crate init [--store <PATH>]` | Create a repository in the current folder. `--store` puts the object store elsewhere, e.g. on a network share. |
| `crate status` | Show new, modified and deleted files and current locks. |
| `crate commit -m <MSG> [--force]` | Record a snapshot of the whole folder. Refuses if the working folder is not based on the latest version, or if changed files are locked by someone else (override with `--force`). |
| `crate log [<PATH>] [-n <N>]` | Show history, optionally only commits that touched a file or folder. |
| `crate diff [<FROM>] [<TO>]` | Compare two versions. Without arguments: latest commit against the working folder. |
| `crate restore <PATH>… [-r <REV>] [-o <FILE>]` | Restore files from a version (default `HEAD`). `-o` writes a single file elsewhere instead of overwriting it. |
| `crate checkout <REV> [--force]` | Set the whole folder to a version. Refuses if there are uncommitted changes (discard them with `--force`). Untracked files are left alone. |
| `crate tag [<NAME> [<REV>]] [-d] [--force]` | List tags, create one (default `HEAD`), delete with `-d`, move with `--force`. |

### Locks

| Command | Description |
| --- | --- |
| `crate lock <PATH>… [-m <MSG>]` | Lock files so nobody else can commit changes to them. |
| `crate unlock <PATH>… [--force]` | Release your locks. `--force` releases other users' locks. |
| `crate locks` | List all locks with owner, time and message. |

### Remotes

| Command | Description |
| --- | --- |
| `crate remote` | List configured remotes. |
| `crate remote add <NAME> <URL>` | Add a remote (a local folder, UNC path or `file://` URL). |
| `crate remote remove <NAME>` | Remove a remote from the configuration. Data at the remote is kept. |
| `crate push [<REMOTE>] [--force]` | Upload local commits and tags (default remote `origin`). `--force` only overrides foreign locks; it never rewrites remote history. |
| `crate pull [<REMOTE>] [--keep-both \| --mine \| --theirs]` | Fetch new commits, rebase local ones and update the working folder. The flags resolve conflicts (see below). |
| `crate clone <URL> [<DIR>]` | Create a new working folder from a remote. |

### Maintenance

| Command | Description |
| --- | --- |
| `crate verify [--full]` | Replay the WAL and compare it with the index, check that all referenced objects exist. `--full` reads every chunk and verifies its hash. |
| `crate wal [-n <N>]` | Print the write-ahead log. |
| `crate gc [--dry-run] [--grace <MIN>]` | Delete unreferenced objects older than the grace period (default 60 min). |

### Revisions

| Syntax | Meaning |
| --- | --- |
| `HEAD` | The version the working folder is based on |
| `HEAD~n` | `n` versions before `HEAD` |
| `latest`, `main` | The newest version in the store |
| `3f2a9c…` | Commit ID; any unambiguous prefix of at least 4 hex characters |
| `Release-1.0` | A tag |

### Ignored files

Always ignored: Office/SolidWorks owner files (`~$*`), `*.tmp`, `*.bak`, `*.swbak`, `*.lck`, `*.swp`, Inventor's `OldVersions/`, `Thumbs.db`, `desktop.ini`, `.DS_Store`, `*.lnk`. Additional patterns go into `.crateignore`, which uses gitignore syntax and is case-insensitive.

The author is recorded as `USERNAME@COMPUTERNAME`. Set the environment variable `CRATE_AUTHOR` to override it.

---

## Working in a team

Every user has a full local repository and commits locally, also offline. `push` and `pull` synchronize with a shared remote.

```bat
REM once: publish an existing repository
crate remote add origin "\\fileserver\cad\gearbox"
crate push

REM colleagues
crate clone "\\fileserver\cad\gearbox" "C:\CAD\Gearbox"

REM daily loop
crate pull
crate lock Frame\Strut.SLDPRT -m "reworking bores"
REM edit the part in CAD, then
crate commit -m "Bores 8 mm"
crate push
crate unlock Frame\Strut.SLDPRT
```

- **Locks** live on `origin` once it is configured, so the whole team sees them. `commit` and `push` both check them.
- **`push`** is fast-forward only. If someone else pushed first, it is rejected and you run `crate pull` first.
- **`pull`** fast-forwards when possible. If both sides have committed, local commits are rebased onto the remote ones automatically, as long as the two sides changed different files. Uncommitted edits to files that the pull does not touch are kept.
- **Conflicts** happen when the same file was changed on both sides. Binary CAD files cannot be merged, so you choose:

  | Flag | Result |
  | --- | --- |
  | `--keep-both` | The remote version keeps the path, yours is committed next to it as `Strut (Konflikt nico).SLDPRT`. Assemblies keep referencing the original path. |
  | `--mine` | Your version wins. The other one stays in the history. |
  | `--theirs` | The remote version wins. Your change to that file is dropped. |

Instead of push/pull, all users can also share one store directly with `crate init --store "\\fileserver\cad\gearbox"`. Every commit is then immediately visible to everyone.

---

## Design

### Repository layout

```
<project>/
├── .crateignore
├── …CAD files…
└── .crate/
    ├── config.json        store location, remotes
    ├── HEAD               commit the working folder is based on (local)
    ├── stat-cache.json    path → (size, mtime, hash, chunk list)
    └── objects/           the store (may live elsewhere, see --store)
        ├── chunks/ab/cdef…    file content, addressed by BLAKE3
        ├── manifests/<id>     snapshot: path → (size, mtime, hash, chunks)
        ├── commits/<id>       parent, manifest, author, time, message, stats
        ├── wal/00000000000000000042   immutable log entries
        └── index              materialized view: head, tags, locks, seq
```

All objects except `index` are immutable and named by the BLAKE3 hash of their content, so the store forms a Merkle DAG: commit → manifest → chunks. Manifests and commits are deterministic JSON (paths are kept sorted). Their ID is the hash of that JSON, and they are stored compressed.

### Content-defined chunking

Files are split with **FastCDC** (the 2020 variant, with normalized chunking) into chunks of 256 KiB min / 1 MiB average / 4 MiB max. Boundaries come from a rolling gear hash over the content, not from fixed offsets. Inserting or deleting bytes therefore shifts boundaries only locally, and all other chunks keep their hashes. With fixed-size blocks, an insertion would invalidate every block after it.

In practice:

- A local edit to a file of any size stores roughly the changed bytes, rounded up to a few chunks of about 1 MiB.
- Deduplication works across versions and across files: copying an assembly under a new name costs almost nothing.
- Each chunk is stored with a 1-byte codec tag: Deflate (level 6), or raw if compression saves less than 5 %. Already compressed CAD containers and ZIP archives are therefore not compressed a second time.

Chunks are verified against their hash when read. A restored file is also checked against its whole-file BLAKE3 hash.

### Change detection

`status` and `commit` scan the folder and use a **stat cache**. A file whose size and nanosecond mtime match the cache is not read again. Changed files are hashed in parallel with Rayon. A no-op `status` on a large project therefore costs one directory walk.

### Concurrency: a CAS-linearized write-ahead log

Every state change is an entry in an append-only log: commit, tag, lock or unlock. Appending works like this:

1. Read `index` (sequence number `n`) and its version tag.
2. Decide on the operation against that state, e.g. "file not locked by someone else" or "head is still my parent".
3. Write entry `wal/n+1` with *put-if-absent*.
4. Publish it by moving `index` from `n` to `n+1` with a single **compare-and-swap**. If the CAS fails, another writer won the race: re-read and retry from step 1.

The CAS on one small object is the only point of coordination, so all writers are linearized into one total order. The index is only a materialized view: `verify` rebuilds it by replaying the WAL and compares the result. An entry whose writer crashed between steps 3 and 4 is detected as an orphan and discarded after a timeout.

The store interface has the semantics of S3:

- `get`, `put`, `put_if_absent`, `cas_put`, `list`, `delete`
- `put_if_absent` and `cas_put` correspond to S3's conditional writes (`If-None-Match`, `If-Match`)

The local implementation follows the same semantics:

- **Atomic writes:** `fsync` to a temporary file, then rename.
- **`put_if_absent`:** creating a hard link, which fails atomically if the target exists.
- **`cas_put`:** an exclusive lock file with stale-lock recovery.

This is why a plain SMB share is enough, and why an object-store backend is a drop-in addition.

**Crash consistency.** Objects are always written bottom-up (chunks → manifest → commit) and become reachable only through a WAL entry. A crash at any point leaves at most unreferenced objects behind, and `gc` collects them. `gc` honors a grace period, because a concurrent writer's objects are unreferenced until it publishes.

### Synchronization

`push` and `pull` copy objects between two stores. Content addressing makes this a set difference: only chunks the target lacks are transferred, in parallel and verified. An interrupted transfer can simply be rerun. Commits are copied only after their data, and published on the target through the same WAL/CAS protocol. This gives:

- **Fast-forward check:** `push` requires the remote head to be an ancestor of the local head. The CAS rejects a concurrent push that happens in between.
- **File-level three-way rebase:** if the histories have diverged, `pull` finds the merge base *B* and replays each local commit's changes on top of the remote head *T*. A path is a conflict iff it changed in both B→local and B→T with a different result. Identical changes on both sides are not conflicts. Non-conflicting commits are rewritten with new parents. Commits whose changes become empty are dropped.
- **Lock checks:** locks are re-checked inside the CAS decision, so a lock taken during an upload still blocks publication.

Pre-rebase commits stay referenced by the WAL, so `gc` never deletes data that once was committed.

---

## Limitations and roadmap

- **Linear history.** There are no branches yet, and merging is at file granularity only. Merging inside CAD files would need vendor APIs or a parametric data model.
- **Remote types.** Remotes are folders and network shares for now. An S3 backend and an HTTP protocol, as the basis for a hosted service, are planned. Both fit the existing store interface.
- **Identity.** The client asserts the author, and there is no access control. That requires a server.
- **Tags.** Tag deletions are not propagated by `push` / `pull`.

---

## Development

```bash
cargo build
cargo test            # unit tests + CLI integration tests (tests/cli.rs)
cargo build --release
```

Source map:

| Module | Responsibility |
| --- | --- |
| `chunk.rs` | FastCDC chunking, codec, hashing, restore |
| `model.rs` | Manifest and commit objects, diffs |
| `store/` | `BlobStore` trait and the local filesystem implementation |
| `wal.rs` | Write-ahead log, index, CAS append, replay |
| `scan.rs` | Folder scan, ignore rules, stat cache |
| `repo.rs` | Repository discovery, configuration, revision resolution |
| `sync.rs` | Object transfer, ancestry, publishing for push/pull |
| `commands/` | One file per CLI command |

---

## License

Copyright © 2026 Nicolas Wörmann.

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
