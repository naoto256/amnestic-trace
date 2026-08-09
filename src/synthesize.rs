//! Capture-side hook boundary and detached snapshot synthesis.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::detach;
use crate::extract;
use crate::journal;
use crate::store::{self, Row, Store};

#[derive(Deserialize)]
struct Request {
    session_id: String,
    transcript_path: Option<PathBuf>,
    rollout_path: Option<PathBuf>,
}

/// Handles one host capture payload. The original process returns after the
/// detach; the worker finishes the extraction and publishes the ready marker.
pub fn run() -> io::Result<()> {
    detach::log_stderr_to(&Store::base_dir()?);
    let request = Request::read()?;
    let journal = request
        .journal()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "host payload names no journal"))?;
    synthesize(&request.session_id, &journal)
}

impl Request {
    fn read() -> io::Result<Self> {
        let mut raw = String::new();
        io::stdin().read_to_string(&mut raw)?;
        let request: Self = serde_json::from_str(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        store::validate_session_id(&request.session_id)?;
        Ok(request)
    }

    /// Prefers what the host actually pointed at. The `is_file` check is not a
    /// mere existence guard: a stale or not-yet-visible path is silently
    /// dropped and the Codex on-disk fallback takes over. Anything past that
    /// walk is a genuine "no journal for this session" and returns None.
    fn journal(&self) -> Option<PathBuf> {
        self.transcript_path
            .clone()
            .filter(|path| path.is_file())
            .or_else(|| self.rollout_path.clone())
            .filter(|path| path.is_file())
            .or_else(|| find_codex_journal(&self.session_id))
    }
}

/// Fallback for hosts (Codex today) whose hook payload does not carry the
/// journal path. Walks `~/.codex/sessions`, matching by session_id substring
/// in the filename. Unreadable subtrees are skipped: this path is best-effort,
/// but one damaged archived session must not hide a readable current rollout.
/// If no readable match remains, the caller reports a missing journal rather
/// than crashing the hook.
fn find_codex_journal(session_id: &str) -> Option<PathBuf> {
    let root = std::env::home_dir()?.join(".codex/sessions");
    find_codex_journal_under(root, session_id)
}

fn find_codex_journal_under(root: PathBuf, session_id: &str) -> Option<PathBuf> {
    let mut pending = vec![root];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
                && path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.contains(session_id))
            {
                return Some(path);
            }
        }
    }
    None
}

/// Orchestrates one capture: publish the debt while the hook is still on the
/// host's stack, then detach and do the extraction as an orphaned worker.
///
/// Ordering matters and is load-bearing in three places:
///
/// - **canonicalize before detach**: the worker chdirs to the store base
///   immediately after the fork, so a relative or cwd-anchored journal path
///   would resolve wrong. Resolving here also fixes the target while the host
///   still owns the filesystem view the payload was written against.
/// - **`prepare_debt` before `detach`**: the "ongoing" marker must be visible
///   before this process returns to the host. A later hook that fires before
///   the worker has started its own work then sees the debt and waits, rather
///   than the fast "no marker → nothing owed" path.
/// - **fail paths always drop the marker**: leaving `ongoing` behind is the
///   one unrecoverable failure — every later hook would sit through the full
///   wait budget for a worker that already isn't coming.
fn synthesize(session_id: &str, journal: &Path) -> io::Result<()> {
    let store = Store::open()?;
    let journal = journal
        .canonicalize()
        .unwrap_or_else(|_| journal.to_path_buf());
    prepare_debt(&store, session_id)?;

    match detach::detach() {
        detach::Role::Caller => return Ok(()),
        detach::Role::Worker => {}
        detach::Role::CannotDetach => {
            // Fail-open: the host would kill the extraction subprocess when
            // the hook exits, and finishing it under the host's process group
            // is not worth the marker looking real when it is not.
            eprintln!("{}: could not detach; giving up this window", store::now());
            drop_marker(&store, session_id);
            return Ok(());
        }
    }

    // Leave the project the session was working on before launching anything
    // else. The extraction agent receives its own empty scratch cwd below; the
    // detached parent has no reason to retain the project's cwd or expose it
    // accidentally to later relative-path operations.
    if let Err(error) = std::env::set_current_dir(store.base()) {
        eprintln!(
            "{}: could not move the detached parent to the store directory \
             before scratch setup: {error}",
            store::now()
        );
    }
    match work(&store, session_id, &journal) {
        Ok(key) => {
            // Extraction succeeded but the marker could not be flipped. The
            // row is written; only the delivery signal is missing. Report the
            // error so the log carries the stranded session's id — the next
            // synthesize will overwrite the row, so the loss is one window's
            // memory rather than a permanent gap.
            if let Err(error) = store.mark_ready(session_id, &key) {
                eprintln!(
                    "{}: extracted, but could not mark deliverable — the row at \
                     {session_id} is stranded until the next compaction: {error}",
                    store::now()
                );
                drop_marker(&store, session_id);
                return Err(error);
            }
        }
        Err(failure) => {
            eprintln!("{}: {failure}", store::now());
            drop_marker(&store, session_id);
        }
    }
    Ok(())
}

/// A new compaction is a new debt. Its marker becomes visible before the hook
/// returns, and no deadline or abandoned atomic-claim candidate may leak from
/// the previous debt into it.
fn prepare_debt(store: &Store, session_id: &str) -> io::Result<()> {
    remove_if_exists(&store.deadline_path(session_id))?;
    let marker_name = file_name(&store.marker_path(session_id));
    let deadline_name = file_name(&store.deadline_path(session_id));
    if let Ok(entries) = fs::read_dir(store.cortex()) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("{marker_name}.delivering."))
                || name.starts_with(&format!("{marker_name}.expiring."))
                || name.starts_with(&format!("{deadline_name}.publishing."))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    store.mark_ongoing(session_id)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn drop_marker(store: &Store, session_id: &str) {
    if let Err(error) = store.unmark(session_id) {
        eprintln!(
            "{}: could not clear the marker for {session_id}; later hooks may \
             wait out the delivery budget until it is gone: {error}",
            store::now()
        );
    }
}

fn work(store: &Store, session_id: &str, journal: &Path) -> Result<String, extract::Failed> {
    // A corrupt prior row is deliberately treated as absent rather than fatal
    // here. Refusing the compaction would leave the row unchanged, so every
    // future window pays the same read cost and nothing recovers. Because
    // `save` overwrites the row in place, treating it as a first-compaction
    // heals the store on success. The window boundary is lost with the row —
    // the acceptable cost of that heal.
    let prior = match store.load(session_id) {
        Ok(row) => row,
        Err(error) => {
            eprintln!(
                "{}: prior handoff unreadable, carrying nothing: {error}",
                store::now()
            );
            None
        }
    };
    let since = prior.as_ref().map(|row| row.compacted_at.clone());
    let window = journal::read_window(journal, since.as_deref())
        .map_err(|error| extract::Failed::Failed(format!("could not read the journal: {error}")))?;

    if window.text.trim().is_empty() {
        return Err(extract::Failed::Vacuous);
    }

    let prompt = store.extraction_prompt(extract::DEFAULT_PROMPT);
    let input = extract::compose(
        &prompt,
        prior.as_ref().map(|row| row.handoff.as_str()),
        &window.text,
    );
    let scratch = Scratch::new().map_err(|error| {
        extract::Failed::Failed(format!("could not make a working directory: {error}"))
    })?;
    let handoff = extract::run(window.host, &input, scratch.path())?;

    let key = store::mint_key();
    store
        .save(&Row {
            session_id: session_id.to_string(),
            amtr_key: Some(key.clone()),
            handoff,
            compacted_at: window.last_ts.unwrap_or_else(store::now),
        })
        .map_err(|error| {
            extract::Failed::Failed(format!("could not store the snapshot: {error}"))
        })?;
    Ok(key)
}

/// A per-process cwd for the extraction subprocess.
///
/// Both agent CLIs treat their cwd as fair game: Codex is passed `-C
/// <workdir>` and `extract::run` documents that its read-only sandbox does
/// not close the network or the hosted-tools surface. This directory is
/// owner-only (mode 0700 where the platform allows), lives under the system
/// temp directory rather than the store, and has a random tail so a coincident
/// name from another run does not collide or inherit its contents.
///
/// `Drop` removes it best-effort — a leaked directory is preferable to a
/// missing `?` failing the extraction over cleanup, and the OS periodically
/// clears the temp directory anyway.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> io::Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "amtr-work-{}-{}",
            std::process::id(),
            store::mint_key()
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&dir)?;
        }
        #[cfg(not(unix))]
        {
            fs::create_dir(&dir)?;
        }
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_session_ids_cannot_escape_the_store() {
        for invalid in ["", "../other", "space here", "雪"] {
            assert!(
                store::validate_session_id(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        assert!(store::validate_session_id("019efc46-72c1-7aa2.test_1").is_ok());
    }

    #[test]
    fn stale_transcript_path_falls_back_to_valid_rollout_path() {
        let root = std::env::temp_dir().join(format!(
            "amtr-journal-candidates-test-{}",
            crate::store::mint_key()
        ));
        fs::create_dir_all(&root).unwrap();
        let rollout = root.join("rollout.jsonl");
        fs::write(&rollout, "{}\n").unwrap();
        let request = Request {
            session_id: "session-a".into(),
            transcript_path: Some(root.join("stale.jsonl")),
            rollout_path: Some(rollout.clone()),
        };

        assert_eq!(request.journal(), Some(rollout));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn scratch_is_private_and_removes_itself() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = Scratch::new().unwrap();
        let path = scratch.path().to_path_buf();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        drop(scratch);
        assert!(!path.exists());
    }

    #[test]
    fn a_new_debt_sweeps_only_prior_operational_state() {
        let sequence = crate::store::mint_key();
        let base = std::env::temp_dir().join(format!("amtr-synthesize-test-{sequence}"));
        let _ = fs::remove_dir_all(&base);
        let store = Store::at(base).unwrap();
        let row = Row {
            session_id: "s".into(),
            amtr_key: Some("amtr-old".into()),
            handoff: "## Working state\nold".into(),
            compacted_at: "2026-08-09T00:00:00.000Z".into(),
        };
        store.save(&row).unwrap();
        fs::write(store.deadline_path("s"), "0").unwrap();
        fs::write(store.cortex().join("s.marker.delivering.1.2"), "ready:old").unwrap();
        fs::write(store.cortex().join("s.marker.expiring.1.3"), "ongoing").unwrap();
        fs::write(
            store.cortex().join("s.deliver-deadline.publishing.1.4"),
            "0",
        )
        .unwrap();

        prepare_debt(&store, "s").unwrap();
        assert_eq!(
            fs::read_to_string(store.marker_path("s")).unwrap(),
            "ongoing"
        );
        assert!(!store.deadline_path("s").exists());
        assert_eq!(store.load("s").unwrap(), Some(row));
        assert_eq!(fs::read_dir(store.cortex()).unwrap().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_codex_subtree_does_not_hide_a_readable_journal() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "amtr-journal-search-test-{}",
            crate::store::mint_key()
        ));
        let good = root.join("good");
        let blocked = root.join("zzz-blocked");
        fs::create_dir_all(&good).unwrap();
        fs::create_dir_all(&blocked).unwrap();
        let journal = good.join("rollout-session-123.jsonl");
        fs::write(&journal, "{}\n").unwrap();
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();

        let found = find_codex_journal_under(root.clone(), "session-123");

        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
        let _ = fs::remove_dir_all(root);
        assert_eq!(found, Some(journal));
    }

    #[cfg(unix)]
    #[test]
    fn journal_search_does_not_follow_symlinked_directories() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "amtr-journal-symlink-root-{}",
            crate::store::mint_key()
        ));
        let outside = std::env::temp_dir().join(format!(
            "amtr-journal-symlink-target-{}",
            crate::store::mint_key()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("rollout-session-123.jsonl"), "{}\n").unwrap();
        symlink(&outside, root.join("linked")).unwrap();

        let found = find_codex_journal_under(root.clone(), "session-123");

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
        assert_eq!(found, None);
    }
}
